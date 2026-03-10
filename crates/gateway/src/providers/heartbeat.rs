//! Adaptive SSE heartbeat stream wrapper.
//!
//! Wraps a [`ChatStream`] to inject keep-alive events when the upstream
//! provider is slow. This prevents proxy and client timeouts when slow
//! models (e.g., Opus doing deep reasoning) take 10-30 seconds before
//! emitting the first token.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use futures::Stream;
use pin_project_lite::pin_project;
use tokio::time::{Instant, Sleep};

use rustyclaw_protocol::ChatChunk;

use super::ProviderError;

/// Sentinel string used to identify heartbeat events in SSE output.
pub const HEARTBEAT_SENTINEL: &str = "__heartbeat__";

/// Configuration for adaptive heartbeat intervals.
#[derive(Debug, Clone)]
pub struct HeartbeatConfig {
    /// Initial keep-alive interval (default 5s).
    pub initial_interval: Duration,
    /// Faster keep-alive interval used after prolonged silence (default 2s).
    pub fast_interval: Duration,
    /// Duration of silence after which we switch to `fast_interval` (default 10s).
    pub accelerate_after: Duration,
}

impl Default for HeartbeatConfig {
    fn default() -> Self {
        Self {
            initial_interval: Duration::from_secs(5),
            fast_interval: Duration::from_secs(2),
            accelerate_after: Duration::from_secs(10),
        }
    }
}

/// Item emitted by [`HeartbeatStream`].
#[derive(Debug)]
pub enum HeartbeatItem {
    /// A real chunk from the upstream provider.
    Chunk(Result<ChatChunk, ProviderError>),
    /// A keep-alive signal — no data, just prevents timeouts.
    KeepAlive,
}

pin_project! {
    /// Stream wrapper that injects heartbeat keep-alive events when the
    /// inner stream is silent for too long.
    ///
    /// When the inner stream yields data, the timer resets. When the timer
    /// fires before data arrives, a [`HeartbeatItem::KeepAlive`] is emitted.
    /// After prolonged silence (>= `accelerate_after`), the heartbeat
    /// switches to the faster interval.
    pub struct HeartbeatStream<S> {
        #[pin]
        inner: S,
        config: HeartbeatConfig,
        #[pin]
        sleep: Sleep,
        last_chunk_at: Instant,
        inner_done: bool,
    }
}

impl<S> HeartbeatStream<S>
where
    S: Stream<Item = Result<ChatChunk, ProviderError>>,
{
    /// Wrap an inner stream with adaptive heartbeat keep-alives.
    pub fn new(inner: S, config: HeartbeatConfig) -> Self {
        let now = Instant::now();
        let sleep = tokio::time::sleep_until(now + config.initial_interval);
        Self {
            inner,
            config,
            sleep,
            last_chunk_at: now,
            inner_done: false,
        }
    }
}

impl<S> Stream for HeartbeatStream<S>
where
    S: Stream<Item = Result<ChatChunk, ProviderError>>,
{
    type Item = HeartbeatItem;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        // If the inner stream is not done, poll it first.
        if !*this.inner_done {
            match this.inner.as_mut().poll_next(cx) {
                Poll::Ready(Some(item)) => {
                    let now = Instant::now();
                    *this.last_chunk_at = now;
                    // Reset the sleep timer with the initial interval.
                    this.sleep
                        .as_mut()
                        .reset(now + this.config.initial_interval);
                    return Poll::Ready(Some(HeartbeatItem::Chunk(item)));
                }
                Poll::Ready(None) => {
                    *this.inner_done = true;
                    return Poll::Ready(None);
                }
                Poll::Pending => {
                    // Inner is not ready — fall through to check the timer.
                }
            }
        } else {
            return Poll::Ready(None);
        }

        // Inner stream is pending — check if the heartbeat timer has fired.
        match this.sleep.as_mut().poll(cx) {
            Poll::Ready(()) => {
                let now = Instant::now();
                let silence = now.duration_since(*this.last_chunk_at);
                let next_interval = if silence >= this.config.accelerate_after {
                    this.config.fast_interval
                } else {
                    this.config.initial_interval
                };
                this.sleep.as_mut().reset(now + next_interval);
                Poll::Ready(Some(HeartbeatItem::KeepAlive))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures::StreamExt;
    use tokio_stream::wrappers::ReceiverStream;

    use rustyclaw_protocol::{ChatChunk, ChatChunkChoice, ChatDelta};

    use super::*;

    /// Helper to create a minimal `ChatChunk` for testing.
    fn test_chunk(content: &str) -> ChatChunk {
        ChatChunk {
            id: "test-id".to_string(),
            object: "chat.completion.chunk".to_string(),
            created: 0,
            model: "test-model".to_string(),
            choices: vec![ChatChunkChoice {
                index: 0,
                delta: ChatDelta {
                    role: None,
                    content: Some(content.to_string()),
                    tool_calls: None,
                },
                finish_reason: None,
            }],
        }
    }

    #[tokio::test]
    async fn test_heartbeat_passes_through_chunks() {
        // Chunks arrive immediately — no heartbeats should fire.
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<ChatChunk, ProviderError>>(10);

        // Send chunks immediately.
        tx.send(Ok(test_chunk("hello"))).await.unwrap();
        tx.send(Ok(test_chunk("world"))).await.unwrap();
        drop(tx);

        let config = HeartbeatConfig {
            initial_interval: Duration::from_secs(60),
            fast_interval: Duration::from_secs(30),
            accelerate_after: Duration::from_secs(120),
        };

        let stream = HeartbeatStream::new(ReceiverStream::new(rx), config);
        let items: Vec<HeartbeatItem> = stream.collect().await;

        assert_eq!(items.len(), 2);
        for item in &items {
            assert!(
                matches!(item, HeartbeatItem::Chunk(Ok(_))),
                "expected Chunk, got KeepAlive"
            );
        }
    }

    #[tokio::test]
    async fn test_heartbeat_emits_keepalive_on_silence() {
        // Stream stays silent — heartbeat should fire.
        let (_tx, rx) = tokio::sync::mpsc::channel::<Result<ChatChunk, ProviderError>>(10);

        let config = HeartbeatConfig {
            initial_interval: Duration::from_millis(50),
            fast_interval: Duration::from_millis(25),
            accelerate_after: Duration::from_secs(60),
        };

        let stream = HeartbeatStream::new(ReceiverStream::new(rx), config);
        tokio::pin!(stream);

        // The first item should be a KeepAlive after ~50ms.
        let item = tokio::time::timeout(Duration::from_millis(200), stream.next())
            .await
            .expect("timed out waiting for heartbeat")
            .expect("stream ended unexpectedly");

        assert!(
            matches!(item, HeartbeatItem::KeepAlive),
            "expected KeepAlive, got Chunk"
        );
    }

    #[tokio::test]
    async fn test_heartbeat_accelerates_after_prolonged_silence() {
        // After `accelerate_after`, heartbeats should switch to fast_interval.
        let (_tx, rx) = tokio::sync::mpsc::channel::<Result<ChatChunk, ProviderError>>(10);

        let config = HeartbeatConfig {
            initial_interval: Duration::from_millis(50),
            fast_interval: Duration::from_millis(20),
            accelerate_after: Duration::from_millis(100),
        };

        let stream = HeartbeatStream::new(ReceiverStream::new(rx), config);
        tokio::pin!(stream);

        // Collect heartbeats over ~200ms. After the first 100ms of silence
        // the interval should switch from 50ms to 20ms, so we should see
        // more heartbeats in the second half.
        let mut timestamps = Vec::new();
        let start = Instant::now();
        let deadline = start + Duration::from_millis(200);

        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, stream.next()).await {
                Ok(Some(HeartbeatItem::KeepAlive)) => {
                    timestamps.push(Instant::now());
                }
                Ok(Some(HeartbeatItem::Chunk(_))) => {
                    panic!("unexpected chunk");
                }
                Ok(None) => break,
                Err(_) => break, // timeout — done collecting
            }
        }

        // We should have received at least 4 heartbeats:
        // ~50ms, ~100ms (switch to fast), ~120ms, ~140ms, ~160ms, ~180ms
        assert!(
            timestamps.len() >= 4,
            "expected at least 4 heartbeats during 200ms window, got {}",
            timestamps.len()
        );

        // Verify the later intervals are shorter than the earlier ones.
        if timestamps.len() >= 3 {
            let first_gap = timestamps[1].duration_since(timestamps[0]);
            let last_gap =
                timestamps[timestamps.len() - 1].duration_since(timestamps[timestamps.len() - 2]);
            assert!(
                last_gap < first_gap,
                "expected acceleration: first gap={first_gap:?}, last gap={last_gap:?}"
            );
        }
    }

    #[tokio::test]
    async fn test_heartbeat_resets_timer_on_chunk() {
        // Chunks arriving faster than the heartbeat interval should
        // prevent heartbeats from firing.
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<ChatChunk, ProviderError>>(10);

        let config = HeartbeatConfig {
            initial_interval: Duration::from_millis(100),
            fast_interval: Duration::from_millis(50),
            accelerate_after: Duration::from_secs(60),
        };

        let stream = HeartbeatStream::new(ReceiverStream::new(rx), config);
        tokio::pin!(stream);

        // Send chunks every 30ms — well within the 100ms heartbeat window.
        let sender = tokio::spawn(async move {
            for i in 0..5 {
                tx.send(Ok(test_chunk(&format!("chunk-{i}"))))
                    .await
                    .unwrap();
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
            // Drop tx to close the stream.
        });

        let mut chunks = 0;
        let mut keepalives = 0;

        while let Some(item) = stream.next().await {
            match item {
                HeartbeatItem::Chunk(Ok(_)) => chunks += 1,
                HeartbeatItem::Chunk(Err(e)) => panic!("unexpected error: {e}"),
                HeartbeatItem::KeepAlive => keepalives += 1,
            }
        }

        sender.await.unwrap();

        assert_eq!(chunks, 5, "expected 5 chunks");
        assert_eq!(
            keepalives, 0,
            "expected no heartbeats when chunks arrive faster than interval"
        );
    }
}
