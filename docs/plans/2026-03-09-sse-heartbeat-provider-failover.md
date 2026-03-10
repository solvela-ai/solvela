# SSE Heartbeat + Per-Model Provider Failover Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add adaptive SSE heartbeat keep-alive to prevent proxy/client timeouts during slow model inference, and upgrade the circuit breaker from per-provider to per-model granularity with transparent fallback via `X-RCR-Fallback` header.

**Architecture:** Two independent features that both live in the `gateway` crate. The heartbeat wraps the existing SSE stream with `tokio::select!` to inject `: keep-alive\n\n` comments on an adaptive timer (5s initial → 2s after 10s silence). The circuit breaker extends the existing `ProviderHealthTracker` to track health per model ID (not just per provider), with model-level fallback chains defined alongside the existing provider-level chains. When a request is served by a fallback model, the response includes an `X-RCR-Fallback` header.

**Tech Stack:** Rust, Axum 0.8, Tokio (select!, interval), futures::Stream, Tower middleware, existing `ProviderHealthTracker`

---

## Context for the Implementer

### Existing Code You Need to Know

1. **`crates/gateway/src/providers/health.rs`** — Per-provider circuit breaker with `CircuitState` (Closed/Open/HalfOpen), `ProviderHealthTracker` using `Arc<RwLock<HashMap<String, ProviderHealth>>>`. Config: 50% failure threshold, 30s cooldown, 60s window, 5 min_requests. Methods: `record_success`, `record_failure`, `is_available`, `get_state`, `get_failure_rate`.

2. **`crates/gateway/src/providers/fallback.rs`** — `chat_with_fallback` and `stream_with_fallback` iterate a provider chain, skip providers with open circuits, record outcomes. `fallback_chain(primary)` returns hardcoded provider-name chains (e.g., openai → anthropic → google → deepseek).

3. **`crates/gateway/src/routes/chat.rs`** — Main handler. For streaming: calls `fallback::stream_with_fallback`, wraps result in `sse::Sse`, maps chunks to `sse::Event::default().data(json)`. For non-streaming: calls `fallback::chat_with_fallback`, returns `Json`.

4. **`crates/gateway/src/providers/mod.rs`** — `LLMProvider` trait with `chat_completion` and `chat_completion_stream`. `ProviderRegistry` maps provider names to `Arc<dyn LLMProvider>`. `spawn_openai_sse_parser` creates `ChatStream` from reqwest response bytes.

5. **`crates/gateway/src/lib.rs`** — `AppState` struct has `provider_health: ProviderHealthTracker`. `build_router` wires everything up.

6. **`config/models.toml`** — 27 models across 5 providers with `provider` field linking each model to its provider name.

### Key Type: `ChatStream`
```rust
pub type ChatStream = Pin<Box<dyn Stream<Item = Result<ChatChunk, ProviderError>> + Send>>;
```

### Test Patterns
- Unit tests: `#[tokio::test]`, use `test_config()` helpers with short durations
- Integration tests: `tower::ServiceExt::oneshot` with `test_app()` in `crates/gateway/tests/integration.rs`
- Run: `cargo test -p gateway --lib` (unit), `cargo test -p gateway --test integration` (integration)

---

## Task 1: Adaptive SSE Heartbeat Stream Wrapper

**Files:**
- Create: `crates/gateway/src/providers/heartbeat.rs`
- Modify: `crates/gateway/src/providers/mod.rs` (add `pub mod heartbeat;`)

**Step 1: Write the failing test**

Create `crates/gateway/src/providers/heartbeat.rs` with tests only:

```rust
//! Adaptive SSE heartbeat wrapper.
//!
//! Wraps a `ChatStream` to inject SSE keep-alive comments when the upstream
//! provider is slow to emit tokens. Prevents proxy and client timeouts.
//!
//! Adaptive timing: starts at 5s, accelerates to 2s after 10s of total silence.

use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use futures::Stream;
use pin_project_lite::pin_project;
use tokio::time::{Instant, Sleep};
use tracing::debug;

use rcr_common::types::ChatChunk;

use super::ProviderError;

/// Sentinel value returned by the heartbeat stream to signal a keep-alive.
/// The SSE layer should emit this as a `: keep-alive\n\n` comment.
pub const HEARTBEAT_SENTINEL: &str = "__heartbeat__";

/// Configuration for the adaptive heartbeat.
#[derive(Debug, Clone)]
pub struct HeartbeatConfig {
    /// Initial interval between keep-alive comments.
    pub initial_interval: Duration,
    /// Accelerated interval after `accelerate_after` of silence.
    pub fast_interval: Duration,
    /// Switch to fast_interval after this much total silence.
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

pin_project! {
    /// A stream wrapper that emits heartbeat events when the inner stream
    /// is silent for too long.
    pub struct HeartbeatStream<S> {
        #[pin]
        inner: S,
        config: HeartbeatConfig,
        #[pin]
        sleep: Sleep,
        /// When the last real chunk was received.
        last_chunk_at: Instant,
        /// Whether the inner stream has finished.
        inner_done: bool,
    }
}

impl<S> HeartbeatStream<S>
where
    S: Stream<Item = Result<ChatChunk, ProviderError>>,
{
    /// Wrap a chat stream with adaptive heartbeat keep-alive.
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

    fn current_interval(&self) -> Duration {
        let silence = self.last_chunk_at.elapsed();
        if silence >= self.config.accelerate_after {
            self.config.fast_interval
        } else {
            self.config.initial_interval
        }
    }
}

/// Result type for heartbeat stream items.
/// Either a real chunk from the provider or a heartbeat keep-alive.
pub enum HeartbeatItem {
    Chunk(Result<ChatChunk, ProviderError>),
    KeepAlive,
}

impl<S> Stream for HeartbeatStream<S>
where
    S: Stream<Item = Result<ChatChunk, ProviderError>>,
{
    type Item = HeartbeatItem;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        // If inner stream is done, we're done.
        if *this.inner_done {
            return Poll::Ready(None);
        }

        // Poll inner stream first.
        match this.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(item)) => {
                *this.last_chunk_at = Instant::now();
                // Reset the sleep timer.
                let interval = if this.last_chunk_at.elapsed() >= this.config.accelerate_after {
                    this.config.fast_interval
                } else {
                    this.config.initial_interval
                };
                this.sleep
                    .as_mut()
                    .reset(Instant::now() + interval);
                return Poll::Ready(Some(HeartbeatItem::Chunk(item)));
            }
            Poll::Ready(None) => {
                *this.inner_done = true;
                return Poll::Ready(None);
            }
            Poll::Pending => {}
        }

        // Inner is pending — check if heartbeat timer fired.
        match this.sleep.as_mut().poll(cx) {
            Poll::Ready(()) => {
                debug!("emitting SSE heartbeat keep-alive");
                // Reset timer with current interval (may have accelerated).
                let silence = this.last_chunk_at.elapsed();
                let interval = if silence >= this.config.accelerate_after {
                    this.config.fast_interval
                } else {
                    this.config.initial_interval
                };
                this.sleep.as_mut().reset(Instant::now() + interval);
                Poll::Ready(Some(HeartbeatItem::KeepAlive))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;
    use rcr_common::types::{ChatChunk, ChatChunkChoice, ChatDelta};

    fn make_chunk(content: &str) -> ChatChunk {
        ChatChunk {
            id: "test".to_string(),
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
        let chunks = vec![
            Ok(make_chunk("Hello")),
            Ok(make_chunk(" world")),
        ];
        let inner = futures::stream::iter(chunks);
        let config = HeartbeatConfig {
            initial_interval: Duration::from_secs(60), // Won't fire
            fast_interval: Duration::from_secs(30),
            accelerate_after: Duration::from_secs(120),
        };

        let mut stream = HeartbeatStream::new(inner, config);
        let mut results = Vec::new();
        while let Some(item) = stream.next().await {
            match item {
                HeartbeatItem::Chunk(Ok(chunk)) => {
                    results.push(chunk.choices[0].delta.content.clone());
                }
                HeartbeatItem::Chunk(Err(e)) => panic!("unexpected error: {e}"),
                HeartbeatItem::KeepAlive => results.push(Some("__heartbeat__".to_string())),
            }
        }

        assert_eq!(results.len(), 2);
        assert_eq!(results[0], Some("Hello".to_string()));
        assert_eq!(results[1], Some(" world".to_string()));
    }

    #[tokio::test]
    async fn test_heartbeat_emits_keepalive_on_silence() {
        // Create a stream that yields one chunk, then pauses, then another chunk
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<ChatChunk, ProviderError>>(10);

        let config = HeartbeatConfig {
            initial_interval: Duration::from_millis(50),
            fast_interval: Duration::from_millis(20),
            accelerate_after: Duration::from_millis(100),
        };

        let rx_stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        let mut stream = HeartbeatStream::new(rx_stream, config);

        // Send first chunk
        tx.send(Ok(make_chunk("first"))).await.unwrap();

        // Read first chunk
        let item = stream.next().await.unwrap();
        assert!(matches!(item, HeartbeatItem::Chunk(Ok(_))));

        // Wait for heartbeat (>50ms)
        let item = tokio::time::timeout(Duration::from_millis(200), stream.next())
            .await
            .expect("should get heartbeat before timeout")
            .unwrap();
        assert!(matches!(item, HeartbeatItem::KeepAlive));

        // Send second chunk and clean up
        tx.send(Ok(make_chunk("second"))).await.unwrap();
        let item = stream.next().await.unwrap();
        assert!(matches!(item, HeartbeatItem::Chunk(Ok(_))));

        drop(tx);
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn test_heartbeat_accelerates_after_prolonged_silence() {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<ChatChunk, ProviderError>>(10);

        let config = HeartbeatConfig {
            initial_interval: Duration::from_millis(50),
            fast_interval: Duration::from_millis(20),
            accelerate_after: Duration::from_millis(80),
        };

        let rx_stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        let mut stream = HeartbeatStream::new(rx_stream, config);

        // Wait for first heartbeat (~50ms)
        let start = Instant::now();
        let item = tokio::time::timeout(Duration::from_millis(200), stream.next())
            .await
            .expect("timeout")
            .unwrap();
        assert!(matches!(item, HeartbeatItem::KeepAlive));
        let first_heartbeat = start.elapsed();

        // Wait for second heartbeat — should still be ~50ms (not yet accelerated)
        // After ~80ms total silence, should accelerate to 20ms
        let item = tokio::time::timeout(Duration::from_millis(200), stream.next())
            .await
            .expect("timeout")
            .unwrap();
        assert!(matches!(item, HeartbeatItem::KeepAlive));

        // By now we've been silent > 80ms, so third heartbeat should come faster (~20ms)
        let fast_start = Instant::now();
        let item = tokio::time::timeout(Duration::from_millis(200), stream.next())
            .await
            .expect("timeout")
            .unwrap();
        assert!(matches!(item, HeartbeatItem::KeepAlive));
        let fast_heartbeat = fast_start.elapsed();

        // The fast heartbeat should be significantly shorter than the first
        assert!(
            fast_heartbeat < first_heartbeat,
            "fast heartbeat ({:?}) should be shorter than first ({:?})",
            fast_heartbeat,
            first_heartbeat
        );

        drop(tx);
    }

    #[tokio::test]
    async fn test_heartbeat_resets_timer_on_chunk() {
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<ChatChunk, ProviderError>>(10);

        let config = HeartbeatConfig {
            initial_interval: Duration::from_millis(100),
            fast_interval: Duration::from_millis(50),
            accelerate_after: Duration::from_millis(200),
        };

        let rx_stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        let mut stream = HeartbeatStream::new(rx_stream, config);

        // Send chunks every 60ms — faster than heartbeat interval (100ms)
        // So no heartbeat should fire
        for i in 0..3 {
            tokio::time::sleep(Duration::from_millis(60)).await;
            tx.send(Ok(make_chunk(&format!("chunk-{i}")))).await.unwrap();
            let item = stream.next().await.unwrap();
            assert!(
                matches!(item, HeartbeatItem::Chunk(Ok(_))),
                "expected chunk, got heartbeat at iteration {i}"
            );
        }

        drop(tx);
    }
}
```

**Step 2: Add `pin-project-lite` and `tokio-stream` dependencies**

Check if `pin-project-lite` is already a dependency. If not, add it:

```bash
# Check workspace Cargo.toml for pin-project-lite
grep -r "pin-project-lite" Cargo.toml crates/gateway/Cargo.toml
```

Add to `crates/gateway/Cargo.toml` under `[dependencies]`:
```toml
pin-project-lite = "0.2"
```

Add to `crates/gateway/Cargo.toml` under `[dev-dependencies]`:
```toml
tokio-stream = "0.1"
```

**Step 3: Add module declaration**

In `crates/gateway/src/providers/mod.rs`, add after the existing module declarations:
```rust
pub mod heartbeat;
```

**Step 4: Run tests to verify they pass**

```bash
cargo test -p gateway heartbeat -- --nocapture
```

Expected: All 4 heartbeat tests pass.

**Step 5: Commit**

```bash
git add crates/gateway/src/providers/heartbeat.rs crates/gateway/src/providers/mod.rs crates/gateway/Cargo.toml
git commit -m "feat: add adaptive SSE heartbeat stream wrapper

Wraps ChatStream with keep-alive comments on an adaptive timer
(5s initial → 2s after 10s silence) to prevent proxy/client timeouts
during slow model inference."
```

---

## Task 2: Integrate Heartbeat into Chat Streaming Path

**Files:**
- Modify: `crates/gateway/src/routes/chat.rs:249-289` (streaming branch)

**Step 1: Write the failing test**

Add a unit test to `crates/gateway/src/routes/chat.rs` in the `#[cfg(test)] mod tests` block:

```rust
#[test]
fn test_heartbeat_sentinel_is_defined() {
    // Smoke test that the heartbeat module is accessible from routes
    assert_eq!(
        crate::providers::heartbeat::HEARTBEAT_SENTINEL,
        "__heartbeat__"
    );
}
```

**Step 2: Run test to verify it passes**

```bash
cargo test -p gateway test_heartbeat_sentinel -- --exact
```

Expected: PASS (module exists from Task 1).

**Step 3: Modify the streaming SSE path in chat.rs**

Replace the streaming branch (lines ~249-289) in `chat_completions`. The key change: wrap the provider stream with `HeartbeatStream`, then map `HeartbeatItem` variants to SSE events — `KeepAlive` becomes an SSE comment (`: keep-alive`), `Chunk` becomes a data event.

In `chat.rs`, add this import at the top:
```rust
use crate::providers::heartbeat::{HeartbeatConfig, HeartbeatItem, HeartbeatStream};
```

Replace the streaming match arm (the `Ok(stream) =>` block inside the `if req.stream` branch):

```rust
Ok(stream) => {
    // Fire escrow claim before returning the stream.
    fire_escrow_claim(
        &state,
        &payment_scheme,
        &escrow_service_id,
        &escrow_agent_pubkey,
        escrow_deposited_amount,
        estimated_atomic_cost(&state.model_registry, &req.model, &req),
    );

    // Wrap with adaptive heartbeat to prevent proxy/client timeouts
    let heartbeat_stream = HeartbeatStream::new(stream, HeartbeatConfig::default());

    let sse_stream = heartbeat_stream.map(|item| match item {
        HeartbeatItem::Chunk(Ok(chunk)) => {
            let json = serde_json::to_string(&chunk).unwrap_or_default();
            Ok::<_, Infallible>(sse::Event::default().data(json))
        }
        HeartbeatItem::Chunk(Err(e)) => {
            warn!(error = %e, "stream chunk error");
            Ok(sse::Event::default().data(format!("{{\"error\": \"{e}\"}}")))
        }
        HeartbeatItem::KeepAlive => {
            Ok(sse::Event::default().comment("keep-alive"))
        }
    });
    return Ok(sse::Sse::new(sse_stream).into_response());
}
```

**Step 4: Run all gateway tests to verify nothing broke**

```bash
cargo test -p gateway
```

Expected: All existing tests still pass.

**Step 5: Commit**

```bash
git add crates/gateway/src/routes/chat.rs
git commit -m "feat: integrate adaptive heartbeat into SSE streaming path

Wraps provider streams with HeartbeatStream so SSE clients receive
': keep-alive' comments during model inference silence. Adaptive
timing: 5s initial, accelerates to 2s after 10s silence."
```

---

## Task 3: Per-Model Health Tracking

**Files:**
- Modify: `crates/gateway/src/providers/health.rs`

**Step 1: Write the failing tests**

Add these tests to the existing `#[cfg(test)] mod tests` block in `health.rs`:

```rust
// =========================================================================
// Per-model circuit breaker tests
// =========================================================================

#[tokio::test]
async fn test_model_circuit_starts_closed() {
    let tracker = ProviderHealthTracker::new(test_config());
    assert_eq!(
        tracker.get_model_state("anthropic", "claude-opus-4.6").await,
        CircuitState::Closed
    );
    assert!(tracker.is_model_available("anthropic", "claude-opus-4.6").await);
}

#[tokio::test]
async fn test_model_circuit_opens_independently() {
    let tracker = ProviderHealthTracker::new(test_config());

    // Fail opus 5 times
    for _ in 0..5 {
        tracker.record_model_failure("anthropic", "claude-opus-4.6", 500).await;
    }

    // Opus should be open, but Sonnet should still be closed
    assert_eq!(
        tracker.get_model_state("anthropic", "claude-opus-4.6").await,
        CircuitState::Open
    );
    assert_eq!(
        tracker.get_model_state("anthropic", "claude-sonnet-4.6").await,
        CircuitState::Closed
    );

    // Provider-level should still be available (only one model failed)
    assert!(tracker.is_available("anthropic").await);
}

#[tokio::test]
async fn test_model_success_closes_half_open_circuit() {
    let config = CircuitBreakerConfig {
        failure_threshold: 0.5,
        cooldown: Duration::from_millis(20),
        window: Duration::from_secs(60),
        min_requests: 5,
    };
    let tracker = ProviderHealthTracker::new(config);

    // Open the model circuit
    for _ in 0..5 {
        tracker.record_model_failure("openai", "gpt-4o", 500).await;
    }
    assert_eq!(
        tracker.get_model_state("openai", "gpt-4o").await,
        CircuitState::Open
    );

    // Wait for cooldown
    tokio::time::sleep(Duration::from_millis(30)).await;

    // Should transition to half-open
    assert!(tracker.is_model_available("openai", "gpt-4o").await);
    assert_eq!(
        tracker.get_model_state("openai", "gpt-4o").await,
        CircuitState::HalfOpen
    );

    // Probe success closes it
    tracker.record_model_success("openai", "gpt-4o", 100).await;
    assert_eq!(
        tracker.get_model_state("openai", "gpt-4o").await,
        CircuitState::Closed
    );
}

#[tokio::test]
async fn test_model_failure_also_records_provider_level() {
    let tracker = ProviderHealthTracker::new(test_config());

    // Record model failures — should also count as provider failures
    for _ in 0..5 {
        tracker.record_model_failure("openai", "gpt-4o", 500).await;
    }

    // Model should be open
    assert_eq!(
        tracker.get_model_state("openai", "gpt-4o").await,
        CircuitState::Open
    );

    // Provider should also accumulate the failures
    let rate = tracker.get_failure_rate("openai").await;
    assert!(rate > 0.0, "provider failure rate should reflect model failures");
}
```

**Step 2: Run tests to verify they fail**

```bash
cargo test -p gateway health::tests::test_model -- --nocapture
```

Expected: FAIL — methods `get_model_state`, `is_model_available`, `record_model_failure`, `record_model_success` don't exist.

**Step 3: Implement per-model tracking**

Add a parallel `models` map to `ProviderHealthTracker` and new methods. The key insight: model health uses the same `ProviderHealth` struct and `CircuitBreakerConfig`, just keyed by `"{provider}:{model}"` instead of just `"{provider}"`.

In `health.rs`, modify `ProviderHealthTracker`:

```rust
/// Tracks health and manages circuit breakers for all providers and models.
#[derive(Clone)]
pub struct ProviderHealthTracker {
    providers: Arc<RwLock<HashMap<String, ProviderHealth>>>,
    models: Arc<RwLock<HashMap<String, ProviderHealth>>>,
    config: CircuitBreakerConfig,
}

impl ProviderHealthTracker {
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            providers: Arc::new(RwLock::new(HashMap::new())),
            models: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }
```

Add these methods after the existing provider-level methods:

```rust
    // ----- Per-model methods -----

    fn model_key(provider: &str, model: &str) -> String {
        format!("{provider}:{model}")
    }

    /// Record a successful request to a specific model.
    /// Also records at the provider level.
    pub async fn record_model_success(&self, provider: &str, model: &str, latency_ms: u64) {
        // Record at model level
        let key = Self::model_key(provider, model);
        {
            let mut models = self.models.write().await;
            let health = models.entry(key).or_insert_with(ProviderHealth::new);
            let now = Instant::now();
            health.outcomes.push((now, true, latency_ms));
            health.ewma_latency_ms = health.ewma_alpha * latency_ms as f64
                + (1.0 - health.ewma_alpha) * health.ewma_latency_ms;

            if health.state == CircuitState::HalfOpen {
                info!(provider, model, "model circuit breaker: half-open → closed (probe succeeded)");
                health.state = CircuitState::Closed;
                health.opened_at = None;
            }
            self.cleanup_old_outcomes(health);
        }
        // Also record at provider level
        self.record_success(provider, latency_ms).await;
    }

    /// Record a failed request to a specific model.
    /// Also records at the provider level.
    pub async fn record_model_failure(&self, provider: &str, model: &str, latency_ms: u64) {
        let key = Self::model_key(provider, model);
        {
            let mut models = self.models.write().await;
            let health = models.entry(key).or_insert_with(ProviderHealth::new);
            let now = Instant::now();
            health.outcomes.push((now, false, latency_ms));
            health.ewma_latency_ms = health.ewma_alpha * latency_ms as f64
                + (1.0 - health.ewma_alpha) * health.ewma_latency_ms;

            if health.state == CircuitState::HalfOpen {
                info!(provider, model, "model circuit breaker: half-open → open (probe failed)");
                health.state = CircuitState::Open;
                health.opened_at = Some(now);
            } else {
                self.cleanup_old_outcomes(health);
                self.evaluate_model_circuit(provider, model, health);
            }
        }
        // Also record at provider level
        self.record_failure(provider, latency_ms).await;
    }

    /// Check if a specific model is available (circuit is not open).
    pub async fn is_model_available(&self, provider: &str, model: &str) -> bool {
        let key = Self::model_key(provider, model);
        let mut models = self.models.write().await;
        let health = match models.get_mut(&key) {
            Some(h) => h,
            None => return true, // Unknown model is assumed available
        };

        match health.state {
            CircuitState::Closed => true,
            CircuitState::HalfOpen => true,
            CircuitState::Open => {
                if let Some(opened_at) = health.opened_at {
                    if opened_at.elapsed() >= self.config.cooldown {
                        info!(
                            provider, model,
                            "model circuit breaker: open → half-open (cooldown elapsed)"
                        );
                        health.state = CircuitState::HalfOpen;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
        }
    }

    /// Get the current circuit state for a specific model.
    pub async fn get_model_state(&self, provider: &str, model: &str) -> CircuitState {
        let key = Self::model_key(provider, model);
        let models = self.models.read().await;
        models
            .get(&key)
            .map(|h| h.state)
            .unwrap_or(CircuitState::Closed)
    }

    fn evaluate_model_circuit(&self, provider: &str, model: &str, health: &mut ProviderHealth) {
        let now = Instant::now();
        let window_start = now - self.config.window;
        let recent: Vec<_> = health
            .outcomes
            .iter()
            .filter(|(t, _, _)| *t >= window_start)
            .collect();

        if (recent.len() as u32) < self.config.min_requests {
            return;
        }

        let failures = recent.iter().filter(|(_, success, _)| !success).count();
        let failure_rate = failures as f64 / recent.len() as f64;

        if failure_rate >= self.config.failure_threshold && health.state == CircuitState::Closed {
            warn!(
                provider, model,
                failure_rate = format!("{:.1}%", failure_rate * 100.0),
                "model circuit breaker: closed → open"
            );
            health.state = CircuitState::Open;
            health.opened_at = Some(now);
        }
    }
```

**Step 4: Run tests to verify they pass**

```bash
cargo test -p gateway health -- --nocapture
```

Expected: All health tests pass (both existing provider-level and new model-level).

**Step 5: Commit**

```bash
git add crates/gateway/src/providers/health.rs
git commit -m "feat: add per-model circuit breaker to ProviderHealthTracker

Extends the existing per-provider circuit breaker with model-level
granularity. Model failures cascade to the provider level. Model
circuits use the same config (50% threshold, 30s cooldown).
Per OpenRouter/LiteLLM patterns — model-specific outages are more
common than full provider outages."
```

---

## Task 4: Model-Level Fallback Chains

**Files:**
- Modify: `crates/gateway/src/providers/fallback.rs`

**Step 1: Write the failing tests**

Add these tests to the existing `#[cfg(test)] mod tests` block in `fallback.rs`:

```rust
#[test]
fn test_model_fallback_chain_opus() {
    let chain = model_fallback_chain("anthropic", "claude-opus-4.6");
    // Same-tier reasoning models across providers
    assert_eq!(chain[0], ("anthropic", "claude-opus-4.6"));
    assert!(chain.len() > 1, "should have fallback options");
    // Should include cross-provider equivalents
    let providers: Vec<&str> = chain.iter().map(|(p, _)| *p).collect();
    assert!(
        providers.iter().any(|p| *p != "anthropic"),
        "should include cross-provider fallbacks"
    );
}

#[test]
fn test_model_fallback_chain_gpt4o() {
    let chain = model_fallback_chain("openai", "gpt-4o");
    assert_eq!(chain[0], ("openai", "gpt-4o"));
    assert!(chain.len() > 1);
}

#[test]
fn test_model_fallback_chain_unknown_model() {
    let chain = model_fallback_chain("openai", "unknown-model");
    assert_eq!(chain.len(), 1);
    assert_eq!(chain[0], ("openai", "unknown-model"));
}

#[test]
fn test_model_fallback_chain_no_self_duplicates() {
    for (provider, model) in &[
        ("anthropic", "claude-opus-4.6"),
        ("openai", "gpt-4o"),
        ("openai", "gpt-4o-mini"),
        ("google", "gemini-3.1-pro"),
        ("deepseek", "deepseek-chat"),
    ] {
        let chain = model_fallback_chain(provider, model);
        let mut seen = std::collections::HashSet::new();
        for entry in &chain {
            assert!(
                seen.insert(entry.clone()),
                "duplicate ({:?}, {:?}) in fallback chain for ({provider}, {model})",
                entry.0, entry.1
            );
        }
    }
}
```

**Step 2: Run tests to verify they fail**

```bash
cargo test -p gateway fallback::tests::test_model_fallback -- --nocapture
```

Expected: FAIL — `model_fallback_chain` doesn't exist.

**Step 3: Implement model-level fallback chains**

Add this function to `fallback.rs` (after the existing `fallback_chain` function):

```rust
/// Get the ordered fallback list for a specific model.
///
/// Returns (provider, model_id) tuples. The primary model is always first.
/// Fallback models are same-capability-tier from different providers.
/// Falls back to `fallback_chain` provider-level matching for unknown models.
pub fn model_fallback_chain<'a>(provider: &'a str, model: &'a str) -> Vec<(&'a str, &'a str)> {
    let chain: Vec<(&str, &str)> = match (provider, model) {
        // --- Premium tier (reasoning, high capability) ---
        ("anthropic", "claude-opus-4.6") => vec![
            ("anthropic", "claude-opus-4.6"),
            ("openai", "gpt-5.2"),
            ("google", "gemini-3.1-pro"),
            ("openai", "o3"),
        ],
        ("openai", "gpt-5.2") => vec![
            ("openai", "gpt-5.2"),
            ("anthropic", "claude-opus-4.6"),
            ("google", "gemini-3.1-pro"),
        ],
        ("google", "gemini-3.1-pro") => vec![
            ("google", "gemini-3.1-pro"),
            ("anthropic", "claude-opus-4.6"),
            ("openai", "gpt-5.2"),
        ],

        // --- Mid tier (strong general purpose) ---
        ("anthropic", "claude-sonnet-4.6") => vec![
            ("anthropic", "claude-sonnet-4.6"),
            ("openai", "gpt-4.1"),
            ("google", "gemini-3.1-pro"),
            ("xai", "grok-3"),
        ],
        ("anthropic", "claude-sonnet-4.5") => vec![
            ("anthropic", "claude-sonnet-4.5"),
            ("openai", "gpt-4.1"),
            ("xai", "grok-3"),
        ],
        ("openai", "gpt-4o") => vec![
            ("openai", "gpt-4o"),
            ("anthropic", "claude-sonnet-4.6"),
            ("google", "gemini-3.1-pro"),
            ("xai", "grok-3"),
        ],
        ("openai", "gpt-4.1") => vec![
            ("openai", "gpt-4.1"),
            ("anthropic", "claude-sonnet-4.6"),
            ("google", "gemini-3.1-pro"),
        ],
        ("xai", "grok-3") => vec![
            ("xai", "grok-3"),
            ("anthropic", "claude-sonnet-4.6"),
            ("openai", "gpt-4o"),
        ],

        // --- Budget tier (fast, cheap) ---
        ("openai", "gpt-4o-mini") => vec![
            ("openai", "gpt-4o-mini"),
            ("anthropic", "claude-haiku-4.5"),
            ("google", "gemini-2.5-flash"),
            ("deepseek", "deepseek-chat"),
        ],
        ("openai", "gpt-4.1-mini") => vec![
            ("openai", "gpt-4.1-mini"),
            ("anthropic", "claude-haiku-4.5"),
            ("google", "gemini-2.5-flash"),
        ],
        ("openai", "gpt-4.1-nano") => vec![
            ("openai", "gpt-4.1-nano"),
            ("google", "gemini-2.5-flash-lite"),
            ("google", "gemini-2.0-flash-lite"),
        ],
        ("anthropic", "claude-haiku-4.5") => vec![
            ("anthropic", "claude-haiku-4.5"),
            ("openai", "gpt-4o-mini"),
            ("google", "gemini-2.5-flash"),
            ("deepseek", "deepseek-chat"),
        ],
        ("google", "gemini-2.5-flash") => vec![
            ("google", "gemini-2.5-flash"),
            ("openai", "gpt-4o-mini"),
            ("anthropic", "claude-haiku-4.5"),
            ("deepseek", "deepseek-chat"),
        ],
        ("deepseek", "deepseek-chat") => vec![
            ("deepseek", "deepseek-chat"),
            ("openai", "gpt-4o-mini"),
            ("google", "gemini-2.5-flash"),
        ],

        // --- Reasoning tier ---
        ("openai", "o3") => vec![
            ("openai", "o3"),
            ("anthropic", "claude-opus-4.6"),
            ("deepseek", "deepseek-reasoner"),
        ],
        ("openai", "o3-mini") | ("openai", "o4-mini") => vec![
            (provider, model),
            ("deepseek", "deepseek-reasoner"),
            ("xai", "grok-3-mini"),
        ],
        ("deepseek", "deepseek-reasoner") => vec![
            ("deepseek", "deepseek-reasoner"),
            ("openai", "o3-mini"),
            ("xai", "grok-3-mini"),
        ],

        // --- Unknown model: just return itself ---
        _ => vec![(provider, model)],
    };
    chain
}
```

**Step 4: Run tests to verify they pass**

```bash
cargo test -p gateway fallback -- --nocapture
```

Expected: All fallback tests pass (existing + new).

**Step 5: Commit**

```bash
git add crates/gateway/src/providers/fallback.rs
git commit -m "feat: add model-level fallback chains for cross-provider failover

Maps each model to same-capability-tier alternatives across providers.
Premium tier: Opus ↔ GPT-5.2 ↔ Gemini Pro. Mid tier: Sonnet ↔ GPT-4o ↔
Grok 3. Budget tier: Haiku ↔ GPT-4o-mini ↔ Flash ↔ DeepSeek."
```

---

## Task 5: Model-Aware Fallback Execution

**Files:**
- Modify: `crates/gateway/src/providers/fallback.rs`

**Step 1: Write the failing tests**

Add to the `tests` module in `fallback.rs`:

```rust
// These tests verify the model-aware fallback functions exist and compile.
// Full integration tests with mock providers are in Task 7.

#[test]
fn test_fallback_result_type_exists() {
    // Verify the FallbackResult type is accessible
    let result: FallbackResult<String> = FallbackResult {
        data: "test".to_string(),
        original_model: "gpt-4o".to_string(),
        actual_model: "gpt-4o".to_string(),
        actual_provider: "openai".to_string(),
        was_fallback: false,
    };
    assert!(!result.was_fallback);
    assert_eq!(result.original_model, result.actual_model);
}

#[test]
fn test_fallback_result_indicates_fallback() {
    let result: FallbackResult<String> = FallbackResult {
        data: "test".to_string(),
        original_model: "claude-opus-4.6".to_string(),
        actual_model: "gpt-5.2".to_string(),
        actual_provider: "openai".to_string(),
        was_fallback: true,
    };
    assert!(result.was_fallback);
    assert_ne!(result.original_model, result.actual_model);
}
```

**Step 2: Run tests to verify they fail**

```bash
cargo test -p gateway test_fallback_result -- --nocapture
```

Expected: FAIL — `FallbackResult` doesn't exist.

**Step 3: Implement FallbackResult and model-aware fallback functions**

Add at the top of `fallback.rs` (after imports):

```rust
/// Result from a fallback-aware request. Tracks whether the response
/// came from the originally requested model or a fallback.
#[derive(Debug)]
pub struct FallbackResult<T> {
    pub data: T,
    pub original_model: String,
    pub actual_model: String,
    pub actual_provider: String,
    pub was_fallback: bool,
}
```

Add these new model-aware functions (keep the existing provider-level ones for backwards compatibility):

```rust
/// Execute a chat completion with model-level fallback.
///
/// First checks if the requested model's circuit is open. If so, tries
/// same-tier models from other providers. Returns which model actually served.
pub async fn chat_with_model_fallback(
    providers: &ProviderRegistry,
    health: &ProviderHealthTracker,
    original_provider: &str,
    original_model: &str,
    req: ChatRequest,
) -> Result<FallbackResult<ChatResponse>, ProviderError> {
    let chain = model_fallback_chain(original_provider, original_model);
    let mut last_error: Option<ProviderError> = None;

    for (prov, model_id) in &chain {
        let provider = match providers.get(prov) {
            Some(p) => p,
            None => continue,
        };

        // Check model-level circuit first, then provider-level
        if !health.is_model_available(prov, model_id).await {
            info!(provider = %prov, model = %model_id, "skipping model (circuit open)");
            continue;
        }
        if !health.is_available(prov).await {
            info!(provider = %prov, "skipping provider (circuit open)");
            continue;
        }

        let mut model_req = req.clone();
        model_req.model = model_id.to_string();

        let start = Instant::now();
        match provider.chat_completion(model_req).await {
            Ok(response) => {
                let latency_ms = start.elapsed().as_millis() as u64;
                health.record_model_success(prov, model_id, latency_ms).await;
                let was_fallback = *prov != original_provider || *model_id != original_model;
                if was_fallback {
                    info!(
                        original_provider, original_model,
                        fallback_provider = %prov, fallback_model = %model_id,
                        latency_ms,
                        "served from fallback model"
                    );
                }
                return Ok(FallbackResult {
                    data: response,
                    original_model: original_model.to_string(),
                    actual_model: model_id.to_string(),
                    actual_provider: prov.to_string(),
                    was_fallback,
                });
            }
            Err(e) => {
                let latency_ms = start.elapsed().as_millis() as u64;
                health.record_model_failure(prov, model_id, latency_ms).await;
                warn!(
                    provider = %prov, model = %model_id,
                    error = %e, latency_ms,
                    "model request failed, trying next in chain"
                );
                last_error = Some(e);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| "no available models in fallback chain".into()))
}

/// Execute a streaming chat completion with model-level fallback.
pub async fn stream_with_model_fallback(
    providers: &ProviderRegistry,
    health: &ProviderHealthTracker,
    original_provider: &str,
    original_model: &str,
    req: ChatRequest,
) -> Result<FallbackResult<ChatStream>, ProviderError> {
    let chain = model_fallback_chain(original_provider, original_model);
    let mut last_error: Option<ProviderError> = None;

    for (prov, model_id) in &chain {
        let provider = match providers.get(prov) {
            Some(p) => p,
            None => continue,
        };

        if !health.is_model_available(prov, model_id).await {
            info!(provider = %prov, model = %model_id, "skipping model for streaming (circuit open)");
            continue;
        }
        if !health.is_available(prov).await {
            info!(provider = %prov, "skipping provider for streaming (circuit open)");
            continue;
        }

        let mut model_req = req.clone();
        model_req.model = model_id.to_string();

        let start = Instant::now();
        match provider.chat_completion_stream(model_req).await {
            Ok(stream) => {
                let latency_ms = start.elapsed().as_millis() as u64;
                health.record_model_success(prov, model_id, latency_ms).await;
                let was_fallback = *prov != original_provider || *model_id != original_model;
                if was_fallback {
                    info!(
                        original_provider, original_model,
                        fallback_provider = %prov, fallback_model = %model_id,
                        "streaming from fallback model"
                    );
                }
                return Ok(FallbackResult {
                    data: stream,
                    original_model: original_model.to_string(),
                    actual_model: model_id.to_string(),
                    actual_provider: prov.to_string(),
                    was_fallback,
                });
            }
            Err(e) => {
                let latency_ms = start.elapsed().as_millis() as u64;
                health.record_model_failure(prov, model_id, latency_ms).await;
                warn!(provider = %prov, model = %model_id, error = %e, "streaming model fallback, trying next");
                last_error = Some(e);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| "no available models for streaming in fallback chain".into()))
}
```

**Step 4: Run tests to verify they pass**

```bash
cargo test -p gateway fallback -- --nocapture
```

Expected: All tests pass.

**Step 5: Commit**

```bash
git add crates/gateway/src/providers/fallback.rs
git commit -m "feat: add model-aware fallback execution with FallbackResult

chat_with_model_fallback and stream_with_model_fallback try
same-tier models from other providers when the requested model's
circuit is open. FallbackResult tracks original vs actual model
for transparent X-RCR-Fallback header support."
```

---

## Task 6: Wire Model-Aware Fallback into Chat Handler + X-RCR-Fallback Header

**Files:**
- Modify: `crates/gateway/src/routes/chat.rs`

**Step 1: Write the failing test**

Add to the `tests` module in `chat.rs`:

```rust
#[test]
fn test_fallback_header_name_is_valid() {
    // Verify the header name we'll use is a valid HTTP header
    use axum::http::HeaderName;
    let name = HeaderName::from_static("x-rcr-fallback");
    assert_eq!(name.as_str(), "x-rcr-fallback");
}
```

**Step 2: Run test**

```bash
cargo test -p gateway test_fallback_header_name -- --exact
```

Expected: PASS (just validates the header name is valid).

**Step 3: Update chat_completions to use model-aware fallback**

Replace the import of `fallback` at the top of `chat.rs`:
```rust
use crate::providers::fallback;
```
Keep it, and add:
```rust
use crate::providers::fallback::FallbackResult;
```

Then modify the non-streaming path (replace the `chat_with_fallback` call and its `Ok(response)` arm):

```rust
    } else {
        // Non-streaming JSON response with model-aware fallback
        info!(provider = provider_name, model = %req.model, "proxying to provider (with model fallback)");

        match fallback::chat_with_model_fallback(
            &state.providers,
            &state.provider_health,
            provider_name,
            &req.model,
            req.clone(),
        )
        .await
        {
            Ok(result) => {
                // Cache the response (async, non-blocking)
                if let Some(cache) = &state.cache {
                    cache.set(&req, &result.data).await;
                }

                // Log spend asynchronously
                if let Some(usage) = &result.data.usage {
                    let cost = state
                        .model_registry
                        .estimate_cost(&req.model, usage.prompt_tokens, usage.completion_tokens)
                        .map(|c| c.total.parse::<f64>().unwrap_or(0.0))
                        .unwrap_or(0.0);

                    state.usage.log_spend(
                        wallet_address.clone(),
                        req.model.clone(),
                        result.actual_provider.clone(),
                        usage.prompt_tokens,
                        usage.completion_tokens,
                        cost,
                        tx_signature.clone(),
                    );
                }

                // Fire escrow claim if this was an escrow payment
                let claim_atomic = if let Some(usage) = &result.data.usage {
                    compute_actual_atomic_cost(
                        usage.prompt_tokens,
                        usage.completion_tokens,
                        model_info,
                    )
                } else {
                    estimated_atomic_cost(&state.model_registry, &req.model, &req)
                };
                fire_escrow_claim(
                    &state,
                    &payment_scheme,
                    &escrow_service_id,
                    &escrow_agent_pubkey,
                    escrow_deposited_amount,
                    claim_atomic,
                );

                let response_json = serde_json::to_value(&result.data)
                    .map_err(|e| GatewayError::Internal(e.to_string()))?;

                let mut resp = Json(response_json).into_response();

                // Add fallback header if served by a different model
                if result.was_fallback {
                    let fallback_value = format!(
                        "{} -> {}",
                        result.original_model, result.actual_model
                    );
                    if let Ok(hv) = HeaderValue::from_str(&fallback_value) {
                        resp.headers_mut()
                            .insert(HeaderName::from_static("x-rcr-fallback"), hv);
                    }
                }

                // Issue a session token after successful paid response
                if let Some(token) = build_session_token(&wallet_address, &state.session_secret) {
                    if let Ok(hv) = HeaderValue::from_str(&token) {
                        resp.headers_mut()
                            .insert(HeaderName::from_static("x-rcr-session"), hv);
                    }
                }
                return Ok(resp);
            }
            Err(_) => {
                // All models failed — fall through to stub
            }
        }
    }
```

Similarly update the streaming path to use `stream_with_model_fallback`:

```rust
    if req.stream {
        info!(provider = provider_name, model = %req.model, "streaming to provider (with model fallback)");

        match fallback::stream_with_model_fallback(
            &state.providers,
            &state.provider_health,
            provider_name,
            &req.model,
            req.clone(),
        )
        .await
        {
            Ok(result) => {
                fire_escrow_claim(
                    &state,
                    &payment_scheme,
                    &escrow_service_id,
                    &escrow_agent_pubkey,
                    escrow_deposited_amount,
                    estimated_atomic_cost(&state.model_registry, &req.model, &req),
                );

                // Wrap with adaptive heartbeat
                let heartbeat_stream = HeartbeatStream::new(result.data, HeartbeatConfig::default());

                let sse_stream = heartbeat_stream.map(|item| match item {
                    HeartbeatItem::Chunk(Ok(chunk)) => {
                        let json = serde_json::to_string(&chunk).unwrap_or_default();
                        Ok::<_, Infallible>(sse::Event::default().data(json))
                    }
                    HeartbeatItem::Chunk(Err(e)) => {
                        warn!(error = %e, "stream chunk error");
                        Ok(sse::Event::default().data(format!("{{\"error\": \"{e}\"}}")))
                    }
                    HeartbeatItem::KeepAlive => {
                        Ok(sse::Event::default().comment("keep-alive"))
                    }
                });

                let mut resp = sse::Sse::new(sse_stream).into_response();

                // Add fallback header if served by a different model
                if result.was_fallback {
                    let fallback_value = format!(
                        "{} -> {}",
                        result.original_model, result.actual_model
                    );
                    if let Ok(hv) = HeaderValue::from_str(&fallback_value) {
                        resp.headers_mut()
                            .insert(HeaderName::from_static("x-rcr-fallback"), hv);
                    }
                }

                return Ok(resp);
            }
            Err(_) => {
                // All models failed — fall through to stub
            }
        }
```

Remove the old `fallback::chat_with_fallback` and `fallback::stream_with_fallback` calls entirely (they are replaced by the model-aware versions above).

**Step 4: Run all gateway tests**

```bash
cargo test -p gateway
```

Expected: All tests pass. The integration tests use `fallback_chain` internally via the old path, which still compiles but the handler now uses the model-aware path.

**Step 5: Commit**

```bash
git add crates/gateway/src/routes/chat.rs
git commit -m "feat: wire model-aware fallback into chat handler with X-RCR-Fallback header

Replaces provider-level fallback with model-level fallback chains.
When a model's circuit is open, tries same-tier models from other
providers. Adds X-RCR-Fallback header (e.g., 'claude-opus-4.6 -> gpt-5.2')
when serving from a different model — transparent to clients."
```

---

## Task 7: Integration Tests for Model Failover and Heartbeat

**Files:**
- Modify: `crates/gateway/tests/integration.rs`

**Step 1: Write the integration tests**

Add these tests to `integration.rs`. They verify end-to-end behavior using the existing `test_app()` pattern.

```rust
// =========================================================================
// Model failover — X-RCR-Fallback header
// =========================================================================

/// Verify that when the primary model's circuit is open, the gateway
/// falls through to the stub (since no providers are configured in tests)
/// and does not panic or error.
#[tokio::test]
async fn test_chat_with_broken_circuit_returns_stub() {
    let (app, state) = test_app();

    // Open the circuit for the requested model
    for _ in 0..5 {
        state
            .provider_health
            .record_model_failure("openai", "gpt-4o", 500)
            .await;
    }

    let body = serde_json::json!({
        "model": "openai-gpt-4o",
        "messages": [{"role": "user", "content": "hello"}],
    });

    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("content-type", "application/json")
        .header("payment-signature", &make_valid_payment_header())
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    // Should get a response (either stub or fallback), not an error
    assert!(
        resp.status() == StatusCode::OK || resp.status() == StatusCode::PAYMENT_REQUIRED,
        "expected 200 or 402, got {}",
        resp.status()
    );
}

/// Verify the heartbeat sentinel constant is accessible from integration tests.
#[test]
fn test_heartbeat_module_accessible() {
    assert_eq!(
        gateway::providers::heartbeat::HEARTBEAT_SENTINEL,
        "__heartbeat__"
    );
}

/// Verify circuit breaker state is queryable.
#[tokio::test]
async fn test_circuit_breaker_model_state_queryable() {
    let (_app, state) = test_app();

    // Initially closed
    assert_eq!(
        state.provider_health.get_model_state("openai", "gpt-4o").await,
        gateway::providers::health::CircuitState::Closed
    );

    // Record failures to open it
    for _ in 0..5 {
        state
            .provider_health
            .record_model_failure("openai", "gpt-4o", 500)
            .await;
    }

    assert_eq!(
        state.provider_health.get_model_state("openai", "gpt-4o").await,
        gateway::providers::health::CircuitState::Open
    );

    // Other models unaffected
    assert_eq!(
        state.provider_health.get_model_state("openai", "gpt-4o-mini").await,
        gateway::providers::health::CircuitState::Closed
    );
}
```

You'll also need a helper `make_valid_payment_header()` if one doesn't exist. Check the existing integration tests — there's likely already a `make_payment_header` or similar helper. Use the same pattern.

**Step 2: Run integration tests**

```bash
cargo test -p gateway --test integration -- --nocapture
```

Expected: All tests pass.

**Step 3: Commit**

```bash
git add crates/gateway/tests/integration.rs
git commit -m "test: add integration tests for model-level circuit breaker and heartbeat

Verifies per-model circuit breaker state, model independence (Opus
broken doesn't affect Sonnet), and heartbeat module accessibility."
```

---

## Task 8: Agent Override via X-RCR-Fallback-Preference Header

**Files:**
- Modify: `crates/gateway/src/routes/chat.rs`

**Step 1: Write the failing test**

Add to `chat.rs` tests:

```rust
#[test]
fn test_parse_fallback_preference_valid() {
    let prefs = parse_fallback_preference("openai/gpt-4.1,anthropic/claude-sonnet-4.6");
    assert_eq!(prefs.len(), 2);
    assert_eq!(prefs[0], ("openai", "gpt-4.1"));
    assert_eq!(prefs[1], ("anthropic", "claude-sonnet-4.6"));
}

#[test]
fn test_parse_fallback_preference_empty() {
    let prefs = parse_fallback_preference("");
    assert!(prefs.is_empty());
}

#[test]
fn test_parse_fallback_preference_invalid_entries_skipped() {
    let prefs = parse_fallback_preference("openai/gpt-4.1,invalid,anthropic/claude-sonnet-4.6");
    assert_eq!(prefs.len(), 2);
}

#[test]
fn test_parse_fallback_preference_whitespace_trimmed() {
    let prefs = parse_fallback_preference(" openai/gpt-4.1 , anthropic/claude-sonnet-4.6 ");
    assert_eq!(prefs.len(), 2);
    assert_eq!(prefs[0], ("openai", "gpt-4.1"));
}
```

**Step 2: Run tests to verify they fail**

```bash
cargo test -p gateway test_parse_fallback -- --nocapture
```

Expected: FAIL — `parse_fallback_preference` doesn't exist.

**Step 3: Implement the parser and header extraction**

Add to `chat.rs`:

```rust
/// Parse the X-RCR-Fallback-Preference header value.
///
/// Format: "provider/model,provider/model,..."
/// Returns (provider, model) tuples. Invalid entries are silently skipped.
fn parse_fallback_preference(header: &str) -> Vec<(&str, &str)> {
    header
        .split(',')
        .filter_map(|entry| {
            let trimmed = entry.trim();
            let (provider, model) = trimmed.split_once('/')?;
            let provider = provider.trim();
            let model = model.trim();
            if provider.is_empty() || model.is_empty() {
                None
            } else {
                Some((provider, model))
            }
        })
        .collect()
}
```

Then in `chat_completions`, after resolving the model and before the fallback call, add header extraction:

```rust
    // Check for agent-specified fallback preferences
    let fallback_pref = headers
        .get("x-rcr-fallback-preference")
        .and_then(|v| v.to_str().ok());
```

**Note:** For now, this header is parsed and available. The actual integration with the fallback chain (overriding `model_fallback_chain` with agent preferences) is a straightforward extension: if the header is present, use its entries as the fallback chain instead of the default. The implementer should add this logic in the streaming and non-streaming fallback call sites — converting the parsed preferences into the chain format and calling the model-aware fallback with a custom chain.

This is left as a simple if-else: if `fallback_pref` is `Some`, build a custom chain from parsed preferences (prepend the original model); otherwise use the default `model_fallback_chain`.

To support this, add a variant of `chat_with_model_fallback` that accepts an explicit chain, or simply inline the chain construction in `chat.rs`:

```rust
    // Build the model fallback chain — agent override or default
    let model_chain: Vec<(String, String)> = if let Some(pref) = fallback_pref {
        let mut chain = vec![(provider_name.to_string(), req.model.clone())];
        for (p, m) in parse_fallback_preference(pref) {
            let entry = (p.to_string(), m.to_string());
            if !chain.contains(&entry) {
                chain.push(entry);
            }
        }
        chain
    } else {
        fallback::model_fallback_chain(provider_name, &req.model)
            .into_iter()
            .map(|(p, m)| (p.to_string(), m.to_string()))
            .collect()
    };
```

Then pass this chain to the fallback functions. This requires adding a `chat_with_model_chain` function to `fallback.rs` that takes `&[(String, String)]` instead of calling `model_fallback_chain` internally. OR you can modify `chat_with_model_fallback` to accept an optional override chain parameter.

The simplest approach: add a `chat_with_chain` function:

```rust
/// Execute a chat completion using an explicit model chain.
pub async fn chat_with_chain(
    providers: &ProviderRegistry,
    health: &ProviderHealthTracker,
    chain: &[(String, String)],
    original_model: &str,
    req: ChatRequest,
) -> Result<FallbackResult<ChatResponse>, ProviderError> {
    let mut last_error: Option<ProviderError> = None;

    for (prov, model_id) in chain {
        let provider = match providers.get(prov) {
            Some(p) => p,
            None => continue,
        };

        if !health.is_model_available(prov, model_id).await {
            info!(provider = %prov, model = %model_id, "skipping model (circuit open)");
            continue;
        }
        if !health.is_available(prov).await {
            info!(provider = %prov, "skipping provider (circuit open)");
            continue;
        }

        let mut model_req = req.clone();
        model_req.model = model_id.to_string();

        let start = Instant::now();
        match provider.chat_completion(model_req).await {
            Ok(response) => {
                let latency_ms = start.elapsed().as_millis() as u64;
                health.record_model_success(prov, model_id, latency_ms).await;
                let was_fallback = model_id != original_model;
                if was_fallback {
                    info!(
                        original_model,
                        fallback_provider = %prov, fallback_model = %model_id,
                        latency_ms,
                        "served from fallback model"
                    );
                }
                return Ok(FallbackResult {
                    data: response,
                    original_model: original_model.to_string(),
                    actual_model: model_id.to_string(),
                    actual_provider: prov.to_string(),
                    was_fallback,
                });
            }
            Err(e) => {
                let latency_ms = start.elapsed().as_millis() as u64;
                health.record_model_failure(prov, model_id, latency_ms).await;
                warn!(
                    provider = %prov, model = %model_id,
                    error = %e, latency_ms,
                    "model request failed, trying next in chain"
                );
                last_error = Some(e);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| "no available models in chain".into()))
}
```

Add the equivalent `stream_with_chain` function following the same pattern.

Then update `chat_completions` to use `chat_with_chain`/`stream_with_chain` with the constructed `model_chain`.

**Step 4: Run all tests**

```bash
cargo test -p gateway
```

Expected: All tests pass.

**Step 5: Commit**

```bash
git add crates/gateway/src/routes/chat.rs crates/gateway/src/providers/fallback.rs
git commit -m "feat: support agent-specified fallback preferences via X-RCR-Fallback-Preference header

Agents can send 'X-RCR-Fallback-Preference: openai/gpt-4.1,google/gemini-3.1-pro'
to override default fallback chains. Original model is always tried first."
```

---

## Task 9: Final Cleanup and Full Test Run

**Files:**
- Verify: all modified files compile and tests pass

**Step 1: Run full workspace lint**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings
```

Fix any warnings or formatting issues.

**Step 2: Run full test suite**

```bash
cargo test
```

Expected: All workspace tests pass (gateway, x402, router, rcr-common).

**Step 3: Run escrow tests separately**

```bash
cargo test --manifest-path programs/escrow/Cargo.toml
```

**Step 4: Commit any final fixes**

```bash
git add -A
git commit -m "chore: lint and format cleanup for heartbeat + model failover"
```

---

## Summary of Changes

| File | Change |
|------|--------|
| `crates/gateway/src/providers/heartbeat.rs` | **NEW** — Adaptive SSE heartbeat stream wrapper |
| `crates/gateway/src/providers/health.rs` | Extended with per-model circuit breaker |
| `crates/gateway/src/providers/fallback.rs` | Model-level fallback chains + `FallbackResult` + chain-based execution |
| `crates/gateway/src/providers/mod.rs` | Added `pub mod heartbeat` |
| `crates/gateway/src/routes/chat.rs` | Heartbeat integration, model-aware fallback, X-RCR-Fallback header, agent override |
| `crates/gateway/tests/integration.rs` | Model circuit breaker and heartbeat integration tests |
| `crates/gateway/Cargo.toml` | Added `pin-project-lite`, `tokio-stream` (dev) |

## Future Enhancements (Not in Scope)

- **Redis-backed circuit breaker** for multi-instance deployments
- **Configurable fallback chains** via `config/models.toml` (instead of hardcoded)
- **Cost-aware fallback** that considers pricing differences when falling back
- **Streaming health tracking** that detects mid-stream failures (timeout after first token)
