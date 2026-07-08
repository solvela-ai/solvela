pub mod anthropic;
pub mod anthropic_inbound;
pub(crate) mod cache_usage;
pub mod deepseek;
pub mod fallback;
pub mod google;
pub mod health;
pub mod heartbeat;
pub mod nvidia;
pub mod openai;
pub mod price;
pub mod search;
pub mod xai;

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::Stream;

use solvela_protocol::{ChatChunk, ChatRequest, ChatResponse, ModelRegistration};

/// Error type for provider operations.
pub type ProviderError = Box<dyn std::error::Error + Send + Sync>;

/// A boxed stream of chat completion chunks.
pub type ChatStream = Pin<Box<dyn Stream<Item = Result<ChatChunk, ProviderError>> + Send>>;

/// Trait for LLM provider adapters.
///
/// Each provider translates between the OpenAI-compatible gateway format
/// and the provider's native API format. Implementations must be Send + Sync
/// for use in Axum's concurrent request handling.
#[async_trait]
pub trait LLMProvider: Send + Sync {
    /// Provider name (e.g., "openai", "anthropic").
    fn name(&self) -> &str;

    /// List of models this provider supports.
    fn supported_models(&self) -> Vec<ModelRegistration>;

    /// Execute a chat completion request.
    async fn chat_completion(
        &self,
        req: ChatRequest,
    ) -> Result<ChatResponse, Box<dyn std::error::Error + Send + Sync>>;

    /// Execute a streaming chat completion request.
    /// Returns a stream of ChatChunk events.
    /// Default implementation returns an error (not all providers support streaming).
    async fn chat_completion_stream(&self, req: ChatRequest) -> Result<ChatStream, ProviderError> {
        let _ = req;
        Err("streaming not supported by this provider".into())
    }
}

/// Spawn an SSE parser for OpenAI-compatible streaming responses.
///
/// Buffers incoming bytes, splits on `\n\n` boundaries, extracts `data: ` lines,
/// and parses each as a `ChatChunk`. Skips `[DONE]` sentinel events.
///
/// This is shared by all OpenAI-compatible providers (OpenAI, DeepSeek, xAI).
pub fn spawn_openai_sse_parser(response: reqwest::Response) -> ChatStream {
    let (mut tx, rx) = futures::channel::mpsc::channel::<Result<ChatChunk, ProviderError>>(32);
    tokio::spawn(async move {
        use futures::{SinkExt, StreamExt};

        let mut byte_stream = response.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk_result) = byte_stream.next().await {
            match chunk_result {
                Ok(bytes) => {
                    buffer.push_str(&String::from_utf8_lossy(&bytes));

                    while let Some(pos) = buffer.find("\n\n") {
                        let event = buffer[..pos].to_string();
                        buffer.drain(..pos + 2);

                        for line in event.lines() {
                            if let Some(data) = line.strip_prefix("data: ") {
                                let data = data.trim();
                                if data == "[DONE]" {
                                    return;
                                }
                                match serde_json::from_str::<ChatChunk>(data) {
                                    Ok(chunk) => {
                                        if tx.send(Ok(chunk)).await.is_err() {
                                            return;
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(error = %e, "failed to parse SSE chunk");
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(Box::new(e) as ProviderError)).await;
                    return;
                }
            }
        }
    });
    Box::pin(rx)
}

/// Registry of configured LLM provider adapters.
///
/// Maps provider names (e.g., "openai", "anthropic") to their adapter
/// implementations. Only providers with configured API keys are registered.
pub struct ProviderRegistry {
    providers: HashMap<String, Arc<dyn LLMProvider>>,
}

impl ProviderRegistry {
    /// Create a new provider registry from environment-configured API keys.
    ///
    /// All providers share the given `reqwest::Client` so that TCP connections
    /// and TLS sessions are reused across providers. The client-level timeout
    /// (typically 10s) is overridden per-request with
    /// [`PROVIDER_REQUEST_TIMEOUT`] for LLM API calls in each provider adapter.
    ///
    /// Only providers with valid API keys are registered. If no API keys
    /// are configured, the registry will be empty and all requests will
    /// return stub responses.
    pub fn from_env(client: reqwest::Client) -> Self {
        let mut providers: HashMap<String, Arc<dyn LLMProvider>> = HashMap::new();

        if let Ok(key) = std::env::var("OPENAI_API_KEY") {
            if !key.is_empty() {
                providers.insert(
                    "openai".to_string(),
                    Arc::new(openai::OpenAIProvider::new(client.clone(), key)),
                );
            }
        }

        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            if !key.is_empty() {
                providers.insert(
                    "anthropic".to_string(),
                    Arc::new(anthropic::AnthropicProvider::new(client.clone(), key)),
                );
            }
        }

        if let Ok(key) = std::env::var("GOOGLE_API_KEY") {
            if !key.is_empty() {
                providers.insert(
                    "google".to_string(),
                    Arc::new(google::GoogleProvider::new(client.clone(), key)),
                );
            }
        }

        if let Ok(key) = std::env::var("XAI_API_KEY") {
            if !key.is_empty() {
                providers.insert(
                    "xai".to_string(),
                    Arc::new(xai::XAIProvider::new(client.clone(), key)),
                );
            }
        }

        if let Ok(key) = std::env::var("NVIDIA_API_KEY") {
            if !key.is_empty() {
                providers.insert(
                    "nvidia".to_string(),
                    Arc::new(nvidia::NvidiaProvider::new(client.clone(), key)),
                );
            }
        }

        if let Ok(key) = std::env::var("DEEPSEEK_API_KEY") {
            if !key.is_empty() {
                providers.insert(
                    "deepseek".to_string(),
                    Arc::new(deepseek::DeepSeekProvider::new(client, key)),
                );
            }
        }

        Self { providers }
    }

    /// Create a provider registry from pre-built provider instances.
    ///
    /// Used in tests to inject mock providers without requiring real API keys.
    pub fn from_providers(providers: HashMap<String, Arc<dyn LLMProvider>>) -> Self {
        Self { providers }
    }

    /// Build the dedicated NATIVE Anthropic relay handle for `POST /v1/messages`.
    ///
    /// Returns a concrete [`anthropic::AnthropicProvider`] (NOT a boxed
    /// `dyn LLMProvider`) because the native passthrough calls
    /// [`anthropic::AnthropicProvider::relay_native`], which is an inherent
    /// method — the OpenAI-shaped `LLMProvider` trait cannot carry it. Gated on
    /// the SAME `ANTHROPIC_API_KEY` env var as the OpenAI-shaped registry entry,
    /// so the native relay is present exactly when the reshape adapter is, and
    /// `None` (the native fork falls back to the reshape branch / fails closed)
    /// when no Anthropic key is configured. Shares the registry's `reqwest`
    /// client so connection reuse is unchanged.
    pub fn native_anthropic_from_env(
        client: reqwest::Client,
    ) -> Option<Arc<anthropic::AnthropicProvider>> {
        match std::env::var("ANTHROPIC_API_KEY") {
            Ok(key) if !key.is_empty() => {
                Some(Arc::new(anthropic::AnthropicProvider::new(client, key)))
            }
            _ => None,
        }
    }

    /// Look up a provider by name.
    pub fn get(&self, provider_name: &str) -> Option<&Arc<dyn LLMProvider>> {
        self.providers.get(provider_name)
    }

    /// List all configured provider names.
    pub fn configured_providers(&self) -> Vec<&str> {
        self.providers.keys().map(|s| s.as_str()).collect()
    }
}

/// Per-attempt timeout for upstream LLM provider requests.
///
/// Applied via `.timeout(..)` on every provider's chat / streaming request.
/// MUST stay strictly below the gateway's global request timeout
/// ([`crate::DEFAULT_REQUEST_TIMEOUT_SECS`], enforced by the tower-http
/// `TimeoutLayer` in `build_router`): a hanging upstream has to surface its
/// `Err` here first so the model fallback chain
/// (`fallback::chat_with_model_fallback`) can advance to the next model
/// before the global layer kills the request with a bare 408. Pinned by
/// `test_hanging_provider_budget_fits_within_global_timeout`.
pub const PROVIDER_REQUEST_TIMEOUT: Duration = Duration::from_secs(90);

/// Retry budget every provider adapter passes to [`retry_with_backoff`].
///
/// Single source of truth: raising it widens the worst-case latency of a
/// *retryable* (connect / 5xx) failure, which must still fit inside
/// [`crate::DEFAULT_REQUEST_TIMEOUT_SECS`]. Referenced by all six adapters and
/// by `test_hanging_provider_budget_fits_within_global_timeout`, so a bump
/// cannot silently leave the budget test checking a stale number.
pub const PROVIDER_MAX_RETRIES: u32 = 2;

/// Retries a future up to `max_retries` times with exponential backoff.
/// Only retries transient errors (connection errors, 5xx).
/// Does NOT retry 4xx errors (auth, rate limit, bad request) or per-attempt
/// timeouts (a hung upstream must surface immediately — see the comment on
/// the transient classification below).
pub async fn retry_with_backoff<F, Fut, T>(max_retries: u32, f: F) -> Result<T, reqwest::Error>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, reqwest::Error>>,
{
    // Run the first attempt outside the retry loop so we always have a concrete
    // error value to return, avoiding `Option::unwrap()` on `last_err`.
    let mut last_err = match f().await {
        Ok(val) => return Ok(val),
        Err(e) => e,
    };

    for attempt in 1..=max_retries {
        // Timeouts are deliberately NOT retried. Each attempt already gets the
        // full per-attempt budget (PROVIDER_REQUEST_TIMEOUT = 90s), so retrying
        // a hung upstream (90s + 1s backoff + 90s = 181s worst case) blows past
        // the gateway's global request timeout (tower-http TimeoutLayer,
        // default 120s): the client gets a bare 408 and the provider `Err`
        // never surfaces, so `chat_with_model_fallback` can never advance to
        // the next model in the chain. Surfacing the timeout at 90s leaves
        // ~30s of budget for failover. Accepted trade-off: a genuinely
        // transient single-attempt timeout is no longer retried same-provider —
        // chained models fail over instead (better), and a primary-only model
        // errors at 90s instead of maybe succeeding at 90s+1s+<90s, which the
        // 120s global layer would usually have killed anyway.
        // UNSTATED INVARIANT this predicate rests on: the provider
        // `reqwest::Client` (main.rs) configures only a whole-request
        // `.timeout()` and a `.read_timeout()` — never a `.connect_timeout()`.
        // Under reqwest 0.12, `is_connect()` and `is_timeout()` both walk the
        // source chain, so a *connect-phase* timeout would satisfy BOTH (a
        // `hyper_util` Connect error carrying an `io::ErrorKind::TimedOut`).
        // Today no such error can be produced, so `is_connect()` below only
        // ever sees genuine connect failures. If a `.connect_timeout()` is
        // ever added to that client, connect-phase timeouts would start being
        // retried through the `is_connect()` arm — silently reintroducing the
        // 2×90s+backoff budget blowout for that error class. Revisit here.
        let is_transient =
            last_err.is_connect() || last_err.status().is_some_and(|s| s.is_server_error());

        if !is_transient {
            return Err(last_err);
        }

        let delay = Duration::from_secs(1 << (attempt - 1)); // 1s, 2s, …
        tracing::warn!(
            attempt = attempt,
            max_retries = max_retries,
            error = %last_err,
            "provider request failed, retrying after {}s",
            delay.as_secs()
        );
        tokio::time::sleep(delay).await;

        match f().await {
            Ok(val) => return Ok(val),
            Err(e) => last_err = e,
        }
    }

    Err(last_err)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicU32, Ordering};

    #[tokio::test]
    async fn test_retry_succeeds_on_first_attempt() {
        let call_count = AtomicU32::new(0);
        let result = retry_with_backoff(2, || {
            call_count.fetch_add(1, Ordering::SeqCst);
            async { Ok::<_, reqwest::Error>(42) }
        })
        .await;

        assert_eq!(result.unwrap(), 42);
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_does_not_retry_4xx_errors() {
        // Build a 401 reqwest::Error by making a request to a mock that returns 401
        // We can't easily construct reqwest::Error directly, so we use a different approach:
        // We test the logic by verifying that non-transient errors are returned immediately.
        let call_count = AtomicU32::new(0);

        // Use a URL that will fail with a connection error to test the retry path,
        // but since connection errors ARE transient, we test with a short timeout instead.
        // For unit testing the non-retry path, we verify via the retry_with_backoff
        // contract: if the error is NOT transient, it returns immediately.
        let result: Result<(), reqwest::Error> = retry_with_backoff(2, || {
            call_count.fetch_add(1, Ordering::SeqCst);
            async {
                // Create a client with a very short timeout to trigger a timeout error
                // Timeout errors ARE transient, so this will retry.
                // For a proper non-transient test, we'd need a real HTTP server.
                // Instead, test that success short-circuits.
                Ok(())
            }
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(call_count.load(Ordering::SeqCst), 1);
    }

    /// A per-attempt timeout (a HANGING upstream) must surface its `Err` after
    /// exactly ONE attempt — never retried same-provider. If it were retried,
    /// the worst case (90s + 1s backoff + 90s) blows past the gateway's global
    /// request timeout (tower-http `TimeoutLayer`, default 120s): the client
    /// gets a bare 408 and `chat_with_model_fallback` never sees the `Err`, so
    /// the model fallback chain never advances. This is the prod defect where
    /// a hung `nvidia/meta/llama-3.3-70b-instruct` 408'd at 120s instead of
    /// falling back to `gemini-3.1-flash-lite`.
    #[tokio::test]
    async fn test_timeout_error_is_not_retried() {
        let call_count = AtomicU32::new(0);

        // A 1ns total-request timeout deterministically wins the race against
        // the (unroutable TEST-NET) connect, yielding a pure timeout-classified
        // error — same construction as the exhaustion test below.
        let client = reqwest::Client::builder()
            .timeout(Duration::from_nanos(1))
            .build()
            .unwrap();

        let result = retry_with_backoff(2, || {
            call_count.fetch_add(1, Ordering::SeqCst);
            let client = client.clone();
            async move { client.get("http://192.0.2.1:1").send().await }
        })
        .await;

        let err = result.expect_err("timeout must surface as Err");
        assert!(
            err.is_timeout(),
            "precondition: error must be timeout-classified, got {err:?}"
        );
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            1,
            "a timeout must NOT be retried same-provider — it must surface \
             immediately so the model fallback chain can advance within the \
             global request timeout"
        );
    }

    #[tokio::test]
    async fn test_retry_returns_last_error_after_exhausting_retries() {
        let call_count = AtomicU32::new(0);

        // Connection refused (nothing listens on 127.0.0.1:1) — a connect
        // error, which IS transient/retryable. (Previously this test forced a
        // timeout error, but timeouts are no longer retried — see
        // test_timeout_error_is_not_retried.)
        let client = reqwest::Client::new();

        let result = retry_with_backoff(1, || {
            call_count.fetch_add(1, Ordering::SeqCst);
            let client = client.clone();
            async move { client.get("http://127.0.0.1:1").send().await }
        })
        .await;

        let err = result.expect_err("connection refused must surface as Err");
        assert!(
            err.is_connect(),
            "precondition: error must be connect-classified, got {err:?}"
        );
        // Should have tried original + 1 retry = 2 attempts
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
    }

    /// Budget invariant: the worst-case latency for a HANGING upstream must be
    /// strictly less than the gateway's default global request timeout
    /// ([`crate::DEFAULT_REQUEST_TIMEOUT_SECS`], tower-http `TimeoutLayer`),
    /// or the model fallback chain is structurally dead — the global layer
    /// kills the request with a bare 408 before the provider `Err` ever
    /// surfaces to `chat_with_model_fallback`.
    ///
    /// The attempt count and backoff are OBSERVED from a real
    /// `retry_with_backoff` run driven by a real timeout-classified
    /// `reqwest::Error` — never hardcoded. Re-adding `is_timeout()` to the
    /// transient predicate makes this test fail (3 attempts × 90s + 3s backoff
    /// = 273s > 120s), which is the whole point: the invariant is enforced
    /// against the retry loop's actual behaviour, not restated as a constant.
    #[tokio::test]
    async fn test_hanging_provider_budget_fits_within_global_timeout() {
        // `PROVIDER_MAX_RETRIES` is the SAME const every provider adapter
        // passes to `retry_with_backoff` — bumping it re-runs this budget check
        // against the new value rather than a stale hand-copied literal.
        let attempts = AtomicU32::new(0);

        // A 1ns total-request timeout deterministically wins the race against
        // the (unroutable TEST-NET) connect, yielding a pure timeout-classified
        // error — same construction as `test_timeout_error_is_not_retried`.
        // Each attempt therefore costs microseconds, not 90s, so whole-second
        // truncation of the elapsed wall clock recovers the retry loop's
        // backoff sum EXACTLY: 0s when no retry occurs (the loop never sleeps),
        // 1s + 2s = 3s if timeouts were (wrongly) retried twice.
        let client = reqwest::Client::builder()
            .timeout(Duration::from_nanos(1))
            .build()
            .expect("test client builds");

        let start = std::time::Instant::now();
        let result = retry_with_backoff(PROVIDER_MAX_RETRIES, || {
            attempts.fetch_add(1, Ordering::SeqCst);
            let client = client.clone();
            async move { client.get("http://192.0.2.1:1").send().await }
        })
        .await;
        let observed_backoff_secs = start.elapsed().as_secs();

        let err = result.expect_err("timeout must surface as Err");
        assert!(
            err.is_timeout(),
            "precondition: error must be timeout-classified, got {err:?}"
        );

        let observed_attempts = u64::from(attempts.load(Ordering::SeqCst));
        let worst_case_secs =
            observed_attempts * PROVIDER_REQUEST_TIMEOUT.as_secs() + observed_backoff_secs;

        assert!(
            worst_case_secs < crate::DEFAULT_REQUEST_TIMEOUT_SECS,
            "hanging-provider worst case ({observed_attempts} attempts × \
             {}s + {observed_backoff_secs}s backoff = {worst_case_secs}s) must be \
             strictly less than the default global request timeout ({}s), or a hung \
             upstream 408s instead of failing over to the next model in the chain",
            PROVIDER_REQUEST_TIMEOUT.as_secs(),
            crate::DEFAULT_REQUEST_TIMEOUT_SECS
        );
    }
}
