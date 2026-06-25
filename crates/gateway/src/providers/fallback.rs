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

    for (prov, model_id) in &chain {
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

        let start = Instant::now();
        match provider.chat_completion(model_req).await {
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

    for (prov, model_id) in &chain {
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

        let start = Instant::now();
        match provider.chat_completion_stream(model_req).await {
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
        for provider in &["openai", "anthropic", "google", "deepseek", "xai"] {
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
}
