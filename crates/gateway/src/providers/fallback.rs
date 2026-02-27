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
}
