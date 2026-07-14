//! Provider fallback logic.
//!
//! When the primary provider fails or its circuit is open,
//! tries the next provider in the fallback chain.

use std::time::Instant;

use tracing::{info, warn};

use solvela_protocol::{ChatRequest, ChatResponse};
use solvela_router::models::ModelRegistry;

use super::health::ProviderHealthTracker;
use super::{ChatStream, ProviderError, ProviderRegistry};

/// Error surfaced when image content reaches a provider-NAME dispatch path
/// (`chat_with_fallback` / `stream_with_fallback`). These paths route by
/// provider name with no per-model capability info, so they cannot perform the
/// `supports_vision` gate the model-aware paths do — they fail closed rather
/// than dispatch an image to an un-capability-checked provider.
const IMAGE_ON_PROVIDER_NAME_PATH_ERR: &str =
    "image content requires a vision-capability-checked path; the provider-name \
     dispatch path does not support image input — route via a model-aware path";

/// True if a candidate model is INELIGIBLE for an image-bearing request because
/// its registry entry declares no vision support.
///
/// The chat route gates the PRIMARY model on `supports_vision` (415 before
/// payment), but failover/chain dispatch can re-target a DIFFERENT model after
/// payment. Without this re-check, an image request whose primary is
/// vision-capable could fail over to a non-vision model and be dispatched
/// (agent paid; image silently dropped or a paid upstream 4xx). We therefore
/// skip any candidate whose `(provider/model_id)` registry entry is non-vision.
///
/// Lookup mirrors the primary gate: the registry is keyed by the canonical
/// `provider/model_id`. A candidate the registry does not know is treated as
/// ineligible for images (fail-closed) — we will not dispatch an image to a
/// model whose capability we cannot confirm.
///
/// The `format!("{provider}/{model_id}")`-then-bare-`model_id` lookup order
/// mirrors `model_fallback_chain`'s key format (canonical `provider/model_id`
/// first, bare id as a fallback), and is fail-closed: an unknown key → reject.
fn candidate_rejects_images(registry: &ModelRegistry, provider: &str, model_id: &str) -> bool {
    let key = format!("{provider}/{model_id}");
    match registry.get(&key).or_else(|| registry.get(model_id)) {
        Some(entry) => !entry.supports_vision,
        None => true,
    }
}

/// True if a candidate model is a **free** ($0 in / $0 out) model per the
/// registry.
///
/// Free-tier models are all small/fast, so a multi-second silence means the
/// upstream is hung, not thinking — safe to fast-fail to the next chain link
/// (see [`super::FAILOVER_FAST_ATTEMPT`]). Paid/reasoning models can legitimately
/// take far longer, so they are NEVER fast-failed: unknown-to-registry or any
/// non-zero cost ⇒ not free ⇒ full [`super::PROVIDER_REQUEST_TIMEOUT`] budget.
/// `<= 0.0` (costs are validated non-negative) rather than `== 0.0` to avoid the
/// `clippy::float_cmp` lint under `-D warnings`.
fn candidate_is_free(registry: &ModelRegistry, model_id: &str) -> bool {
    registry.get(model_id).is_some_and(|m| {
        m.input_cost_per_million <= 0.0 && m.output_cost_per_million <= 0.0
    })
}

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

/// Execute a chat completion with model-aware fallback.
///
/// Uses `model_fallback_chain()` to iterate through same-tier models across
/// providers, checking both model-level and provider-level circuit breakers.
/// Returns a `FallbackResult` indicating whether the response came from the
/// originally requested model or a fallback.
pub async fn chat_with_model_fallback(
    providers: &ProviderRegistry,
    health: &ProviderHealthTracker,
    registry: &ModelRegistry,
    primary_provider: &str,
    primary_model: &str,
    req: ChatRequest,
) -> Result<FallbackResult<ChatResponse>, ProviderError> {
    let chain = model_fallback_chain(primary_provider, primary_model);
    let request_has_images = req.messages.iter().any(|m| m.content.has_image_parts());
    let mut last_error: Option<ProviderError> = None;

    for (idx, (prov, model_id)) in chain.iter().enumerate() {
        // Skip if provider is not configured
        let provider = match providers.get(prov) {
            Some(p) => p,
            None => continue,
        };

        // Skip a non-vision model for an image-bearing request (post-payment
        // failover must not dispatch an image to a model that can't see it).
        if request_has_images && candidate_rejects_images(registry, prov, model_id) {
            warn!(provider = %prov, model = %model_id, "skipping non-vision model for image request");
            continue;
        }

        // Skip if model circuit is open
        if !health.is_model_available(prov, model_id).await {
            info!(provider = %prov, model = %model_id, "skipping model (circuit open)");
            continue;
        }

        // Skip if provider circuit is open
        if !health.is_available(prov).await {
            info!(provider = %prov, "skipping provider (circuit open)");
            continue;
        }

        // Clone request and set the fallback model
        let mut model_req = req.clone();
        model_req.model = model_id.to_string();

        // Fail fast to the next link when this one is a NON-terminal FREE model:
        // a hung free upstream must not burn the full 90s budget when there's a
        // fallback waiting (the free-tier "90s first impression"). Paid/reasoning
        // models and the last link keep the full budget. Only ever shortens an
        // attempt, so the global-timeout budget invariant is preserved.
        let fast_fail = idx + 1 < chain.len() && candidate_is_free(registry, model_id);
        let start = Instant::now();
        let call = provider.chat_completion(model_req);
        let outcome = if fast_fail {
            tokio::time::timeout(super::FAILOVER_FAST_ATTEMPT, call)
                .await
                .unwrap_or_else(|_| {
                    Err(format!(
                        "free model {prov}/{model_id} exceeded the {}s fast-failover budget; trying next",
                        super::FAILOVER_FAST_ATTEMPT.as_secs()
                    )
                    .into())
                })
        } else {
            call.await
        };
        match outcome {
            Ok(response) => {
                let latency_ms = start.elapsed().as_millis() as u64;
                health
                    .record_model_success(prov, model_id, latency_ms)
                    .await;
                let was_fallback = *prov != primary_provider || *model_id != primary_model;
                info!(
                    provider = %prov,
                    model = %model_id,
                    was_fallback,
                    latency_ms,
                    "model-aware request succeeded"
                );
                return Ok(FallbackResult {
                    data: response,
                    original_model: primary_model.to_string(),
                    actual_model: model_id.to_string(),
                    actual_provider: prov.to_string(),
                    was_fallback,
                });
            }
            Err(e) => {
                let latency_ms = start.elapsed().as_millis() as u64;
                health
                    .record_model_failure(prov, model_id, latency_ms)
                    .await;
                warn!(
                    provider = %prov,
                    model = %model_id,
                    error = %e,
                    latency_ms,
                    "model-aware request failed, trying next"
                );
                last_error = Some(e);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        if request_has_images {
            "no vision-capable model available in fallback chain for image request".into()
        } else {
            "no available models in fallback chain".into()
        }
    }))
}

/// Execute a streaming chat completion with model-aware fallback.
///
/// Same pattern as `chat_with_model_fallback` but calls `chat_completion_stream`
/// and returns a `FallbackResult<ChatStream>`.
pub async fn stream_with_model_fallback(
    providers: &ProviderRegistry,
    health: &ProviderHealthTracker,
    registry: &ModelRegistry,
    primary_provider: &str,
    primary_model: &str,
    req: ChatRequest,
) -> Result<FallbackResult<ChatStream>, ProviderError> {
    let chain = model_fallback_chain(primary_provider, primary_model);
    let request_has_images = req.messages.iter().any(|m| m.content.has_image_parts());
    let mut last_error: Option<ProviderError> = None;

    for (idx, (prov, model_id)) in chain.iter().enumerate() {
        let provider = match providers.get(prov) {
            Some(p) => p,
            None => continue,
        };

        if request_has_images && candidate_rejects_images(registry, prov, model_id) {
            warn!(provider = %prov, model = %model_id, "skipping non-vision model for image stream");
            continue;
        }

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

        // Same free-tier fast-failover as the non-streaming path: a hung free
        // upstream that never returns a stream head drops to the next link fast.
        let fast_fail = idx + 1 < chain.len() && candidate_is_free(registry, model_id);
        let start = Instant::now();
        let call = provider.chat_completion_stream(model_req);
        let outcome = if fast_fail {
            tokio::time::timeout(super::FAILOVER_FAST_ATTEMPT, call)
                .await
                .unwrap_or_else(|_| {
                    Err(format!(
                        "free model {prov}/{model_id} exceeded the {}s fast-failover budget; trying next",
                        super::FAILOVER_FAST_ATTEMPT.as_secs()
                    )
                    .into())
                })
        } else {
            call.await
        };
        match outcome {
            Ok(stream) => {
                let latency_ms = start.elapsed().as_millis() as u64;
                health
                    .record_model_success(prov, model_id, latency_ms)
                    .await;
                let was_fallback = *prov != primary_provider || *model_id != primary_model;
                info!(
                    provider = %prov,
                    model = %model_id,
                    was_fallback,
                    latency_ms,
                    "model-aware streaming request succeeded"
                );
                return Ok(FallbackResult {
                    data: stream,
                    original_model: primary_model.to_string(),
                    actual_model: model_id.to_string(),
                    actual_provider: prov.to_string(),
                    was_fallback,
                });
            }
            Err(e) => {
                let latency_ms = start.elapsed().as_millis() as u64;
                health
                    .record_model_failure(prov, model_id, latency_ms)
                    .await;
                warn!(
                    provider = %prov,
                    model = %model_id,
                    error = %e,
                    "model-aware streaming fallback, trying next"
                );
                last_error = Some(e);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        if request_has_images {
            "no vision-capable model available in fallback chain for image stream".into()
        } else {
            "no available models for streaming in fallback chain".into()
        }
    }))
}

/// Execute a chat completion with fallback.
///
/// Tries providers in the given order, skipping any whose circuit is open.
/// Records success/failure metrics for circuit breaker evaluation.
///
// NOTE: this provider-NAME dispatch path does NOT perform `supports_vision`
// gating — it routes by provider name and has no per-model capability info. An
// image request is therefore rejected here fail-closed (never silently
// dispatched to an un-capability-checked provider). Use the model-aware
// `*_with_model_fallback` / `*_with_chain` paths for vision.
pub async fn chat_with_fallback(
    providers: &ProviderRegistry,
    health: &ProviderHealthTracker,
    provider_names: &[String],
    req: ChatRequest,
) -> Result<ChatResponse, ProviderError> {
    if req.messages.iter().any(|m| m.content.has_image_parts()) {
        return Err(IMAGE_ON_PROVIDER_NAME_PATH_ERR.into());
    }
    let mut last_error: Option<ProviderError> = None;

    for name in provider_names {
        // Skip if provider is not configured
        let provider = match providers.get(name) {
            Some(p) => p,
            None => continue,
        };

        // Skip if circuit is open
        if !health.is_available(name).await {
            info!(provider = %name, "skipping provider (circuit open)");
            continue;
        }

        let start = Instant::now();
        match provider.chat_completion(req.clone()).await {
            Ok(response) => {
                let latency_ms = start.elapsed().as_millis() as u64;
                health.record_success(name, latency_ms).await;
                info!(
                    provider = %name,
                    latency_ms,
                    "provider request succeeded"
                );
                return Ok(response);
            }
            Err(e) => {
                let latency_ms = start.elapsed().as_millis() as u64;
                health.record_failure(name, latency_ms).await;
                warn!(
                    provider = %name,
                    error = %e,
                    latency_ms,
                    "provider request failed, trying next"
                );
                last_error = Some(e);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| "no available providers".into()))
}

/// Execute a streaming chat completion with fallback.
///
// NOTE: like `chat_with_fallback`, this provider-NAME dispatch path does NOT do
// `supports_vision` gating, so image content is rejected here by design. Use the
// model-aware `*_with_model_fallback` / `*_with_chain` paths for vision.
pub async fn stream_with_fallback(
    providers: &ProviderRegistry,
    health: &ProviderHealthTracker,
    provider_names: &[String],
    req: ChatRequest,
) -> Result<ChatStream, ProviderError> {
    if req.messages.iter().any(|m| m.content.has_image_parts()) {
        return Err(IMAGE_ON_PROVIDER_NAME_PATH_ERR.into());
    }
    let mut last_error: Option<ProviderError> = None;

    for name in provider_names {
        let provider = match providers.get(name) {
            Some(p) => p,
            None => continue,
        };

        if !health.is_available(name).await {
            info!(provider = %name, "skipping provider for streaming (circuit open)");
            continue;
        }

        let start = Instant::now();
        match provider.chat_completion_stream(req.clone()).await {
            Ok(stream) => {
                let latency_ms = start.elapsed().as_millis() as u64;
                health.record_success(name, latency_ms).await;
                return Ok(stream);
            }
            Err(e) => {
                let latency_ms = start.elapsed().as_millis() as u64;
                health.record_failure(name, latency_ms).await;
                warn!(provider = %name, error = %e, "streaming fallback, trying next");
                last_error = Some(e);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| "no available providers for streaming".into()))
}

/// Execute a chat completion using an explicit model chain.
pub async fn chat_with_chain(
    providers: &ProviderRegistry,
    health: &ProviderHealthTracker,
    registry: &ModelRegistry,
    chain: &[(String, String)],
    original_model: &str,
    req: ChatRequest,
) -> Result<FallbackResult<ChatResponse>, ProviderError> {
    let request_has_images = req.messages.iter().any(|m| m.content.has_image_parts());
    let mut last_error: Option<ProviderError> = None;

    for (prov, model_id) in chain {
        let provider = match providers.get(prov) {
            Some(p) => p,
            None => continue,
        };

        if request_has_images && candidate_rejects_images(registry, prov, model_id) {
            warn!(provider = %prov, model = %model_id, "skipping non-vision model for image request (chain)");
            continue;
        }

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
                health
                    .record_model_success(prov, model_id, latency_ms)
                    .await;
                let was_fallback = model_id != original_model;
                if was_fallback {
                    info!(
                        original_model,
                        fallback_provider = %prov, fallback_model = %model_id,
                        latency_ms,
                        "served from fallback model (agent preference)"
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
                health
                    .record_model_failure(prov, model_id, latency_ms)
                    .await;
                warn!(
                    provider = %prov, model = %model_id,
                    error = %e, latency_ms,
                    "model request failed, trying next in chain"
                );
                last_error = Some(e);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        if request_has_images {
            "no vision-capable model available in chain for image request".into()
        } else {
            "no available models in chain".into()
        }
    }))
}

/// Execute a streaming chat completion using an explicit model chain.
pub async fn stream_with_chain(
    providers: &ProviderRegistry,
    health: &ProviderHealthTracker,
    registry: &ModelRegistry,
    chain: &[(String, String)],
    original_model: &str,
    req: ChatRequest,
) -> Result<FallbackResult<ChatStream>, ProviderError> {
    let request_has_images = req.messages.iter().any(|m| m.content.has_image_parts());
    let mut last_error: Option<ProviderError> = None;

    for (prov, model_id) in chain {
        let provider = match providers.get(prov) {
            Some(p) => p,
            None => continue,
        };

        if request_has_images && candidate_rejects_images(registry, prov, model_id) {
            warn!(provider = %prov, model = %model_id, "skipping non-vision model for image stream (chain)");
            continue;
        }

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
                health
                    .record_model_success(prov, model_id, latency_ms)
                    .await;
                let was_fallback = model_id != original_model;
                if was_fallback {
                    info!(
                        original_model,
                        fallback_provider = %prov, fallback_model = %model_id,
                        "streaming from fallback model (agent preference)"
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
                health
                    .record_model_failure(prov, model_id, latency_ms)
                    .await;
                warn!(provider = %prov, model = %model_id, error = %e, "streaming model fallback, trying next");
                last_error = Some(e);
            }
        }
    }

    Err(last_error.unwrap_or_else(|| {
        if request_has_images {
            "no vision-capable model available in chain for image stream".into()
        } else {
            "no available models for streaming in chain".into()
        }
    }))
}

/// Get the ordered fallback list for a provider.
///
/// The primary provider is always first. If it fails, try these alternatives
/// in order. Returns owned `String`s since the input may not be `'static`.
pub fn fallback_chain(primary: &str) -> Vec<String> {
    let chain: Vec<&str> = match primary {
        "openai" => vec!["openai", "anthropic", "google", "deepseek"],
        "anthropic" => vec!["anthropic", "openai", "google"],
        "google" => vec!["google", "openai", "anthropic"],
        "deepseek" => vec!["deepseek", "openai"],
        "xai" => vec!["xai", "openai", "anthropic"],
        _ => vec![primary],
    };
    chain.into_iter().map(String::from).collect()
}

/// Get the ordered fallback list for a specific model.
///
/// Returns (provider, model_id) tuples. The primary model is always first.
/// Fallback models are same-capability-tier from different providers.
pub fn model_fallback_chain<'a>(provider: &'a str, model: &'a str) -> Vec<(&'a str, &'a str)> {
    let chain: Vec<(&str, &str)> = match (provider, model) {
        // --- Premium tier (reasoning, high capability) ---
        // Opus degrades newest -> older within Anthropic first, then falls
        // through to the same cross-provider tail the 4.6 chain uses.
        ("anthropic", "claude-opus-4-8") => vec![
            ("anthropic", "claude-opus-4-8"),
            ("anthropic", "claude-opus-4-7"),
            ("anthropic", "claude-opus-4-6"),
            ("openai", "gpt-5.2"),
            ("google", "gemini-3.1-pro"),
            ("openai", "o3"),
        ],
        ("anthropic", "claude-opus-4-7") => vec![
            ("anthropic", "claude-opus-4-7"),
            ("anthropic", "claude-opus-4-6"),
            ("openai", "gpt-5.2"),
            ("google", "gemini-3.1-pro"),
            ("openai", "o3"),
        ],
        ("anthropic", "claude-opus-4-6") => vec![
            ("anthropic", "claude-opus-4-6"),
            ("openai", "gpt-5.2"),
            ("google", "gemini-3.1-pro"),
            ("openai", "o3"),
        ],
        ("openai", "gpt-5.2") => vec![
            ("openai", "gpt-5.2"),
            ("anthropic", "claude-opus-4-6"),
            ("google", "gemini-3.1-pro"),
        ],
        ("google", "gemini-3.1-pro") => vec![
            ("google", "gemini-3.1-pro"),
            ("anthropic", "claude-opus-4-6"),
            ("openai", "gpt-5.2"),
        ],

        // --- Mid tier (strong general purpose) ---
        ("anthropic", "claude-sonnet-4-6") => vec![
            ("anthropic", "claude-sonnet-4-6"),
            ("openai", "gpt-4.1"),
            ("google", "gemini-3.1-pro"),
            ("xai", "grok-3"),
        ],
        ("openai", "gpt-4o") => vec![
            ("openai", "gpt-4o"),
            ("anthropic", "claude-sonnet-4-6"),
            ("google", "gemini-3.1-pro"),
            ("xai", "grok-3"),
        ],
        ("openai", "gpt-4.1") => vec![
            ("openai", "gpt-4.1"),
            ("anthropic", "claude-sonnet-4-6"),
            ("google", "gemini-3.1-pro"),
        ],
        ("xai", "grok-3") => vec![
            ("xai", "grok-3"),
            ("anthropic", "claude-sonnet-4-6"),
            ("openai", "gpt-4o"),
        ],

        // --- Budget tier (fast, cheap) ---
        ("openai", "gpt-4o-mini") => vec![
            ("openai", "gpt-4o-mini"),
            ("anthropic", "claude-haiku-4-5-20251001"),
            ("google", "gemini-2.5-flash"),
            ("deepseek", "deepseek-chat"),
        ],
        ("openai", "gpt-4.1-mini") => vec![
            ("openai", "gpt-4.1-mini"),
            ("anthropic", "claude-haiku-4-5-20251001"),
            ("google", "gemini-2.5-flash"),
        ],
        ("openai", "gpt-4.1-nano") => vec![
            ("openai", "gpt-4.1-nano"),
            ("google", "gemini-2.5-flash-lite"),
        ],
        ("anthropic", "claude-haiku-4-5-20251001") => vec![
            ("anthropic", "claude-haiku-4-5-20251001"),
            ("openai", "gpt-4o-mini"),
            ("google", "gemini-2.5-flash"),
            ("deepseek", "deepseek-chat"),
        ],
        ("google", "gemini-2.5-flash") => vec![
            ("google", "gemini-2.5-flash"),
            ("openai", "gpt-4o-mini"),
            ("anthropic", "claude-haiku-4-5-20251001"),
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
            ("anthropic", "claude-opus-4-6"),
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

        // --- NVIDIA NIM free tier (every $0 NVIDIA model) ---
        // If NIM is unavailable or rate-limited, fall back to Google's free
        // Gemini so the free tier degrades gracefully instead of erroring.
        // Keyed on the FULL canonical string `resolve_model()` emits (the
        // doubled-`nvidia/` form — `nvidia/` provider prefix + the
        // publisher-qualified `nvidia/...` model_id), because THAT is exactly
        // what reaches `model_fallback_chain` at runtime (`req.model`). The
        // Gemini target is bare, matching the chain convention; `google.rs`
        // strips no prefix off a bare id. These keys are intentionally NOT in
        // `fallback_chain_keys()` (whose `every_fallback_chain_id_is_registered`
        // helper prepends `provider/` and would mis-derive a triple prefix);
        // `nvidia_free_models_fall_back_to_gemini` proves they fire + register.
        //
        // EVERY free ($0/$0) NVIDIA model in config/models.toml is listed here
        // so any of them degrades to the free Gemini — not just the 3 the Free
        // profile routes to. The fallback target gemini-3.1-flash-lite is itself
        // $0/$0, so the degraded path stays free. The 2 PAID NVIDIA models
        // (nemotron-super-49b-v1.5, nvidia-nemotron-nano-9b-v2) are deliberately
        // OMITTED: a paid request must never silently fall back to a
        // differently-priced/$0 model — they keep the primary-only `_` behavior.
        ("nvidia", "nvidia/nvidia/llama-3.1-nemotron-nano-8b-v1")
        | ("nvidia", "nvidia/nvidia/llama-3.1-nemotron-ultra-253b-v1")
        | ("nvidia", "nvidia/nvidia/llama-3.3-nemotron-super-49b-v1")
        | ("nvidia", "nvidia/nvidia/nemotron-3-nano-30b-a3b")
        | ("nvidia", "nvidia/nvidia/nemotron-3-super-120b-a12b")
        | ("nvidia", "nvidia/nvidia/nemotron-3-ultra-550b-a55b")
        | ("nvidia", "nvidia/nvidia/nemotron-mini-4b-instruct")
        | ("nvidia", "nvidia/meta/llama-4-maverick-17b-128e-instruct")
        | ("nvidia", "nvidia/meta/llama-4-scout-17b-16e-instruct")
        | ("nvidia", "nvidia/meta/llama-3.3-70b-instruct")
        | ("nvidia", "nvidia/qwen/qwen3-coder-480b-a35b-instruct")
        | ("nvidia", "nvidia/deepseek-ai/deepseek-r1")
        | ("nvidia", "nvidia/mistralai/mistral-large-3-675b-instruct-2512")
        | ("nvidia", "nvidia/minimaxai/minimax-m2.7")
        | ("nvidia", "nvidia/minimaxai/minimax-m3") => {
            vec![(provider, model), ("google", "gemini-3.1-flash-lite")]
        }

        // --- Unknown model: just return itself ---
        _ => vec![(provider, model)],
    };
    chain
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use solvela_protocol::{
        ChatMessage, ContentPart, ImageUrl, MessageContent, ModelRegistration, Role,
    };
    use solvela_router::models::ModelRegistry;

    use crate::providers::health::{CircuitBreakerConfig, ProviderHealthTracker};
    use crate::providers::{ChatStream, LLMProvider, ProviderError, ProviderRegistry};

    /// A provider that records every model id it is dispatched, and always
    /// fails the call (so the loop advances to the next chain entry). Lets a
    /// test assert that a non-vision model is NEVER dispatched for an image
    /// request.
    struct RecordingProvider {
        name: String,
        dispatched: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl LLMProvider for RecordingProvider {
        fn name(&self) -> &str {
            &self.name
        }
        fn supported_models(&self) -> Vec<ModelRegistration> {
            vec![]
        }
        async fn chat_completion(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError> {
            self.dispatched.lock().unwrap().push(req.model.clone());
            Err("simulated provider failure".into())
        }
        async fn chat_completion_stream(
            &self,
            req: ChatRequest,
        ) -> Result<ChatStream, ProviderError> {
            self.dispatched.lock().unwrap().push(req.model.clone());
            Err("simulated provider failure".into())
        }
    }

    /// A provider that fails every call with a REAL reqwest timeout-classified
    /// error — the exact error a hung upstream produces when the per-attempt
    /// timeout (`PROVIDER_REQUEST_TIMEOUT`) fires.
    struct TimeoutErrProvider {
        name: String,
        dispatched: Arc<Mutex<Vec<String>>>,
    }

    impl TimeoutErrProvider {
        /// Construct a real `reqwest::Error` with `is_timeout() == true`: a 1ns
        /// total-request timeout deterministically wins the race against the
        /// (unroutable TEST-NET) connect. Same construction as the
        /// `retry_with_backoff` tests in `providers/mod.rs`.
        async fn real_timeout_error() -> ProviderError {
            let err = reqwest::Client::builder()
                .timeout(std::time::Duration::from_nanos(1))
                .build()
                .unwrap()
                .get("http://192.0.2.1:1")
                .send()
                .await
                .expect_err("1ns timeout must fire");
            assert!(err.is_timeout(), "precondition: {err:?}");
            Box::new(err)
        }
    }

    #[async_trait]
    impl LLMProvider for TimeoutErrProvider {
        fn name(&self) -> &str {
            &self.name
        }
        fn supported_models(&self) -> Vec<ModelRegistration> {
            vec![]
        }
        async fn chat_completion(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError> {
            self.dispatched.lock().unwrap().push(req.model.clone());
            Err(Self::real_timeout_error().await)
        }
        async fn chat_completion_stream(
            &self,
            req: ChatRequest,
        ) -> Result<ChatStream, ProviderError> {
            self.dispatched.lock().unwrap().push(req.model.clone());
            Err(Self::real_timeout_error().await)
        }
    }

    /// A provider that answers every call with a stub response.
    struct SucceedingProvider {
        name: String,
        dispatched: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl LLMProvider for SucceedingProvider {
        fn name(&self) -> &str {
            &self.name
        }
        fn supported_models(&self) -> Vec<ModelRegistration> {
            vec![]
        }
        async fn chat_completion(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError> {
            self.dispatched.lock().unwrap().push(req.model.clone());
            Ok(ChatResponse {
                id: "chatcmpl-test".to_string(),
                object: "chat.completion".to_string(),
                created: 0,
                model: req.model,
                choices: vec![],
                usage: None,
            })
        }
    }

    /// Registry with a vision-capable and a non-vision model under the same
    /// provider, so a chain can mix them.
    fn vision_mixed_registry() -> ModelRegistry {
        ModelRegistry::from_toml(
            r#"
[models.vision-model]
provider = "openai"
model_id = "vis"
display_name = "Vision"
input_cost_per_million = 1.0
output_cost_per_million = 1.0
context_window = 128000
supports_streaming = true
supports_vision = true

[models.text-model]
provider = "openai"
model_id = "txt"
display_name = "Text"
input_cost_per_million = 1.0
output_cost_per_million = 1.0
context_window = 128000
supports_streaming = true
"#,
        )
        .unwrap()
    }

    /// Build a request whose single message carries an image content part.
    fn image_request() -> ChatRequest {
        ChatRequest {
            model: "openai/gpt-4o".to_string(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Parts(vec![
                    ContentPart::Text {
                        text: "what is in this image?".to_string(),
                    },
                    ContentPart::ImageUrl {
                        image_url: ImageUrl {
                            url: "https://example.com/cat.png".to_string(),
                            detail: None,
                        },
                    },
                ]),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            max_tokens: None,
            temperature: None,
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
        }
    }

    /// Registry mirroring a real `model_fallback_chain` arm so the
    /// `*_with_model_fallback` paths (which build the chain INTERNALLY from
    /// `model_fallback_chain`, unlike the `*_with_chain` paths that take an
    /// explicit chain) can be exercised. For `("openai", "gpt-4o")` the chain is
    /// `[gpt-4o, claude-sonnet-4-6, gemini-3.1-pro, grok-3]`; here the primary
    /// `openai/gpt-4o` is vision-capable and the next-in-chain
    /// `anthropic/claude-sonnet-4-6` is declared NON-vision, so a correct
    /// implementation must dispatch the primary (then fail) and SKIP the
    /// non-vision fallback rather than dispatch an image to it post-payment.
    fn gpt4o_chain_mixed_registry() -> ModelRegistry {
        ModelRegistry::from_toml(
            r#"
[models.gpt-4o]
provider = "openai"
model_id = "gpt-4o"
display_name = "GPT-4o"
input_cost_per_million = 1.0
output_cost_per_million = 1.0
context_window = 128000
supports_streaming = true
supports_vision = true

[models.claude-sonnet]
provider = "anthropic"
model_id = "claude-sonnet-4-6"
display_name = "Claude Sonnet"
input_cost_per_million = 1.0
output_cost_per_million = 1.0
context_window = 128000
supports_streaming = true
"#,
        )
        .unwrap()
    }

    /// FIX 1 (PR #2 round-1 review): post-payment failover must NOT dispatch an
    /// image request to a non-vision model. The chain's first entry is
    /// vision-capable (and is dispatched, then fails); the second entry is
    /// non-vision and must be SKIPPED — never dispatched — and the call must
    /// surface a clear vision-exhaustion error rather than silently dropping
    /// the image at a text-only model that the agent already paid for.
    #[tokio::test]
    async fn image_request_never_dispatches_to_non_vision_fallback() {
        let dispatched = Arc::new(Mutex::new(Vec::<String>::new()));
        let mut map: HashMap<String, Arc<dyn LLMProvider>> = HashMap::new();
        map.insert(
            "openai".to_string(),
            Arc::new(RecordingProvider {
                name: "openai".to_string(),
                dispatched: dispatched.clone(),
            }),
        );
        let providers = ProviderRegistry::from_providers(map);
        let health = ProviderHealthTracker::new(CircuitBreakerConfig::default());
        let registry = vision_mixed_registry();

        // vision model first (dispatched, fails), then NON-vision model.
        let chain = vec![
            ("openai".to_string(), "vis".to_string()),
            ("openai".to_string(), "txt".to_string()),
        ];
        let err = chat_with_chain(
            &providers,
            &health,
            &registry,
            &chain,
            "vis",
            image_request(),
        )
        .await
        .expect_err("must error: only the non-vision fallback remained");

        let calls = dispatched.lock().unwrap().clone();
        assert!(
            calls.contains(&"vis".to_string()),
            "the vision-capable model should have been dispatched, got {calls:?}"
        );
        assert!(
            !calls.contains(&"txt".to_string()),
            "the non-vision fallback model must NEVER be dispatched for an image \
             request, got {calls:?}"
        );
        // The surfaced error is the vision model's own provider failure (it WAS
        // dispatched and failed). The point of this test is the skip, asserted
        // above. The all-non-vision case below pins the vision-exhaustion error.
        assert!(
            err.to_string().contains("simulated provider failure"),
            "{err}"
        );
    }

    /// FIX 1 (test-coverage review): the `*_with_chain` paths are tested above,
    /// but `chat_with_model_fallback` — which `routes/chat/provider.rs` and the
    /// A2A handler actually call — builds its chain INTERNALLY via
    /// `model_fallback_chain` and had no direct vision-skip test. A regression
    /// that reverted the `candidate_rejects_images` check inside THIS function
    /// would slip past CI. This pins it: the generated chain for
    /// `("openai","gpt-4o")` is `[gpt-4o, claude-sonnet-4-6, ...]`; with a
    /// registry where the primary is vision-capable and the next entry is
    /// non-vision, the primary is dispatched (and fails) but the non-vision
    /// fallback must NEVER be dispatched.
    #[tokio::test]
    async fn model_fallback_image_request_never_dispatches_to_non_vision() {
        let dispatched = Arc::new(Mutex::new(Vec::<String>::new()));
        let mut map: HashMap<String, Arc<dyn LLMProvider>> = HashMap::new();
        map.insert(
            "openai".to_string(),
            Arc::new(RecordingProvider {
                name: "openai".to_string(),
                dispatched: dispatched.clone(),
            }),
        );
        // The non-vision fallback's provider IS configured — so if the skip were
        // broken, the image WOULD be dispatched to it. That is what makes the
        // negative assertion meaningful.
        map.insert(
            "anthropic".to_string(),
            Arc::new(RecordingProvider {
                name: "anthropic".to_string(),
                dispatched: dispatched.clone(),
            }),
        );
        let providers = ProviderRegistry::from_providers(map);
        let health = ProviderHealthTracker::new(CircuitBreakerConfig::default());
        let registry = gpt4o_chain_mixed_registry();

        // primary openai/gpt-4o (vision) is dispatched then fails; the chain's
        // next entry anthropic/claude-sonnet-4-6 (non-vision) must be skipped.
        let err = chat_with_model_fallback(
            &providers,
            &health,
            &registry,
            "openai",
            "gpt-4o",
            image_request(),
        )
        .await
        .expect_err("must error once the vision primary fails and only non-vision remains");

        let calls = dispatched.lock().unwrap().clone();
        assert!(
            calls.contains(&"gpt-4o".to_string()),
            "the vision-capable primary should have been dispatched, got {calls:?}"
        );
        assert!(
            !calls.contains(&"claude-sonnet-4-6".to_string()),
            "the non-vision fallback must NEVER be dispatched for an image request \
             via chat_with_model_fallback, got {calls:?}"
        );
        assert!(
            err.to_string().contains("simulated provider failure"),
            "{err}"
        );
    }

    /// The prod-defect scenario at the chain level: a provider failing with a
    /// timeout-classified `Err` (a hung upstream whose per-attempt timeout
    /// fired) must advance `chat_with_model_fallback` to the next chain entry,
    /// which then serves the request as a fallback. Before the
    /// `retry_with_backoff` fix, the timeout `Err` never surfaced within the
    /// global request timeout, so this loop never ran its `Err` arm — the
    /// client got a bare 408 and the documented fallback never fired.
    #[tokio::test]
    async fn timeout_err_advances_model_fallback_chain() {
        let dispatched = Arc::new(Mutex::new(Vec::<String>::new()));
        let mut map: HashMap<String, Arc<dyn LLMProvider>> = HashMap::new();
        // Primary openai/gpt-4o hangs (times out); next-in-chain
        // anthropic/claude-sonnet-4-6 succeeds.
        map.insert(
            "openai".to_string(),
            Arc::new(TimeoutErrProvider {
                name: "openai".to_string(),
                dispatched: dispatched.clone(),
            }),
        );
        map.insert(
            "anthropic".to_string(),
            Arc::new(SucceedingProvider {
                name: "anthropic".to_string(),
                dispatched: dispatched.clone(),
            }),
        );
        let providers = ProviderRegistry::from_providers(map);
        let health = ProviderHealthTracker::new(CircuitBreakerConfig::default());
        let registry = gpt4o_chain_mixed_registry();

        let req = ChatRequest {
            model: "openai/gpt-4o".to_string(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text("hello".to_string()),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            max_tokens: None,
            temperature: None,
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
        };

        let result =
            chat_with_model_fallback(&providers, &health, &registry, "openai", "gpt-4o", req)
                .await
                .expect("the chain must advance past the timed-out primary and succeed");

        assert!(result.was_fallback, "response must be marked as a fallback");
        assert_eq!(result.actual_provider, "anthropic");
        assert_eq!(result.actual_model, "claude-sonnet-4-6");
        let calls = dispatched.lock().unwrap().clone();
        assert_eq!(
            calls,
            vec!["gpt-4o".to_string(), "claude-sonnet-4-6".to_string()],
            "the timed-out primary must be dispatched first, then the chain \
             must advance to the next entry"
        );
    }

    /// A provider whose call HANGS for effectively forever before erroring —
    /// a truly hung upstream (accepts the request, never answers). Under
    /// `start_paused`, virtual time only advances past this if a fast-failover
    /// timeout caps the call; otherwise the loop would wait the full hang.
    struct HangingProvider {
        name: String,
        dispatched: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl LLMProvider for HangingProvider {
        fn name(&self) -> &str {
            &self.name
        }
        fn supported_models(&self) -> Vec<ModelRegistration> {
            vec![]
        }
        async fn chat_completion(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError> {
            self.dispatched.lock().unwrap().push(req.model.clone());
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            Err("hung upstream finally errored".into())
        }
    }

    /// Like `gpt4o_chain_mixed_registry` but the primary `gpt-4o` is FREE
    /// ($0/$0) — the free-tier condition that arms fast-failover.
    fn gpt4o_free_primary_registry() -> ModelRegistry {
        ModelRegistry::from_toml(
            r#"
[models.gpt-4o]
provider = "openai"
model_id = "gpt-4o"
display_name = "GPT-4o (free in test)"
input_cost_per_million = 0.0
output_cost_per_million = 0.0
context_window = 128000
supports_streaming = true

[models.claude-sonnet]
provider = "anthropic"
model_id = "claude-sonnet-4-6"
display_name = "Claude Sonnet"
input_cost_per_million = 1.0
output_cost_per_million = 1.0
context_window = 128000
supports_streaming = true
"#,
        )
        .unwrap()
    }

    #[test]
    fn candidate_is_free_classifies_by_registry_cost() {
        let registry = gpt4o_free_primary_registry();
        assert!(
            candidate_is_free(&registry, "gpt-4o"),
            "$0/$0 model must classify as free"
        );
        assert!(
            !candidate_is_free(&registry, "claude-sonnet-4-6"),
            "a priced model must NOT classify as free"
        );
        assert!(
            !candidate_is_free(&registry, "not-in-registry"),
            "unknown model defaults to NOT free (keeps the full 90s budget)"
        );
    }

    /// The free-tier "90s first impression" fix: a HUNG **free** non-terminal
    /// model must fail over within `FAILOVER_FAST_ATTEMPT`, not after the full
    /// `PROVIDER_REQUEST_TIMEOUT` (90s). Paused time makes the assertion about
    /// VIRTUAL elapsed: fast-fail ⇒ ~10s; a regression that dropped the timeout
    /// wrap ⇒ the full 3600s hang, which the upper bound below catches.
    #[tokio::test(start_paused = true)]
    async fn hung_free_nonterminal_fast_fails_to_next_link() {
        let dispatched = Arc::new(Mutex::new(Vec::<String>::new()));
        let mut map: HashMap<String, Arc<dyn LLMProvider>> = HashMap::new();
        map.insert(
            "openai".to_string(),
            Arc::new(HangingProvider {
                name: "openai".to_string(),
                dispatched: dispatched.clone(),
            }),
        );
        map.insert(
            "anthropic".to_string(),
            Arc::new(SucceedingProvider {
                name: "anthropic".to_string(),
                dispatched: dispatched.clone(),
            }),
        );
        let providers = ProviderRegistry::from_providers(map);
        let health = ProviderHealthTracker::new(CircuitBreakerConfig::default());
        let registry = gpt4o_free_primary_registry();

        let req = ChatRequest {
            model: "openai/gpt-4o".to_string(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Text("hello".to_string()),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            max_tokens: None,
            temperature: None,
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
        };

        let started = tokio::time::Instant::now();
        let result =
            chat_with_model_fallback(&providers, &health, &registry, "openai", "gpt-4o", req)
                .await
                .expect("a hung free primary must fast-fail over to the succeeding fallback");
        let elapsed = started.elapsed();

        assert_eq!(result.actual_model, "claude-sonnet-4-6");
        assert!(result.was_fallback, "response must be marked as a fallback");
        assert!(
            elapsed < std::time::Duration::from_secs(30),
            "expected failover near {}s (fast budget), took {elapsed:?} — the \
             fast-fail wrap did not fire",
            crate::providers::FAILOVER_FAST_ATTEMPT.as_secs()
        );
        let calls = dispatched.lock().unwrap().clone();
        assert_eq!(
            calls,
            vec!["gpt-4o".to_string(), "claude-sonnet-4-6".to_string()],
            "hung free primary dispatched first, then failover to the next link"
        );
    }

    /// Streaming twin: `stream_with_model_fallback` must apply the same vision
    /// skip on its internally-built chain. Same coverage-gap rationale as the
    /// non-streaming test above.
    #[tokio::test]
    async fn model_fallback_image_stream_never_dispatches_to_non_vision() {
        let dispatched = Arc::new(Mutex::new(Vec::<String>::new()));
        let mut map: HashMap<String, Arc<dyn LLMProvider>> = HashMap::new();
        map.insert(
            "openai".to_string(),
            Arc::new(RecordingProvider {
                name: "openai".to_string(),
                dispatched: dispatched.clone(),
            }),
        );
        map.insert(
            "anthropic".to_string(),
            Arc::new(RecordingProvider {
                name: "anthropic".to_string(),
                dispatched: dispatched.clone(),
            }),
        );
        let providers = ProviderRegistry::from_providers(map);
        let health = ProviderHealthTracker::new(CircuitBreakerConfig::default());
        let registry = gpt4o_chain_mixed_registry();

        let err = stream_with_model_fallback(
            &providers,
            &health,
            &registry,
            "openai",
            "gpt-4o",
            image_request(),
        )
        .await
        .map(|_| ())
        .expect_err("must error once the vision primary fails and only non-vision remains");

        let calls = dispatched.lock().unwrap().clone();
        assert!(
            calls.contains(&"gpt-4o".to_string()),
            "the vision-capable primary should have been dispatched, got {calls:?}"
        );
        assert!(
            !calls.contains(&"claude-sonnet-4-6".to_string()),
            "the non-vision fallback must NEVER be dispatched for an image stream \
             via stream_with_model_fallback, got {calls:?}"
        );
        assert!(
            err.to_string().contains("simulated provider failure"),
            "{err}"
        );
    }

    /// When EVERY candidate is non-vision, an image request must surface a clear
    /// vision-exhaustion error and dispatch to NONE of them.
    #[tokio::test]
    async fn image_request_all_non_vision_surfaces_vision_error() {
        let dispatched = Arc::new(Mutex::new(Vec::<String>::new()));
        let mut map: HashMap<String, Arc<dyn LLMProvider>> = HashMap::new();
        map.insert(
            "openai".to_string(),
            Arc::new(RecordingProvider {
                name: "openai".to_string(),
                dispatched: dispatched.clone(),
            }),
        );
        let providers = ProviderRegistry::from_providers(map);
        let health = ProviderHealthTracker::new(CircuitBreakerConfig::default());
        let registry = vision_mixed_registry();

        let chain = vec![("openai".to_string(), "txt".to_string())];
        let err = chat_with_chain(
            &providers,
            &health,
            &registry,
            &chain,
            "txt",
            image_request(),
        )
        .await
        .expect_err("an all-non-vision chain must error for an image request");

        assert!(
            dispatched.lock().unwrap().is_empty(),
            "no non-vision model may be dispatched for an image request"
        );
        assert!(
            err.to_string().contains("vision-capable"),
            "error must name the vision-capability exhaustion, got: {err}"
        );
    }

    /// A TEXT request must still be free to fall back to the non-vision model —
    /// the vision skip is scoped to image-bearing requests only.
    #[tokio::test]
    async fn text_request_may_dispatch_to_non_vision_fallback() {
        let dispatched = Arc::new(Mutex::new(Vec::<String>::new()));
        let mut map: HashMap<String, Arc<dyn LLMProvider>> = HashMap::new();
        map.insert(
            "openai".to_string(),
            Arc::new(RecordingProvider {
                name: "openai".to_string(),
                dispatched: dispatched.clone(),
            }),
        );
        let providers = ProviderRegistry::from_providers(map);
        let health = ProviderHealthTracker::new(CircuitBreakerConfig::default());
        let registry = vision_mixed_registry();

        let chain = vec![
            ("openai".to_string(), "vis".to_string()),
            ("openai".to_string(), "txt".to_string()),
        ];
        let mut text_req = image_request();
        text_req.messages = vec![ChatMessage {
            role: Role::User,
            content: "hello".into(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }];
        let _ = chat_with_chain(&providers, &health, &registry, &chain, "vis", text_req).await;

        let calls = dispatched.lock().unwrap().clone();
        assert!(
            calls.contains(&"txt".to_string()),
            "a text request must be allowed to fall back to the non-vision model, got {calls:?}"
        );
    }

    /// The streaming chain path applies the same vision skip.
    #[tokio::test]
    async fn image_stream_never_dispatches_to_non_vision_fallback() {
        let dispatched = Arc::new(Mutex::new(Vec::<String>::new()));
        let mut map: HashMap<String, Arc<dyn LLMProvider>> = HashMap::new();
        map.insert(
            "openai".to_string(),
            Arc::new(RecordingProvider {
                name: "openai".to_string(),
                dispatched: dispatched.clone(),
            }),
        );
        let providers = ProviderRegistry::from_providers(map);
        let health = ProviderHealthTracker::new(CircuitBreakerConfig::default());
        let registry = vision_mixed_registry();

        let chain = vec![
            ("openai".to_string(), "vis".to_string()),
            ("openai".to_string(), "txt".to_string()),
        ];
        let err = stream_with_chain(
            &providers,
            &health,
            &registry,
            &chain,
            "vis",
            image_request(),
        )
        .await
        .map(|_| ())
        .expect_err("must error: only the non-vision fallback remained");

        let calls = dispatched.lock().unwrap().clone();
        assert!(!calls.contains(&"txt".to_string()), "got {calls:?}");
        // Surfaced error is the vision model's dispatch failure; the skip is the
        // asserted invariant.
        assert!(
            err.to_string().contains("simulated provider failure"),
            "got: {err}"
        );
    }

    /// FIX 1 (PR #2 round-2 review): the provider-NAME dispatch path
    /// `chat_with_fallback` does NO `supports_vision` gating (it has no
    /// per-model capability info — it dispatches by provider name only). An
    /// image-bearing request must therefore be rejected at the top, fail-closed,
    /// and NEVER dispatched to any provider. Round-1 removed the old
    /// `reject_image_content` backstop here without a replacement; this restores
    /// it as a hard error.
    #[tokio::test]
    async fn chat_with_fallback_rejects_image_request_without_dispatch() {
        let dispatched = Arc::new(Mutex::new(Vec::<String>::new()));
        let mut map: HashMap<String, Arc<dyn LLMProvider>> = HashMap::new();
        map.insert(
            "openai".to_string(),
            Arc::new(RecordingProvider {
                name: "openai".to_string(),
                dispatched: dispatched.clone(),
            }),
        );
        let providers = ProviderRegistry::from_providers(map);
        let health = ProviderHealthTracker::new(CircuitBreakerConfig::default());

        let err = chat_with_fallback(
            &providers,
            &health,
            &["openai".to_string()],
            image_request(),
        )
        .await
        .expect_err("provider-name dispatch must reject image content");

        assert!(
            dispatched.lock().unwrap().is_empty(),
            "no provider may be dispatched an image via the un-capability-checked \
             provider-name path"
        );
        assert!(
            err.to_string().contains("vision"),
            "error must name the vision-unsupported reason, got: {err}"
        );
    }

    /// Streaming twin of the above: `stream_with_fallback` must also reject image
    /// content fail-closed without dispatching.
    #[tokio::test]
    async fn stream_with_fallback_rejects_image_request_without_dispatch() {
        let dispatched = Arc::new(Mutex::new(Vec::<String>::new()));
        let mut map: HashMap<String, Arc<dyn LLMProvider>> = HashMap::new();
        map.insert(
            "openai".to_string(),
            Arc::new(RecordingProvider {
                name: "openai".to_string(),
                dispatched: dispatched.clone(),
            }),
        );
        let providers = ProviderRegistry::from_providers(map);
        let health = ProviderHealthTracker::new(CircuitBreakerConfig::default());

        let err = stream_with_fallback(
            &providers,
            &health,
            &["openai".to_string()],
            image_request(),
        )
        .await
        .map(|_| ())
        .expect_err("provider-name stream dispatch must reject image content");

        assert!(
            dispatched.lock().unwrap().is_empty(),
            "no provider may be dispatched an image via the un-capability-checked \
             provider-name stream path"
        );
        assert!(
            err.to_string().contains("vision"),
            "error must name the vision-unsupported reason, got: {err}"
        );
    }

    /// A TEXT request through the provider-name path must still dispatch normally
    /// (the rejection is scoped to image-bearing requests only).
    #[tokio::test]
    async fn chat_with_fallback_dispatches_text_request() {
        let dispatched = Arc::new(Mutex::new(Vec::<String>::new()));
        let mut map: HashMap<String, Arc<dyn LLMProvider>> = HashMap::new();
        map.insert(
            "openai".to_string(),
            Arc::new(RecordingProvider {
                name: "openai".to_string(),
                dispatched: dispatched.clone(),
            }),
        );
        let providers = ProviderRegistry::from_providers(map);
        let health = ProviderHealthTracker::new(CircuitBreakerConfig::default());

        let mut text_req = image_request();
        text_req.messages = vec![ChatMessage {
            role: Role::User,
            content: "hello".into(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }];
        // Provider always fails, but the point is that it WAS dispatched (the
        // image guard did not short-circuit a text request).
        let _ = chat_with_fallback(&providers, &health, &["openai".to_string()], text_req).await;
        assert!(
            dispatched
                .lock()
                .unwrap()
                .contains(&"openai/gpt-4o".to_string()),
            "a text request must be dispatched through the provider-name path"
        );
    }

    #[test]
    fn test_fallback_chain_primary_first() {
        let chain = fallback_chain("openai");
        assert_eq!(chain[0], "openai");
        assert!(chain.len() > 1);

        let chain = fallback_chain("anthropic");
        assert_eq!(chain[0], "anthropic");
        assert!(chain.len() > 1);

        let chain = fallback_chain("google");
        assert_eq!(chain[0], "google");
        assert!(chain.len() > 1);

        let chain = fallback_chain("deepseek");
        assert_eq!(chain[0], "deepseek");
        assert!(chain.len() > 1);

        let chain = fallback_chain("xai");
        assert_eq!(chain[0], "xai");
        assert!(chain.len() > 1);
    }

    #[test]
    fn test_fallback_chain_unknown_provider() {
        let chain = fallback_chain("unknown-provider");
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0], "unknown-provider");
    }

    #[test]
    fn test_fallback_chain_no_duplicates() {
        for provider in &["openai", "anthropic", "google", "deepseek", "xai", "nvidia"] {
            let chain = fallback_chain(provider);
            let mut seen = std::collections::HashSet::new();
            for name in &chain {
                assert!(
                    seen.insert(name.clone()),
                    "duplicate provider '{name}' in fallback chain for '{provider}'"
                );
            }
        }
    }

    #[test]
    fn test_model_fallback_chain_opus() {
        let chain = model_fallback_chain("anthropic", "claude-opus-4-6");
        assert_eq!(chain[0], ("anthropic", "claude-opus-4-6"));
        assert!(chain.len() > 1);
        // Must have cross-provider fallbacks
        assert!(chain.iter().any(|(p, _)| *p != "anthropic"));
    }

    /// The newest Opus models degrade newest -> older within Anthropic before
    /// crossing providers. Pins the 2026-06 4.8/4.7 additions so the
    /// degradation order can't silently regress.
    #[test]
    fn test_model_fallback_chain_opus_4_8_and_4_7() {
        let chain = model_fallback_chain("anthropic", "claude-opus-4-8");
        assert_eq!(chain[0], ("anthropic", "claude-opus-4-8"));
        // Anthropic-internal degradation: 4.8 -> 4.7 -> 4.6 come first, in order.
        assert_eq!(chain[1], ("anthropic", "claude-opus-4-7"));
        assert_eq!(chain[2], ("anthropic", "claude-opus-4-6"));
        // Then cross-provider fallbacks.
        assert!(chain.iter().any(|(p, _)| *p != "anthropic"));

        let chain = model_fallback_chain("anthropic", "claude-opus-4-7");
        assert_eq!(chain[0], ("anthropic", "claude-opus-4-7"));
        assert_eq!(chain[1], ("anthropic", "claude-opus-4-6"));
        assert!(chain.iter().any(|(p, _)| *p != "anthropic"));
    }

    #[test]
    fn test_model_fallback_chain_gpt4o() {
        let chain = model_fallback_chain("openai", "gpt-4o");
        assert_eq!(chain[0], ("openai", "gpt-4o"));
        assert!(chain.len() > 1);
    }

    #[test]
    fn test_model_fallback_chain_unknown_model() {
        let chain = model_fallback_chain("openai", "totally-unknown-model");
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0], ("openai", "totally-unknown-model"));
    }

    #[test]
    fn test_fallback_result_type_exists() {
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
            original_model: "claude-opus-4-6".to_string(),
            actual_model: "gpt-5.2".to_string(),
            actual_provider: "openai".to_string(),
            was_fallback: true,
        };
        assert!(result.was_fallback);
        assert_ne!(result.original_model, result.actual_model);
    }

    /// Every `(provider, model_id)` primary that `model_fallback_chain` has a
    /// dedicated match arm for. Single source for the fallback tests below.
    ///
    /// **When you add a new arm to `model_fallback_chain`, add its key here** so
    /// `every_fallback_chain_id_is_registered` exercises it.
    fn fallback_chain_keys() -> Vec<(&'static str, &'static str)> {
        vec![
            ("anthropic", "claude-opus-4-8"),
            ("anthropic", "claude-opus-4-7"),
            ("anthropic", "claude-opus-4-6"),
            ("openai", "gpt-5.2"),
            ("google", "gemini-3.1-pro"),
            ("anthropic", "claude-sonnet-4-6"),
            ("openai", "gpt-4o"),
            ("openai", "gpt-4.1"),
            ("xai", "grok-3"),
            ("openai", "gpt-4o-mini"),
            ("openai", "gpt-4.1-mini"),
            ("openai", "gpt-4.1-nano"),
            ("anthropic", "claude-haiku-4-5-20251001"),
            ("google", "gemini-2.5-flash"),
            ("deepseek", "deepseek-chat"),
            ("openai", "o3"),
            ("openai", "o3-mini"),
            ("openai", "o4-mini"),
            ("deepseek", "deepseek-reasoner"),
        ]
    }

    /// CI guard: every model ID referenced by `model_fallback_chain` — each
    /// keyed primary AND every cross-provider fallback target it returns — must
    /// be a model registered in the production `config/models.toml`.
    ///
    /// Without this, a model renamed in `config/models.toml` (but not here)
    /// silently routes a fallback to an ID the gateway doesn't know, or — when
    /// the primary itself goes stale — drops to the `_ => vec![primary]` arm and
    /// loses cross-provider failover entirely. This is exactly the drift class
    /// that left a never-registered `claude-3-5-haiku-20241022` and the
    /// pre-#358 `claude-*-4-20250514` IDs lingering in these chains. Mirrors the
    /// router's `every_alias_and_profile_tier_resolves_to_a_registered_model`.
    #[test]
    fn every_fallback_chain_id_is_registered() {
        let toml_str = include_str!("../../../../config/models.toml");
        let registry = solvela_router::models::ModelRegistry::from_toml(toml_str)
            .expect("config/models.toml must parse");

        for (provider, model) in fallback_chain_keys() {
            let canonical = format!("{provider}/{model}");
            assert!(
                registry.get(&canonical).is_some(),
                "fallback chain key {canonical:?} is not in config/models.toml — \
                 a stale primary drops to primary-only failover"
            );
            for (p, m) in model_fallback_chain(provider, model) {
                let target = format!("{p}/{m}");
                assert!(
                    registry.get(&target).is_some(),
                    "fallback target {target:?} (in the chain for {canonical:?}) \
                     is not in config/models.toml — a stale ID routes failover to \
                     a model the gateway doesn't know"
                );
            }
        }
    }

    #[test]
    fn test_model_fallback_chain_no_self_duplicates() {
        let known_models = fallback_chain_keys();
        for (provider, model) in &known_models {
            let chain = model_fallback_chain(provider, model);
            let mut seen = std::collections::HashSet::new();
            for entry in &chain {
                assert!(
                    seen.insert(entry),
                    "duplicate entry {:?} in model fallback chain for ({}, {})",
                    entry,
                    provider,
                    model
                );
            }
        }
    }

    /// Proves the NVIDIA free-tier → Gemini fallback ACTUALLY fires for the
    /// exact canonical string `resolve_model(Profile::Free, _)` emits at runtime
    /// (the doubled-`nvidia/` form), and that the fallback target is a
    /// registered model. This is the load-bearing check: the other
    /// `model_fallback_chain` arms are keyed on BARE model_ids while the runtime
    /// passes the canonical `provider/model_id`, so they silently drop to
    /// primary-only. The NVIDIA arms are deliberately keyed on the canonical
    /// string so they actually match — this test pins that, and would catch any
    /// future drift between `profiles::resolve_model` and these keys.
    #[test]
    fn nvidia_free_models_fall_back_to_gemini() {
        use solvela_router::profiles::{resolve_model, Profile, Tier};

        let toml_str = include_str!("../../../../config/models.toml");
        let registry = ModelRegistry::from_toml(toml_str).expect("config/models.toml must parse");

        for tier in [Tier::Simple, Tier::Medium, Tier::Complex, Tier::Reasoning] {
            let target = resolve_model(Profile::Free, tier);
            assert!(
                target.starts_with("nvidia/"),
                "precondition: Profile::Free {tier:?} should route to NVIDIA, got {target:?}"
            );

            let chain = model_fallback_chain("nvidia", target);
            assert_eq!(
                chain.first(),
                Some(&("nvidia", target)),
                "primary must be first in the chain for {target:?}"
            );
            assert!(
                chain
                    .iter()
                    .any(|(p, m)| *p == "google" && *m == "gemini-3.1-flash-lite"),
                "Free NVIDIA model {target:?} must fall back to gemini-3.1-flash-lite, \
                 got chain {chain:?} (arm not firing → no graceful degradation)"
            );
        }

        // The fallback target must itself be a registered model.
        assert!(
            registry.get("google/gemini-3.1-flash-lite").is_some(),
            "fallback target google/gemini-3.1-flash-lite must be in config/models.toml"
        );
    }

    /// Coverage guard for the WHOLE free NVIDIA catalog — not just the 3 models
    /// `Profile::Free` happens to route to. Enumerates every NVIDIA entry in the
    /// production `config/models.toml`, derives the exact canonical runtime key
    /// (`provider/model_id`, the doubled-`nvidia/` form), and asserts:
    ///   * every FREE ($0 in AND $0 out) NVIDIA model falls back to the free
    ///     `google/gemini-3.1-flash-lite`, so NIM downtime degrades gracefully
    ///     instead of 503-ing;
    ///   * every PAID NVIDIA model keeps primary-only behavior (NO Gemini arm) —
    ///     a paid request must never silently fall back to a differently-priced
    ///     / $0 model;
    ///   * the Gemini fallback target is itself $0/$0, so a free model's degraded
    ///     path stays free.
    ///
    /// Without this, a new free NVIDIA model added to models.toml but not to the
    /// `model_fallback_chain` arm silently drops to the primary-only `_` arm and
    /// 503s when NIM is down — the exact gap this fixes.
    #[test]
    fn all_free_nvidia_models_fall_back_to_gemini_paid_do_not() {
        let toml_str = include_str!("../../../../config/models.toml");
        let registry = ModelRegistry::from_toml(toml_str).expect("config/models.toml must parse");

        let gemini = ("google", "gemini-3.1-flash-lite");

        // The fallback target must be free, else a free model's degraded path
        // would start billing.
        let target = registry
            .get("google/gemini-3.1-flash-lite")
            .expect("fallback target google/gemini-3.1-flash-lite must be registered");
        assert_eq!(
            (
                target.input_cost_per_million,
                target.output_cost_per_million
            ),
            (0.0, 0.0),
            "fallback target gemini-3.1-flash-lite must be $0/$0 so the degraded path stays free"
        );

        let mut free_seen = 0usize;
        let mut paid_seen = 0usize;
        for entry in registry.all() {
            if entry.provider != "nvidia" {
                continue;
            }
            // Canonical runtime key == what reaches model_fallback_chain.
            let canonical = format!("{}/{}", entry.provider, entry.model_id);
            let chain = model_fallback_chain("nvidia", &canonical);
            assert_eq!(
                chain.first(),
                Some(&("nvidia", canonical.as_str())),
                "primary must be first in the chain for {canonical:?}"
            );
            let falls_back_to_gemini = chain.iter().any(|(p, m)| (*p, *m) == (gemini.0, gemini.1));
            let is_free =
                entry.input_cost_per_million == 0.0 && entry.output_cost_per_million == 0.0;

            if is_free {
                free_seen += 1;
                assert!(
                    falls_back_to_gemini,
                    "FREE NVIDIA model {canonical:?} must fall back to gemini-3.1-flash-lite \
                     (got chain {chain:?}) — missing arm → 503 when NIM is down"
                );
            } else {
                paid_seen += 1;
                assert!(
                    !falls_back_to_gemini,
                    "PAID NVIDIA model {canonical:?} must NOT fall back to the free Gemini \
                     (got chain {chain:?}) — a paid request must never silently degrade to \
                     a differently-priced/$0 model"
                );
                // Paid models keep primary-only behavior.
                assert_eq!(
                    chain.len(),
                    1,
                    "PAID NVIDIA model {canonical:?} must keep primary-only fallback, got {chain:?}"
                );
            }
        }

        // Sanity: the catalog actually has both kinds, so the assertions above
        // were meaningfully exercised (catches an empty-iteration false pass).
        assert!(
            free_seen >= 15,
            "expected >=15 free NVIDIA models exercised, saw {free_seen}"
        );
        assert!(
            paid_seen >= 2,
            "expected >=2 paid NVIDIA models exercised, saw {paid_seen}"
        );
    }
}
