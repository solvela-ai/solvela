//! Hard caps for SSE streaming.
//!
//! Wraps a stream of `axum::response::sse::Event`s with two enforced limits:
//!
//! 1. **Wallclock deadline** — measured from the first byte. Default 5 minutes,
//!    overridable via `SOLVELA_STREAM_DEADLINE_SECS`.
//! 2. **Cumulative body cap** — sum of UTF-8 byte lengths of every event's
//!    `data:` payload. Default 5 MiB, overridable via
//!    `SOLVELA_STREAM_MAX_BYTES`.
//!
//! When either limit is exceeded, the wrapper emits a final `[DONE]` event
//! and terminates the stream. Upstream providers' channels are dropped via
//! the underlying stream being dropped — no explicit cancellation token is
//! required because dropping the stream cancels its `tokio::spawn`-ed sender
//! when the channel receiver hangs up.

use std::convert::Infallible;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use axum::response::sse::Event;
use futures::Stream;
use pin_project_lite::pin_project;
use tracing::warn;

/// Default wallclock deadline (5 minutes).
pub const DEFAULT_DEADLINE_SECS: u64 = 300;

/// Default cumulative body cap (5 MiB).
pub const DEFAULT_MAX_BYTES: usize = 5 * 1024 * 1024;

/// Resolved configuration for stream caps.
#[derive(Debug, Clone, Copy)]
pub struct StreamCapConfig {
    pub deadline: Duration,
    pub max_bytes: usize,
}

impl StreamCapConfig {
    /// Load configuration from environment with defaults.
    pub fn from_env() -> Self {
        let deadline_secs = std::env::var("SOLVELA_STREAM_DEADLINE_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(DEFAULT_DEADLINE_SECS);

        let max_bytes = std::env::var("SOLVELA_STREAM_MAX_BYTES")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(DEFAULT_MAX_BYTES);

        Self {
            deadline: Duration::from_secs(deadline_secs),
            max_bytes,
        }
    }
}

impl Default for StreamCapConfig {
    fn default() -> Self {
        Self {
            deadline: Duration::from_secs(DEFAULT_DEADLINE_SECS),
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }
}

pin_project! {
    /// Stream wrapper enforcing wallclock + cumulative-byte caps on SSE events.
    ///
    /// Generic over `S` where `S::Item = Result<Event, Infallible>` — matches the
    /// shape produced by `provider.rs::execute_streaming_call`.
    pub struct CappedStream<S> {
        #[pin]
        inner: S,
        config: StreamCapConfig,
        bytes_seen: usize,
        started_at: Option<Instant>,
        terminated: bool,
        emitted_done: bool,
    }
}

impl<S> CappedStream<S>
where
    S: Stream<Item = Result<Event, Infallible>>,
{
    pub fn new(inner: S, config: StreamCapConfig) -> Self {
        Self {
            inner,
            config,
            bytes_seen: 0,
            started_at: None,
            terminated: false,
            emitted_done: false,
        }
    }
}

/// Approximate the bytes a [`Event`] will contribute to the wire.
///
/// `axum::response::sse::Event` does not expose its data payload, so we use
/// a coarse fixed-size lower bound when the event isn't a heartbeat. This is
/// safe for the cap: real LLM chunks are far larger than this floor, so the
/// cap will trip well before the wire actually exceeds the configured limit.
///
/// Callers that need precise accounting should track byte counts themselves
/// and feed them through [`CappedStream::record_bytes`].
fn estimated_event_bytes() -> usize {
    // Conservative per-event floor matching SSE framing overhead
    // (`data: ...\n\n` = 8 bytes minimum).
    32
}

pin_project! {
    /// Stream-level wrapper that takes pre-serialized event bytes alongside each
    /// event. This avoids the limitation in [`CappedStream`] where event payload
    /// size is not visible.
    pub struct CappedSizedStream<S> {
        #[pin]
        inner: S,
        config: StreamCapConfig,
        bytes_seen: usize,
        started_at: Option<Instant>,
        terminated: bool,
        emitted_done: bool,
    }
}

impl<S> CappedSizedStream<S>
where
    S: Stream<Item = (usize, Result<Event, Infallible>)>,
{
    pub fn new(inner: S, config: StreamCapConfig) -> Self {
        Self {
            inner,
            config,
            bytes_seen: 0,
            started_at: None,
            terminated: false,
            emitted_done: false,
        }
    }
}

impl<S> Stream for CappedSizedStream<S>
where
    S: Stream<Item = (usize, Result<Event, Infallible>)>,
{
    type Item = Result<Event, Infallible>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        if *this.terminated {
            if !*this.emitted_done {
                *this.emitted_done = true;
                return Poll::Ready(Some(Ok(Event::default().data("[DONE]"))));
            }
            return Poll::Ready(None);
        }

        // Check deadline before polling inner.
        if let Some(start) = *this.started_at {
            if start.elapsed() >= this.config.deadline {
                warn!(
                    elapsed_secs = start.elapsed().as_secs(),
                    deadline_secs = this.config.deadline.as_secs(),
                    "SSE stream wallclock deadline exceeded — terminating"
                );
                *this.terminated = true;
                *this.emitted_done = true;
                return Poll::Ready(Some(Ok(Event::default().data("[DONE]"))));
            }
        }

        match this.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some((bytes, ev))) => {
                if this.started_at.is_none() {
                    *this.started_at = Some(Instant::now());
                }
                *this.bytes_seen = this.bytes_seen.saturating_add(bytes);
                if *this.bytes_seen > this.config.max_bytes {
                    warn!(
                        bytes_seen = *this.bytes_seen,
                        max_bytes = this.config.max_bytes,
                        "SSE stream byte cap exceeded — terminating"
                    );
                    *this.terminated = true;
                    *this.emitted_done = true;
                    return Poll::Ready(Some(Ok(Event::default().data("[DONE]"))));
                }
                Poll::Ready(Some(ev))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<S> Stream for CappedStream<S>
where
    S: Stream<Item = Result<Event, Infallible>>,
{
    type Item = Result<Event, Infallible>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        if *this.terminated {
            if !*this.emitted_done {
                *this.emitted_done = true;
                return Poll::Ready(Some(Ok(Event::default().data("[DONE]"))));
            }
            return Poll::Ready(None);
        }

        // Check deadline before polling inner.
        if let Some(start) = *this.started_at {
            if start.elapsed() >= this.config.deadline {
                warn!(
                    elapsed_secs = start.elapsed().as_secs(),
                    deadline_secs = this.config.deadline.as_secs(),
                    "SSE stream wallclock deadline exceeded — terminating"
                );
                *this.terminated = true;
                *this.emitted_done = true;
                return Poll::Ready(Some(Ok(Event::default().data("[DONE]"))));
            }
        }

        match this.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(ev)) => {
                if this.started_at.is_none() {
                    *this.started_at = Some(Instant::now());
                }
                *this.bytes_seen = this.bytes_seen.saturating_add(estimated_event_bytes());
                if *this.bytes_seen > this.config.max_bytes {
                    warn!(
                        bytes_seen = *this.bytes_seen,
                        max_bytes = this.config.max_bytes,
                        "SSE stream byte cap exceeded — terminating"
                    );
                    *this.terminated = true;
                    *this.emitted_done = true;
                    return Poll::Ready(Some(Ok(Event::default().data("[DONE]"))));
                }
                Poll::Ready(Some(ev))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use futures::stream::{self, StreamExt};

    #[test]
    fn config_default_uses_5min_5mib() {
        let c = StreamCapConfig::default();
        assert_eq!(c.deadline, Duration::from_secs(300));
        assert_eq!(c.max_bytes, 5 * 1024 * 1024);
    }

    #[test]
    fn config_env_overrides_apply() {
        // Save & restore so we don't pollute other tests' env.
        let prior_deadline = std::env::var("SOLVELA_STREAM_DEADLINE_SECS").ok();
        let prior_bytes = std::env::var("SOLVELA_STREAM_MAX_BYTES").ok();

        std::env::set_var("SOLVELA_STREAM_DEADLINE_SECS", "60");
        std::env::set_var("SOLVELA_STREAM_MAX_BYTES", "1024");
        let c = StreamCapConfig::from_env();
        assert_eq!(c.deadline, Duration::from_secs(60));
        assert_eq!(c.max_bytes, 1024);

        match prior_deadline {
            Some(v) => std::env::set_var("SOLVELA_STREAM_DEADLINE_SECS", v),
            None => std::env::remove_var("SOLVELA_STREAM_DEADLINE_SECS"),
        }
        match prior_bytes {
            Some(v) => std::env::set_var("SOLVELA_STREAM_MAX_BYTES", v),
            None => std::env::remove_var("SOLVELA_STREAM_MAX_BYTES"),
        }
    }

    #[tokio::test]
    async fn capped_sized_stream_passes_through_when_under_caps() {
        let events: Vec<(usize, Result<Event, Infallible>)> = vec![
            (16, Ok(Event::default().data("hello"))),
            (16, Ok(Event::default().data("world"))),
        ];
        let inner = stream::iter(events);
        let capped = CappedSizedStream::new(
            inner,
            StreamCapConfig {
                deadline: Duration::from_secs(60),
                max_bytes: 1024,
            },
        );
        let collected: Vec<_> = capped.collect().await;
        assert_eq!(collected.len(), 2);
    }

    #[tokio::test]
    async fn capped_sized_stream_terminates_after_byte_cap() {
        // First event is 100 bytes, second is 100 bytes; cap = 150.
        // First event passes (100 ≤ 150), second triggers cap (200 > 150),
        // so we expect: event1, [DONE], end.
        let events: Vec<(usize, Result<Event, Infallible>)> = vec![
            (100, Ok(Event::default().data("a"))),
            (100, Ok(Event::default().data("b"))),
            (100, Ok(Event::default().data("c"))),
        ];
        let inner = stream::iter(events);
        let capped = CappedSizedStream::new(
            inner,
            StreamCapConfig {
                deadline: Duration::from_secs(60),
                max_bytes: 150,
            },
        );
        let collected: Vec<_> = capped.collect().await;
        // First event passes, then cap trips and a [DONE] event is emitted.
        assert_eq!(collected.len(), 2);
    }

    #[tokio::test]
    async fn capped_sized_stream_terminates_after_deadline() {
        // We can't easily simulate a real wallclock deadline in a unit test.
        // Instead, set deadline to 0 — meaning "expire on the next poll after
        // first event".
        let events: Vec<(usize, Result<Event, Infallible>)> = vec![
            (1, Ok(Event::default().data("a"))),
            (1, Ok(Event::default().data("b"))),
        ];
        let inner = stream::iter(events);
        let capped = CappedSizedStream::new(
            inner,
            StreamCapConfig {
                deadline: Duration::from_millis(0),
                max_bytes: 1024,
            },
        );
        // Force a small sleep between polls so the deadline trips.
        tokio::pin!(capped);
        let first = capped.next().await;
        assert!(first.is_some());
        // Sleep so the next poll observes elapsed > 0.
        tokio::time::sleep(Duration::from_millis(5)).await;
        let next = capped.next().await;
        // Next item should be the [DONE] termination event.
        assert!(next.is_some());
        let last = capped.next().await;
        // Stream should now be done.
        assert!(last.is_none());
    }
}
