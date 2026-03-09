//! Provider fallback logic.
//!
//! When the primary provider fails or its circuit is open,
//! tries the next provider in the fallback chain.

use std::time::Instant;

use tracing::{info, warn};

use rcr_common::types::{ChatRequest, ChatResponse};

use super::health::ProviderHealthTracker;
use super::{ChatStream, ProviderError, ProviderRegistry};

/// Execute a chat completion with fallback.
///
/// Tries providers in the given order, skipping any whose circuit is open.
/// Records success/failure metrics for circuit breaker evaluation.
pub async fn chat_with_fallback(
    providers: &ProviderRegistry,
    health: &ProviderHealthTracker,
    provider_names: &[String],
    req: ChatRequest,
) -> Result<ChatResponse, ProviderError> {
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
pub async fn stream_with_fallback(
    providers: &ProviderRegistry,
    health: &ProviderHealthTracker,
    provider_names: &[String],
    req: ChatRequest,
) -> Result<ChatStream, ProviderError> {
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let chain = model_fallback_chain("anthropic", "claude-opus-4.6");
        assert_eq!(chain[0], ("anthropic", "claude-opus-4.6"));
        assert!(chain.len() > 1);
        // Must have cross-provider fallbacks
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
    fn test_model_fallback_chain_no_self_duplicates() {
        let known_models: Vec<(&str, &str)> = vec![
            ("anthropic", "claude-opus-4.6"),
            ("openai", "gpt-5.2"),
            ("google", "gemini-3.1-pro"),
            ("anthropic", "claude-sonnet-4.6"),
            ("anthropic", "claude-sonnet-4.5"),
            ("openai", "gpt-4o"),
            ("openai", "gpt-4.1"),
            ("xai", "grok-3"),
            ("openai", "gpt-4o-mini"),
            ("openai", "gpt-4.1-mini"),
            ("openai", "gpt-4.1-nano"),
            ("anthropic", "claude-haiku-4.5"),
            ("google", "gemini-2.5-flash"),
            ("deepseek", "deepseek-chat"),
            ("openai", "o3"),
            ("openai", "o3-mini"),
            ("openai", "o4-mini"),
            ("deepseek", "deepseek-reasoner"),
        ];
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
