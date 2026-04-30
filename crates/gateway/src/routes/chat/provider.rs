//! Provider call logic: cache lookup, fallback chains, streaming SSE
//! construction, and debug header attachment.
//!
//! Contains the shared provider-call pipeline used by both the main
//! `chat_completions` handler (paid path) and the dev-bypass handler.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;

use axum::http::{HeaderMap, HeaderName, HeaderValue};
use axum::response::{sse, IntoResponse, Response};
use axum::Json;
use futures::StreamExt;
use metrics::{counter, histogram};
use tracing::info;

use solvela_protocol::ChatRequest;

use crate::providers::fallback;
use crate::providers::heartbeat::{HeartbeatConfig, HeartbeatItem, HeartbeatStream};
use crate::providers::stream_caps::{CappedSizedStream, StreamCapConfig};
use crate::routes::debug_headers::{attach_debug_headers, CacheStatus, PaymentStatus};
use crate::AppState;

use super::cost::estimate_input_tokens;
use super::response::{attach_session_id, build_debug_info};

/// Classify a provider error into a bounded set of label values for metrics.
///
/// Returns one of: `"timeout"`, `"auth"`, `"rate_limit"`, `"server_error"`, `"unknown"`.
/// Cardinality is fixed -- never use the raw error message as a label.
pub(crate) fn classify_provider_error(err: &impl std::fmt::Display) -> &'static str {
    let msg = err.to_string().to_lowercase();
    if msg.contains("timeout") || msg.contains("timed out") {
        "timeout"
    } else if msg.contains("401") || msg.contains("unauthorized") || msg.contains("auth") {
        "auth"
    } else if msg.contains("429") || msg.contains("rate") || msg.contains("too many") {
        "rate_limit"
    } else if msg.contains("500")
        || msg.contains("502")
        || msg.contains("503")
        || msg.contains("504")
    {
        "server_error"
    } else {
        "unknown"
    }
}

/// Parse the `X-Solvela-Fallback-Preference` (or `X-RCR-Fallback-Preference`) header value.
///
/// Format: `"provider/model,provider/model,..."`
/// Returns `(provider, model)` tuples. Invalid entries are silently skipped.
pub(crate) fn parse_fallback_preference(header: &str) -> Vec<(&str, &str)> {
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

/// Context for a provider call -- captures all routing/debug metadata that
/// both the paid and dev-bypass paths need.
pub(crate) struct ProviderCallContext<'a> {
    pub state: &'a Arc<AppState>,
    pub req: &'a ChatRequest,
    pub model_info: &'a solvela_protocol::ModelInfo,
    pub headers: &'a HeaderMap,
    pub debug_enabled: bool,
    pub request_start: Instant,
    pub routing_tier: &'a str,
    pub routing_score: f64,
    pub routing_profile: &'a str,
    pub session_id: &'a Option<String>,
    pub payment_status: PaymentStatus,
}

/// Metadata returned alongside the HTTP response from a provider call.
///
/// Contains information needed for post-response processing (usage logging,
/// escrow claims) that would otherwise be lost once the response body is sealed.
pub(crate) struct ProviderCallResult {
    pub response: Response,
    /// Actual token usage from the provider (non-streaming only).
    pub usage: Option<solvela_protocol::Usage>,
    /// The provider that actually served the request (may differ from primary due to fallback).
    pub actual_provider: Option<String>,
}

/// Execute the provider call pipeline: cache lookup, fallback chain,
/// streaming/non-streaming response construction, and debug headers.
///
/// This is the shared core used by both the main `chat_completions` handler
/// (paid requests) and the dev-bypass handler, eliminating the ~200 lines
/// of duplicated logic between the two paths.
///
/// Returns a [`ProviderCallResult`] containing the HTTP response and optional
/// metadata for post-response processing.
/// Provider errors that exhaust all fallbacks return `Err`.
pub(crate) async fn execute_provider_call(
    ctx: &ProviderCallContext<'_>,
) -> Result<ProviderCallResult, ProviderCallError> {
    let provider_name = &ctx.model_info.provider;

    let mut cache_status = if ctx.req.stream {
        counter!("solvela_cache_total", "result" => "skip").increment(1);
        CacheStatus::Skip
    } else {
        CacheStatus::Miss
    };

    // Check cache first (only for non-streaming requests)
    if !ctx.req.stream {
        if let Some(cache) = &ctx.state.cache {
            if let Some(cached) = cache.get(ctx.req).await {
                counter!("solvela_cache_total", "result" => "hit").increment(1);
                info!(model = %ctx.req.model, "serving from cache");
                cache_status = CacheStatus::Hit;
                let mut resp = Json(
                    serde_json::to_value(&cached)
                        .map_err(|e| ProviderCallError::Internal(e.to_string()))?,
                )
                .into_response();
                attach_session_id(&mut resp, ctx.session_id);
                if ctx.debug_enabled {
                    attach_debug_headers(
                        &mut resp,
                        &build_debug_info(
                            &ctx.req.model,
                            ctx.routing_tier,
                            ctx.routing_score,
                            ctx.routing_profile,
                            provider_name,
                            cache_status,
                            ctx.request_start.elapsed().as_millis() as u64,
                            ctx.payment_status,
                            estimate_input_tokens(ctx.req),
                            ctx.req.max_tokens.unwrap_or(1000),
                        ),
                    );
                }
                return Ok(ProviderCallResult {
                    response: resp,
                    usage: cached.usage.clone(),
                    actual_provider: None,
                });
            }
            counter!("solvela_cache_total", "result" => "miss").increment(1);
        } else {
            counter!("solvela_cache_total", "result" => "miss").increment(1);
        }
    }

    // Check for agent-specified fallback preferences (accept both new and legacy headers)
    let fallback_pref = ctx
        .headers
        .get("x-solvela-fallback-preference")
        .or_else(|| ctx.headers.get("x-rcr-fallback-preference"))
        .and_then(|v| v.to_str().ok());

    if ctx.req.stream {
        let resp = execute_streaming_call(ctx, provider_name, fallback_pref, cache_status).await?;
        Ok(ProviderCallResult {
            response: resp,
            usage: None, // streaming doesn't have usage data
            actual_provider: None,
        })
    } else {
        execute_non_streaming_call(ctx, provider_name, fallback_pref, cache_status).await
    }
}

/// Errors from the provider call pipeline.
pub(crate) enum ProviderCallError {
    /// All providers/fallbacks failed.
    AllProvidersFailed {
        model: String,
        provider: String,
        error: String,
    },
    /// Internal serialization or other error.
    Internal(String),
}

/// Build a fallback chain from the preference header + primary provider.
fn build_fallback_chain(
    provider_name: &str,
    model: &str,
    fallback_pref: Option<&str>,
) -> Option<Vec<(String, String)>> {
    fallback_pref.map(|pref| {
        let mut chain: Vec<(String, String)> = vec![(provider_name.to_string(), model.to_string())];
        for (p, m) in parse_fallback_preference(pref) {
            let entry = (p.to_string(), m.to_string());
            if !chain.contains(&entry) {
                chain.push(entry);
            }
        }
        chain
    })
}

async fn execute_streaming_call(
    ctx: &ProviderCallContext<'_>,
    provider_name: &str,
    fallback_pref: Option<&str>,
    cache_status: CacheStatus,
) -> Result<Response, ProviderCallError> {
    info!(provider = provider_name, model = %ctx.req.model, "streaming to provider (with model fallback)");

    let provider_start = Instant::now();
    let result =
        if let Some(chain) = build_fallback_chain(provider_name, &ctx.req.model, fallback_pref) {
            fallback::stream_with_chain(
                &ctx.state.providers,
                &ctx.state.provider_health,
                &chain,
                &ctx.req.model,
                ctx.req.clone(),
            )
            .await
        } else {
            fallback::stream_with_model_fallback(
                &ctx.state.providers,
                &ctx.state.provider_health,
                provider_name,
                &ctx.req.model,
                ctx.req.clone(),
            )
            .await
        };

    let provider_duration = provider_start.elapsed();
    histogram!("solvela_provider_request_duration_seconds", "provider" => provider_name.to_string())
        .record(provider_duration.as_secs_f64());

    match result {
        Ok(result) => {
            // Wrap with adaptive heartbeat
            let heartbeat_stream = HeartbeatStream::new(result.data, HeartbeatConfig::default());

            // S3 FIX: Generic error message instead of raw provider errors.
            // Error envelope mirrors the OpenAI format
            //   { "error": { "type", "code", "message" } }
            // so SDK clients can detect failures with a single shape regardless
            // of which upstream provider produced the error. Provider-specific
            // detail stays in the structured log, never on the wire.
            //
            // Each item is paired with its serialized byte length so the
            // downstream `CappedSizedStream` can enforce the `SOLVELA_STREAM_MAX_BYTES`
            // cumulative-body cap accurately.
            let model_for_err = ctx.req.model.clone();
            let sse_stream = heartbeat_stream.map(move |item| match item {
                HeartbeatItem::Chunk(Ok(chunk)) => {
                    let json = serde_json::to_string(&chunk).unwrap_or_default();
                    let bytes = json.len();
                    (
                        bytes,
                        Ok::<_, Infallible>(sse::Event::default().data(json)),
                    )
                }
                HeartbeatItem::Chunk(Err(e)) => {
                    tracing::error!(error = %e, "stream chunk error (details redacted from client)");
                    let envelope = serde_json::json!({
                        "error": {
                            "type": "upstream_error",
                            "code": "stream_error",
                            "message": format!("[{}] stream processing error", model_for_err),
                        }
                    });
                    let body = envelope.to_string();
                    let bytes = body.len();
                    (bytes, Ok(sse::Event::default().data(body)))
                }
                HeartbeatItem::KeepAlive => {
                    // Keep-alive comments don't count against the byte cap —
                    // they're framing overhead, not LLM output.
                    (0, Ok(sse::Event::default().comment("keep-alive")))
                }
            });

            // Hard caps: 5-minute wallclock + 5 MiB cumulative body (env-tunable).
            let capped = CappedSizedStream::new(sse_stream, StreamCapConfig::from_env());

            let mut resp = sse::Sse::new(capped).into_response();

            // Add fallback header if served by a different model
            if result.was_fallback {
                let fallback_value =
                    format!("{} -> {}", result.original_model, result.actual_model);
                if let Ok(hv) = HeaderValue::from_str(&fallback_value) {
                    resp.headers_mut()
                        .insert(HeaderName::from_static("x-solvela-fallback"), hv.clone());
                    resp.headers_mut()
                        .insert(HeaderName::from_static("x-rcr-fallback"), hv);
                }
            }

            attach_session_id(&mut resp, ctx.session_id);

            if ctx.debug_enabled {
                attach_debug_headers(
                    &mut resp,
                    &build_debug_info(
                        &ctx.req.model,
                        ctx.routing_tier,
                        ctx.routing_score,
                        ctx.routing_profile,
                        provider_name,
                        cache_status,
                        ctx.request_start.elapsed().as_millis() as u64,
                        ctx.payment_status,
                        estimate_input_tokens(ctx.req),
                        ctx.req.max_tokens.unwrap_or(1000),
                    ),
                );
            }

            Ok(resp)
        }
        Err(e) => {
            let error_type = classify_provider_error(&e);
            counter!("solvela_provider_errors_total", "provider" => provider_name.to_string(), "error_type" => error_type).increment(1);
            Err(ProviderCallError::AllProvidersFailed {
                model: ctx.req.model.clone(),
                provider: provider_name.to_string(),
                error: e.to_string(),
            })
        }
    }
}

async fn execute_non_streaming_call(
    ctx: &ProviderCallContext<'_>,
    provider_name: &str,
    fallback_pref: Option<&str>,
    cache_status: CacheStatus,
) -> Result<ProviderCallResult, ProviderCallError> {
    info!(provider = provider_name, model = %ctx.req.model, "proxying to provider (with model fallback)");

    let provider_start = Instant::now();
    let result =
        if let Some(chain) = build_fallback_chain(provider_name, &ctx.req.model, fallback_pref) {
            fallback::chat_with_chain(
                &ctx.state.providers,
                &ctx.state.provider_health,
                &chain,
                &ctx.req.model,
                ctx.req.clone(),
            )
            .await
        } else {
            fallback::chat_with_model_fallback(
                &ctx.state.providers,
                &ctx.state.provider_health,
                provider_name,
                &ctx.req.model,
                ctx.req.clone(),
            )
            .await
        };

    let provider_duration = provider_start.elapsed();
    histogram!("solvela_provider_request_duration_seconds", "provider" => provider_name.to_string())
        .record(provider_duration.as_secs_f64());

    match result {
        Ok(result) => {
            // Cache the response
            if let Some(cache) = &ctx.state.cache {
                cache.set(ctx.req, &result.data).await;
            }

            let response_json = serde_json::to_value(&result.data)
                .map_err(|e| ProviderCallError::Internal(e.to_string()))?;
            let mut resp = Json(response_json).into_response();

            // Add fallback header if served by a different model
            if result.was_fallback {
                let fallback_value =
                    format!("{} -> {}", result.original_model, result.actual_model);
                if let Ok(hv) = HeaderValue::from_str(&fallback_value) {
                    resp.headers_mut()
                        .insert(HeaderName::from_static("x-solvela-fallback"), hv.clone());
                    resp.headers_mut()
                        .insert(HeaderName::from_static("x-rcr-fallback"), hv);
                }
            }

            attach_session_id(&mut resp, ctx.session_id);

            if ctx.debug_enabled {
                let actual_tokens_out = result
                    .data
                    .usage
                    .as_ref()
                    .map(|u| u.completion_tokens)
                    .unwrap_or(ctx.req.max_tokens.unwrap_or(1000));
                let actual_tokens_in = result
                    .data
                    .usage
                    .as_ref()
                    .map(|u| u.prompt_tokens)
                    .unwrap_or(estimate_input_tokens(ctx.req));
                attach_debug_headers(
                    &mut resp,
                    &build_debug_info(
                        &ctx.req.model,
                        ctx.routing_tier,
                        ctx.routing_score,
                        ctx.routing_profile,
                        provider_name,
                        cache_status,
                        ctx.request_start.elapsed().as_millis() as u64,
                        ctx.payment_status,
                        actual_tokens_in,
                        actual_tokens_out,
                    ),
                );
            }

            Ok(ProviderCallResult {
                response: resp,
                usage: result.data.usage.clone(),
                actual_provider: Some(result.actual_provider.clone()),
            })
        }
        Err(e) => {
            let error_type = classify_provider_error(&e);
            counter!("solvela_provider_errors_total", "provider" => provider_name.to_string(), "error_type" => error_type).increment(1);
            Err(ProviderCallError::AllProvidersFailed {
                model: ctx.req.model.clone(),
                provider: provider_name.to_string(),
                error: e.to_string(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // parse_fallback_preference
    // =========================================================================

    #[test]
    fn test_parse_fallback_preference_valid() {
        let prefs = parse_fallback_preference("openai/gpt-4.1,anthropic/claude-sonnet-4-20250514");
        assert_eq!(prefs.len(), 2);
        assert_eq!(prefs[0], ("openai", "gpt-4.1"));
        assert_eq!(prefs[1], ("anthropic", "claude-sonnet-4-20250514"));
    }

    #[test]
    fn test_parse_fallback_preference_empty() {
        let prefs = parse_fallback_preference("");
        assert!(prefs.is_empty());
    }

    #[test]
    fn test_parse_fallback_preference_invalid_entries_skipped() {
        let prefs =
            parse_fallback_preference("openai/gpt-4.1,invalid,anthropic/claude-sonnet-4-20250514");
        assert_eq!(prefs.len(), 2);
    }

    #[test]
    fn test_parse_fallback_preference_whitespace_trimmed() {
        let prefs =
            parse_fallback_preference(" openai/gpt-4.1 , anthropic/claude-sonnet-4-20250514 ");
        assert_eq!(prefs.len(), 2);
        assert_eq!(prefs[0], ("openai", "gpt-4.1"));
    }

    // -------------------------------------------------------------------------
    // classify_provider_error
    // -------------------------------------------------------------------------

    #[test]
    fn test_classify_provider_error_timeout() {
        assert_eq!(classify_provider_error(&"connection timeout"), "timeout");
        assert_eq!(classify_provider_error(&"request timed out"), "timeout");
    }

    #[test]
    fn test_classify_provider_error_auth() {
        assert_eq!(classify_provider_error(&"HTTP 401 Unauthorized"), "auth");
        assert_eq!(
            classify_provider_error(&"unauthorized: invalid API key"),
            "auth"
        );
        assert_eq!(classify_provider_error(&"auth error"), "auth");
    }

    #[test]
    fn test_classify_provider_error_rate_limit() {
        assert_eq!(
            classify_provider_error(&"HTTP 429 Too Many Requests"),
            "rate_limit"
        );
        assert_eq!(
            classify_provider_error(&"rate limit exceeded"),
            "rate_limit"
        );
        assert_eq!(classify_provider_error(&"too many requests"), "rate_limit");
    }

    #[test]
    fn test_classify_provider_error_server_error() {
        assert_eq!(
            classify_provider_error(&"HTTP 500 Internal Server Error"),
            "server_error"
        );
        assert_eq!(classify_provider_error(&"502 Bad Gateway"), "server_error");
        assert_eq!(
            classify_provider_error(&"503 Service Unavailable"),
            "server_error"
        );
        assert_eq!(
            classify_provider_error(&"504 Gateway Error"),
            "server_error"
        );
        // "504 Gateway Timeout" matches "timeout" first -- intentional;
        // the timeout bucket is more operationally specific.
        assert_eq!(classify_provider_error(&"504 Gateway Timeout"), "timeout");
    }

    #[test]
    fn test_classify_provider_error_unknown() {
        assert_eq!(classify_provider_error(&"something went wrong"), "unknown");
        assert_eq!(classify_provider_error(&"connection refused"), "unknown");
    }
}
