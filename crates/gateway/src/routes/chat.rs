use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{sse, IntoResponse, Response};
use axum::Json;
use futures::StreamExt;
use tracing::{info, warn};

use rcr_common::types::ChatRequest;
use router::profiles::{self, Profile};
use router::scorer;

use crate::error::GatewayError;
use crate::middleware::prompt_guard::{self, GuardResult, PromptGuardConfig};
use crate::providers::fallback;
use crate::AppState;

/// POST /v1/chat/completions — OpenAI-compatible chat endpoint.
///
/// Flow:
/// 1. Parse request, resolve model (aliases, smart routing)
/// 2. Check for PAYMENT-SIGNATURE header
/// 3. If missing → return 402 Payment Required with cost breakdown
/// 4. If present → verify payment via Facilitator → proxy to provider → return response
/// 5. Support both JSON and SSE streaming responses
pub async fn chat_completions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(mut req): Json<ChatRequest>,
) -> Result<Response, GatewayError> {
    // Step 1: Resolve model — handle aliases and smart routing profiles
    let original_model = req.model.clone();
    req.model = resolve_model_id(&req, &state)?;

    info!(
        original_model,
        resolved_model = %req.model,
        messages = req.messages.len(),
        stream = req.stream,
        "chat completion request"
    );

    // Step 1b: Prompt guard — check for injection, jailbreak, and PII
    let guard_config = PromptGuardConfig::default();
    match prompt_guard::check(&req.messages, &guard_config) {
        GuardResult::Blocked { reason } => {
            warn!(reason = %reason, "request blocked by prompt guard");
            return Err(GatewayError::BadRequest(format!(
                "request blocked: {reason}"
            )));
        }
        GuardResult::PiiDetected { fields } => {
            // Log only (pii_block=false by default); do not reject
            warn!(
                pii_fields = ?fields,
                "PII detected in request — forwarding with warning logged"
            );
        }
        GuardResult::Clean => {}
    }

    // Step 2: Look up model in registry for pricing
    let model_info = state
        .model_registry
        .get(&req.model)
        .ok_or_else(|| GatewayError::ModelNotFound(req.model.clone()))?;

    // Step 3: Check for payment
    let payment_header = headers
        .get("payment-signature")
        .and_then(|v| v.to_str().ok());

    if payment_header.is_none() {
        // Return 402 with pricing info
        info!(model = %req.model, "no payment signature, returning 402");

        let cost = state
            .model_registry
            .estimate_cost(
                &req.model,
                estimate_input_tokens(&req),
                req.max_tokens.unwrap_or(1000),
            )
            .map_err(|e| GatewayError::Internal(e.to_string()))?;

        let payment_required = x402::types::PaymentRequired {
            x402_version: x402::types::X402_VERSION,
            resource: x402::types::Resource {
                url: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
            },
            accepts: vec![x402::types::PaymentAccept {
                scheme: "exact".to_string(),
                network: x402::types::SOLANA_NETWORK.to_string(),
                amount: usdc_atomic_amount(&cost.total),
                asset: x402::types::USDC_MINT.to_string(),
                pay_to: state.config.solana.recipient_wallet.clone(),
                max_timeout_seconds: x402::types::MAX_TIMEOUT_SECONDS,
            }],
            cost_breakdown: cost,
            error: "Payment required".to_string(),
        };

        return Err(GatewayError::InvalidPayment(
            serde_json::to_string(&payment_required)
                .unwrap_or_else(|_| "payment required".to_string()),
        ));
    }

    // Step 4: Payment present — try to decode and verify via Facilitator
    let payment_payload = decode_payment_from_header(payment_header.unwrap());

    match payment_payload {
        Some(payload) => {
            // Verify the resource field matches this endpoint
            if payload.resource.url != "/v1/chat/completions" {
                return Err(GatewayError::InvalidPayment(format!(
                    "payment resource '{}' does not match this endpoint",
                    payload.resource.url
                )));
            }

            // Replay attack prevention: atomically record this transaction
            // signature in Redis before verifying. If it was already seen,
            // reject immediately — same signed tx cannot be replayed.
            if let Some(cache) = &state.cache {
                let tx_raw = &payload.payload.transaction;
                if cache.check_and_record_tx(tx_raw).await.is_err() {
                    warn!(tx = %tx_raw, "replay attack detected — transaction already used");
                    return Err(GatewayError::InvalidPayment(
                        "transaction has already been used; each payment signature may only be submitted once".to_string()
                    ));
                }
            }

            // Verify and settle via Facilitator — hard enforcement
            match state.facilitator.verify_and_settle(&payload).await {
                Ok(settlement) => {
                    info!(
                        tx_signature = ?settlement.tx_signature,
                        network = %settlement.network,
                        "payment verified and settled"
                    );
                }
                Err(e) => {
                    // Payment verification failed — reject the request
                    warn!(error = %e, "payment verification failed");
                    return Err(GatewayError::InvalidPayment(format!(
                        "payment verification failed: {e}"
                    )));
                }
            }
        }
        None => {
            // Header present but could not be decoded — reject with a clear error.
            // A malformed header is not a valid payment; never serve for free.
            return Err(GatewayError::InvalidPayment(
                "PAYMENT-SIGNATURE header is present but could not be decoded. \
                 Encode a valid PaymentPayload as standard base64 JSON."
                    .to_string(),
            ));
        }
    }

    // Extract tx_signature for usage tracking.
    // Note: `accepted.pay_to` is the gateway's own recipient wallet.
    // The payer identity is the transaction fee payer (first account in the tx),
    // which is not decoded here — we use the tx signature as a unique identifier.
    let (wallet_address, tx_signature) = extract_payment_info(payment_header.unwrap());

    // Check budget before proxying to provider
    let estimated_cost = state
        .model_registry
        .estimate_cost(
            &req.model,
            estimate_input_tokens(&req),
            req.max_tokens.unwrap_or(1000),
        )
        .map(|c| c.total.parse::<f64>().unwrap_or(0.0))
        .unwrap_or(0.0);

    if let Err(e) = state
        .usage
        .check_budget(&wallet_address, estimated_cost)
        .await
    {
        return Err(GatewayError::BadRequest(e.to_string()));
    }

    // Step 5: Proxy to provider (with cache and fallback)
    let provider_name = &model_info.provider;

    // Check cache first (only for non-streaming requests)
    if !req.stream {
        if let Some(cache) = &state.cache {
            if let Some(cached) = cache.get(&req).await {
                info!(model = %req.model, "serving from cache");
                return Ok(Json(
                    serde_json::to_value(&cached)
                        .map_err(|e| GatewayError::Internal(e.to_string()))?,
                )
                .into_response());
            }
        }
    }

    // Build fallback chain for this provider
    let fallback_providers = fallback::fallback_chain(provider_name);

    if req.stream {
        // Streaming response via SSE with fallback
        info!(provider = provider_name, model = %req.model, "streaming to provider (with fallback)");

        match fallback::stream_with_fallback(
            &state.providers,
            &state.provider_health,
            &fallback_providers,
            req.clone(),
        )
        .await
        {
            Ok(stream) => {
                let sse_stream = stream.map(|result| match result {
                    Ok(chunk) => {
                        let json = serde_json::to_string(&chunk).unwrap_or_default();
                        Ok::<_, Infallible>(sse::Event::default().data(json))
                    }
                    Err(e) => {
                        warn!(error = %e, "stream chunk error");
                        Ok(sse::Event::default().data(format!("{{\"error\": \"{e}\"}}")))
                    }
                });
                return Ok(sse::Sse::new(sse_stream).into_response());
            }
            Err(_) => {
                // All providers failed or none configured — fall through to stub
            }
        }
    } else {
        // Non-streaming JSON response with fallback
        info!(provider = provider_name, model = %req.model, "proxying to provider (with fallback)");

        match fallback::chat_with_fallback(
            &state.providers,
            &state.provider_health,
            &fallback_providers,
            req.clone(),
        )
        .await
        {
            Ok(response) => {
                // Cache the response (async, non-blocking)
                if let Some(cache) = &state.cache {
                    cache.set(&req, &response).await;
                }

                // Log spend asynchronously
                if let Some(usage) = &response.usage {
                    let cost = state
                        .model_registry
                        .estimate_cost(&req.model, usage.prompt_tokens, usage.completion_tokens)
                        .map(|c| c.total.parse::<f64>().unwrap_or(0.0))
                        .unwrap_or(0.0);

                    state.usage.log_spend(
                        wallet_address.clone(),
                        req.model.clone(),
                        provider_name.to_string(),
                        usage.prompt_tokens,
                        usage.completion_tokens,
                        cost,
                        tx_signature.clone(),
                    );
                }

                let response_json = serde_json::to_value(&response)
                    .map_err(|e| GatewayError::Internal(e.to_string()))?;
                return Ok(Json(response_json).into_response());
            }
            Err(_) => {
                // All providers failed or none configured — fall through to stub
            }
        }
    }

    // Fallback: stub response when no provider succeeded
    info!(
        provider = provider_name,
        model = %req.model,
        "no provider available, returning stub response"
    );

    let response = serde_json::json!({
        "id": format!("chatcmpl-{}", uuid::Uuid::new_v4()),
        "object": "chat.completion",
        "created": chrono::Utc::now().timestamp(),
        "model": model_info.id,
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": format!(
                    "[STUB] {} provider not configured. Set {}_API_KEY env var to enable.",
                    model_info.display_name,
                    provider_name.to_uppercase()
                ),
            },
            "finish_reason": "stop",
        }],
        "usage": {
            "prompt_tokens": estimate_input_tokens(&req),
            "completion_tokens": 20,
            "total_tokens": estimate_input_tokens(&req) + 20,
        },
    });

    Ok(Json(response).into_response())
}

/// Try to decode a PaymentPayload from the PAYMENT-SIGNATURE header.
///
/// Returns `None` if decoding fails — this is intentional for backwards
/// compatibility with raw string headers used in tests (e.g., "fake-payment-for-testing").
fn decode_payment_from_header(header: &str) -> Option<x402::types::PaymentPayload> {
    use base64::Engine;

    // Try base64-encoded JSON
    if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(header) {
        if let Ok(json_str) = String::from_utf8(decoded) {
            if let Ok(payload) = serde_json::from_str(&json_str) {
                return Some(payload);
            }
        }
    }

    // Try URL-safe base64
    if let Ok(decoded) = base64::engine::general_purpose::URL_SAFE.decode(header) {
        if let Ok(json_str) = String::from_utf8(decoded) {
            if let Ok(payload) = serde_json::from_str(&json_str) {
                return Some(payload);
            }
        }
    }

    // Try raw JSON
    if let Ok(payload) = serde_json::from_str(header) {
        return Some(payload);
    }

    None
}

/// Extract wallet address and transaction signature from the payment header.
///
/// If the header is a valid PaymentPayload, extracts the `pay_to` address and
/// uses the transaction signature. Otherwise returns "unknown" / None.
fn extract_payment_info(header: &str) -> (String, Option<String>) {
    match decode_payment_from_header(header) {
        Some(payload) => {
            let wallet = payload.accepted.pay_to.clone();
            let tx_sig = Some(payload.payload.transaction.clone());
            (wallet, tx_sig)
        }
        None => ("unknown".to_string(), None),
    }
}

/// Resolve model ID from aliases, smart routing profiles, or direct model IDs.
fn resolve_model_id(req: &ChatRequest, state: &AppState) -> Result<String, GatewayError> {
    // Check for profile-based routing (e.g., "auto", "eco", "premium")
    if let Some(profile) = Profile::from_alias(&req.model) {
        let result = scorer::classify(&req.messages, false);
        let model_id = profiles::resolve_model(profile, result.tier);
        return Ok(model_id.to_string());
    }

    // Check for model aliases (e.g., "gpt5", "sonnet")
    if let Some(canonical) = profiles::resolve_alias(&req.model) {
        return Ok(canonical.to_string());
    }

    // Check if it's a direct model ID
    if state.model_registry.get(&req.model).is_some() {
        return Ok(req.model.clone());
    }

    Err(GatewayError::ModelNotFound(req.model.clone()))
}

/// Rough token estimate: ~4 chars per token.
fn estimate_input_tokens(req: &ChatRequest) -> u32 {
    let chars: usize = req.messages.iter().map(|m| m.content.len()).sum();
    (chars / 4).max(1) as u32
}

/// Convert a USDC decimal string to atomic units (6 decimals).
///
/// Uses integer arithmetic to avoid f64 precision loss on financial amounts.
/// Splits on the decimal point and pads/truncates the fractional part to 6 digits.
fn usdc_atomic_amount(decimal_str: &str) -> String {
    // Parse as integer arithmetic to avoid f64 rounding errors
    let s = decimal_str.trim();
    let (integer_part, fractional_part) = if let Some(dot) = s.find('.') {
        (&s[..dot], &s[dot + 1..])
    } else {
        (s, "")
    };

    let integer: u64 = integer_part.parse().unwrap_or(0);
    // Pad or truncate fractional part to exactly 6 digits
    let frac_padded = format!("{:0<6}", fractional_part);
    let frac_6 = &frac_padded[..6.min(frac_padded.len())];
    let fractional: u64 = frac_6.parse().unwrap_or(0);

    let atomic = integer * 1_000_000 + fractional;
    atomic.to_string()
}
