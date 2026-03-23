//! POST /v1/chat/completions — OpenAI-compatible chat endpoint.
//!
//! Submodules:
//! - [`cost`] — USDC computation and token estimation
//! - [`payment`] — Payment extraction, validation, escrow claims
//! - [`provider`] — Shared provider call pipeline (cache, fallback, SSE)
//! - [`response`] — Debug headers, session tokens, response construction

mod cost;
mod payment;
mod provider;
mod response;

use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use axum::response::Response;
use axum::Json;
use metrics::{counter, histogram};
use tracing::{info, warn};

use router::profiles::{self, Profile};
use router::scorer;
use rustyclaw_protocol::ChatRequest;

use crate::error::GatewayError;
use crate::middleware::prompt_guard::{self, GuardResult, PromptGuardConfig};
use crate::routes::debug_headers::{is_debug_enabled, PaymentStatus};
use crate::usage::SpendLogEntry;
use crate::AppState;

use cost::{
    compute_actual_atomic_cost, estimate_input_tokens, estimated_atomic_cost,
    usdc_atomic_amount_checked,
};
use payment::{decode_payment_from_header, extract_payment_info, fire_escrow_claim};
use provider::{ProviderCallContext, ProviderCallError, ProviderCallResult};
use response::{build_session_token, validate_session_id};

// Re-export `uses_durable_nonce` for use by `crate::routes::proxy`
pub(crate) use payment::uses_durable_nonce;

/// Maximum number of messages allowed in a single chat request.
/// Prevents excessive memory usage and cost from very long conversations.
const MAX_MESSAGES: usize = 256;

/// Platform-wide upper bound for `max_tokens` to prevent unbounded cost exposure.
const MAX_TOKENS_LIMIT: u32 = 128_000;

/// POST /v1/chat/completions — OpenAI-compatible chat endpoint.
///
/// Flow:
/// 1. Parse request, resolve model (aliases, smart routing)
/// 2. Check for PAYMENT-SIGNATURE header
/// 3. If missing -> return 402 Payment Required with cost breakdown
/// 4. If present -> verify payment via Facilitator -> proxy to provider -> return response
/// 5. Support both JSON and SSE streaming responses
pub async fn chat_completions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(mut req): Json<ChatRequest>,
) -> Result<Response, GatewayError> {
    let request_start = Instant::now();
    let debug_enabled = is_debug_enabled(&headers);

    // Validate message count before any processing
    if req.messages.len() > MAX_MESSAGES {
        return Err(GatewayError::BadRequest(format!(
            "too many messages: {} exceeds maximum of {}",
            req.messages.len(),
            MAX_MESSAGES
        )));
    }

    // Extract request ID from the incoming header
    let request_id: Option<String> = headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    // Extract and validate X-Session-Id header
    let session_id: Option<String> = headers
        .get("x-session-id")
        .and_then(|v| v.to_str().ok())
        .and_then(validate_session_id);

    // Step 1: Resolve model — handle aliases and smart routing profiles
    let original_model = req.model.clone();
    let (resolved_model, routing_profile, routing_tier, routing_score) =
        resolve_model_with_debug(&req, &state)?;
    req.model = resolved_model;

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
            return Err(GatewayError::BadRequest(
                "Request blocked by content policy".to_string(),
            ));
        }
        GuardResult::PiiDetected { fields } => {
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

    // Step 2b: Clamp max_tokens to model/platform limit to prevent unbounded cost
    if let Some(requested_max) = req.max_tokens {
        let model_limit = model_info.max_output_tokens.unwrap_or(MAX_TOKENS_LIMIT);
        let effective_limit = model_limit.min(MAX_TOKENS_LIMIT);
        if requested_max > effective_limit {
            warn!(
                original = requested_max,
                clamped = effective_limit,
                "max_tokens clamped to model/platform limit"
            );
            req.max_tokens = Some(effective_limit);
        }
    }

    // Step 3: Check for payment
    let payment_header = headers
        .get("payment-signature")
        .and_then(|v| v.to_str().ok());

    if payment_header.is_none() && state.dev_bypass_payment {
        // Dev-mode payment bypass — skip payment verification entirely
        warn!(
            model = %req.model,
            "DEV MODE: payment bypassed for request to {}",
            req.model
        );
        counter!("rcr_payments_total", "status" => "dev_bypass").increment(1);

        let ctx = ProviderCallContext {
            state: &state,
            req: &req,
            model_info,
            headers: &headers,
            debug_enabled,
            request_start,
            routing_tier: &routing_tier,
            routing_score,
            routing_profile: &routing_profile,
            session_id: &session_id,
            payment_status: PaymentStatus::DevBypass,
        };

        return provider::execute_provider_call(&ctx)
            .await
            .map(|r| r.response)
            .map_err(|e| match e {
                ProviderCallError::AllProvidersFailed { model, error, .. } => {
                    GatewayError::Internal(format!(
                        "all providers failed for model '{}' (dev bypass): {}",
                        model, error
                    ))
                }
                ProviderCallError::Internal(msg) => GatewayError::Internal(msg),
            });
    }

    if payment_header.is_none() {
        // Return 402 with pricing info
        counter!("rcr_payments_total", "status" => "none").increment(1);
        info!(model = %req.model, "no payment signature, returning 402");

        let cost = state
            .model_registry
            .estimate_cost(
                &req.model,
                estimate_input_tokens(&req),
                req.max_tokens.unwrap_or(1000),
            )
            .map_err(|e| GatewayError::Internal(e.to_string()))?;

        let atomic_amount = usdc_atomic_amount_checked(&cost.total).map_err(|e| {
            GatewayError::Internal(format!(
                "failed to compute USDC atomic amount for model '{}': {}",
                req.model, e
            ))
        })?;

        let mut accepts = vec![x402::types::PaymentAccept {
            scheme: "exact".to_string(),
            network: x402::types::SOLANA_NETWORK.to_string(),
            amount: atomic_amount.clone(),
            asset: x402::types::USDC_MINT.to_string(),
            pay_to: state.config.solana.recipient_wallet.clone(),
            max_timeout_seconds: x402::types::MAX_TIMEOUT_SECONDS,
            escrow_program_id: None,
        }];

        // Offer escrow scheme if configured
        if state.escrow_claimer.is_some() {
            accepts.push(x402::types::PaymentAccept {
                scheme: "escrow".to_string(),
                network: x402::types::SOLANA_NETWORK.to_string(),
                amount: atomic_amount,
                asset: x402::types::USDC_MINT.to_string(),
                pay_to: state.config.solana.recipient_wallet.clone(),
                max_timeout_seconds: x402::types::MAX_TIMEOUT_SECONDS,
                escrow_program_id: state.config.solana.escrow_program_id.clone(),
            });
        }

        let payment_required = x402::types::PaymentRequired {
            x402_version: x402::types::X402_VERSION,
            resource: x402::types::Resource {
                url: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
            },
            accepts,
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

    // Track escrow-specific info for post-response claim
    let payment_scheme: String;
    let mut escrow_service_id: Option<String> = None;
    let mut escrow_agent_pubkey: Option<String> = None;
    // FIX 3: Track the verified deposit amount to cap claim amounts
    let escrow_deposited_amount: Option<u64>;

    match payment_payload {
        Some(payload) => {
            // --- H2: Validate all `accepted` fields ---

            // Verify the resource URL matches this endpoint
            if payload.resource.url != "/v1/chat/completions" {
                return Err(GatewayError::InvalidPayment(format!(
                    "payment resource '{}' does not match this endpoint",
                    payload.resource.url
                )));
            }

            // Verify the resource method is POST
            if !payload.resource.method.eq_ignore_ascii_case("POST") {
                warn!(
                    method = %payload.resource.method,
                    "payment resource method mismatch"
                );
                return Err(GatewayError::BadRequest(format!(
                    "payment resource method must be POST, got '{}'",
                    payload.resource.method
                )));
            }

            // Verify network is Solana
            if !payload
                .accepted
                .network
                .eq_ignore_ascii_case(x402::types::SOLANA_NETWORK)
            {
                warn!(
                    network = %payload.accepted.network,
                    expected = %x402::types::SOLANA_NETWORK,
                    "payment network mismatch"
                );
                return Err(GatewayError::BadRequest(format!(
                    "payment network must be '{}', got '{}'",
                    x402::types::SOLANA_NETWORK,
                    payload.accepted.network
                )));
            }

            // Verify asset is USDC-SPL mint
            if payload.accepted.asset != x402::types::USDC_MINT {
                warn!(
                    asset = %payload.accepted.asset,
                    expected = %x402::types::USDC_MINT,
                    "payment asset mismatch"
                );
                return Err(GatewayError::BadRequest(format!(
                    "payment asset must be USDC mint '{}', got '{}'",
                    x402::types::USDC_MINT,
                    payload.accepted.asset
                )));
            }

            // Verify pay_to matches the gateway's recipient wallet
            if payload.accepted.pay_to != state.config.solana.recipient_wallet {
                warn!(
                    pay_to = %payload.accepted.pay_to,
                    expected = %state.config.solana.recipient_wallet,
                    "payment pay_to mismatch"
                );
                return Err(GatewayError::BadRequest(format!(
                    "payment pay_to must be '{}', got '{}'",
                    state.config.solana.recipient_wallet, payload.accepted.pay_to
                )));
            }

            // --- C1: Recompute expected cost and validate client amount ---
            let expected_cost = state
                .model_registry
                .estimate_cost(
                    &req.model,
                    estimate_input_tokens(&req),
                    req.max_tokens.unwrap_or(1000),
                )
                .map_err(|e| GatewayError::Internal(e.to_string()))?;
            let expected_amount: u64 = usdc_atomic_amount_checked(&expected_cost.total)
                .map_err(|e| {
                    GatewayError::Internal(format!(
                        "failed to compute expected payment amount: {e}"
                    ))
                })?
                .parse()
                .map_err(|_| {
                    GatewayError::Internal(
                        "failed to parse expected payment amount as u64".to_string(),
                    )
                })?;
            let client_amount: u64 = payload.accepted.amount.parse().map_err(|_| {
                GatewayError::BadRequest(format!(
                    "invalid payment amount '{}': must be a valid integer",
                    payload.accepted.amount
                ))
            })?;

            if client_amount < expected_amount {
                warn!(
                    client_amount,
                    expected_amount,
                    model = %req.model,
                    "payment amount insufficient"
                );
                return Err(GatewayError::BadRequest(format!(
                    "payment amount insufficient: paid {client_amount} but cost is {expected_amount} atomic USDC"
                )));
            }

            // --- M6: Validate scheme matches PayloadData variant ---
            match (payload.accepted.scheme.as_str(), &payload.payload) {
                ("exact", x402::types::PayloadData::Escrow(_)) => {
                    return Err(GatewayError::BadRequest(
                        "scheme is 'exact' but payload contains escrow data".to_string(),
                    ));
                }
                ("escrow", x402::types::PayloadData::Direct(_)) => {
                    return Err(GatewayError::BadRequest(
                        "scheme is 'escrow' but payload contains direct transfer data".to_string(),
                    ));
                }
                _ => {}
            }

            // Track scheme and escrow info
            payment_scheme = payload.accepted.scheme.clone();
            if let x402::types::PayloadData::Escrow(ref ep) = payload.payload {
                escrow_service_id = Some(ep.service_id.clone());
                escrow_agent_pubkey = Some(ep.agent_pubkey.clone());
            }

            // --- C2: Mandatory replay attack prevention ---
            let tx_raw = match &payload.payload {
                x402::types::PayloadData::Direct(p) => &p.transaction,
                x402::types::PayloadData::Escrow(p) => &p.deposit_tx,
            };

            // Detect durable nonce to set appropriate replay TTL.
            let is_durable_nonce = payment::uses_durable_nonce(tx_raw);

            // S2 FIX: In-memory replay set uses Instant-based TTL
            let replay_detected = if let Some(cache) = &state.cache {
                cache
                    .check_and_record_tx(tx_raw, is_durable_nonce)
                    .await
                    .is_err()
            } else {
                // No Redis — fall back to in-memory LRU replay set with TTL
                let mut replay_set = state.replay_set.lock().expect("replay_set mutex poisoned");
                let now = Instant::now();
                let found = match replay_set.get(tx_raw) {
                    Some(&inserted_at)
                        if now.duration_since(inserted_at) < crate::AppState::REPLAY_TTL =>
                    {
                        true
                    }
                    Some(_) => {
                        // Entry expired — remove and treat as not found
                        replay_set.pop(tx_raw);
                        false
                    }
                    None => false,
                };
                if found {
                    true
                } else {
                    replay_set.put(tx_raw.to_string(), now);
                    warn!(
                        tx = %tx_raw,
                        "payment accepted under degraded in-memory replay protection (no Redis)"
                    );
                    false
                }
            };

            if replay_detected {
                counter!("rcr_replay_rejections_total").increment(1);
                counter!("rcr_payments_total", "status" => "failed").increment(1);
                warn!(tx = %tx_raw, "replay attack detected — transaction already used");
                return Err(GatewayError::InvalidPayment(
                    "transaction has already been used; each payment signature may only be submitted once".to_string()
                ));
            }

            // Verify and settle via Facilitator — hard enforcement
            // R1 FIX: Check settlement.success flag
            match state.facilitator.verify_and_settle(&payload).await {
                Ok(settlement) if !settlement.success => {
                    // Settlement returned Ok but the transaction was not confirmed
                    counter!("rcr_payments_total", "status" => "failed").increment(1);
                    tracing::warn!(
                        tx_signature = %settlement.tx_signature.as_deref().unwrap_or("unknown"),
                        error = ?settlement.error,
                        "payment settlement failed: transaction not confirmed"
                    );
                    return Err(GatewayError::InvalidPayment(
                        "Payment transaction could not be confirmed. Please retry.".to_string(),
                    ));
                }
                Ok(settlement) => {
                    escrow_deposited_amount = settlement.verified_amount;
                    counter!("rcr_payments_total", "status" => "verified").increment(1);
                    histogram!("rcr_payment_amount_usdc")
                        .record(client_amount as f64 / 1_000_000.0);
                    info!(
                        tx_signature = ?settlement.tx_signature,
                        network = %settlement.network,
                        verified_amount = ?settlement.verified_amount,
                        "payment verified and settled"
                    );
                }
                Err(e) => {
                    counter!("rcr_payments_total", "status" => "failed").increment(1);
                    warn!(error = %e, "payment verification failed");
                    return Err(GatewayError::InvalidPayment(format!(
                        "payment verification failed: {e}"
                    )));
                }
            }
        }
        None => {
            counter!("rcr_payments_total", "status" => "failed").increment(1);
            return Err(GatewayError::InvalidPayment(
                "PAYMENT-SIGNATURE header is present but could not be decoded. \
                 Encode a valid PaymentPayload as standard base64 JSON."
                    .to_string(),
            ));
        }
    }

    // Extract tx_signature for usage tracking.
    let (wallet_address, tx_signature) = extract_payment_info(payment_header.unwrap());

    // Check budget before proxying to provider.
    let estimated_cost = state
        .model_registry
        .estimate_cost(
            &req.model,
            estimate_input_tokens(&req),
            req.max_tokens.unwrap_or(1000),
        )
        .map_err(|e| GatewayError::Internal(format!("failed to estimate cost: {e}")))?
        .total
        .parse::<f64>()
        .map_err(|_| GatewayError::Internal("failed to parse estimated cost as f64".to_string()))?;

    if let Err(e) = state
        .usage
        .check_budget(&wallet_address, estimated_cost)
        .await
    {
        return Err(GatewayError::BadRequest(e.to_string()));
    }

    // Step 5: Proxy to provider (with cache and fallback)
    let provider_name = &model_info.provider;

    let ctx = ProviderCallContext {
        state: &state,
        req: &req,
        model_info,
        headers: &headers,
        debug_enabled,
        request_start,
        routing_tier: &routing_tier,
        routing_score,
        routing_profile: &routing_profile,
        session_id: &session_id,
        payment_status: PaymentStatus::Verified,
    };

    match provider::execute_provider_call(&ctx).await {
        Ok(ProviderCallResult {
            mut response,
            usage,
            actual_provider,
        }) => {
            // Post-response: usage logging, session token, and escrow claims (paid path only)

            // Attach session token for paid non-streaming requests
            if !req.stream {
                if let Some(token) = build_session_token(&wallet_address, &state.session_secret) {
                    if let Ok(hv) = HeaderValue::from_str(&token) {
                        response
                            .headers_mut()
                            .insert(HeaderName::from_static("x-rcr-session"), hv);
                    }
                }
            }

            // Compute escrow claim amount: prefer actual usage, fall back to estimate
            // E2 FIX: Use minimum 1 atomic unit for streaming when estimation fails
            let claim_atomic = if let Some(ref u) = usage {
                Some(compute_actual_atomic_cost(
                    u.prompt_tokens,
                    u.completion_tokens,
                    model_info,
                ))
            } else {
                Some(
                    estimated_atomic_cost(&state.model_registry, &req.model, &req)
                        .unwrap_or_else(|| {
                            warn!(
                                model = %req.model,
                                "cost estimation failed for streaming request — using minimum claim amount (1 atomic unit)"
                            );
                            1
                        }),
                )
            };
            if let Some(amount) = claim_atomic {
                fire_escrow_claim(
                    &state,
                    &payment_scheme,
                    &escrow_service_id,
                    &escrow_agent_pubkey,
                    escrow_deposited_amount,
                    amount,
                );
            } else {
                warn!(
                    model = %req.model,
                    "skipping escrow claim — cost estimation failed"
                );
            }

            // Log spend with actual usage (non-streaming) or estimated (streaming)
            if let Some(ref u) = usage {
                match state
                    .model_registry
                    .estimate_cost(&req.model, u.prompt_tokens, u.completion_tokens)
                    .and_then(|c| {
                        c.total.parse::<f64>().map_err(|e| {
                            router::models::ModelRegistryError::ParseError(e.to_string())
                        })
                    }) {
                    Ok(cost) => {
                        state.usage.log_spend(SpendLogEntry {
                            wallet_address: wallet_address.clone(),
                            model: req.model.clone(),
                            provider: actual_provider.unwrap_or_else(|| provider_name.to_string()),
                            input_tokens: u.prompt_tokens,
                            output_tokens: u.completion_tokens,
                            cost_usdc: cost,
                            tx_signature: tx_signature.clone(),
                            request_id: request_id.clone(),
                            session_id: session_id.clone(),
                        });
                    }
                    Err(e) => {
                        warn!(
                            error = %e,
                            model = %req.model,
                            wallet = %wallet_address,
                            "failed to compute actual cost — skipping spend log to avoid $0 entry"
                        );
                    }
                }
            }

            Ok(response)
        }
        Err(ProviderCallError::AllProvidersFailed {
            model, provider, ..
        }) => {
            // SECURITY: A paid request reached the stub path — all providers failed.
            warn!(
                provider = %provider,
                model = %model,
                wallet = %wallet_address,
                "paid request failed: no provider available — returning error instead of stub"
            );
            counter!("rcr_paid_stub_rejections_total").increment(1);

            Err(GatewayError::Internal(format!(
                "all providers failed for model '{}'. Your payment was submitted but no response \
                 could be generated. Contact the gateway operator or retry. \
                 Provider: {}, tx: {}",
                model,
                provider,
                tx_signature.as_deref().unwrap_or("unknown")
            )))
        }
        Err(ProviderCallError::Internal(msg)) => Err(GatewayError::Internal(msg)),
    }
}

/// Resolve model ID from aliases, smart routing profiles, or direct model IDs.
///
/// Returns (resolved_model, profile_name, tier_name, score) for debug headers.
fn resolve_model_with_debug(
    req: &ChatRequest,
    state: &AppState,
) -> Result<(String, String, String, f64), GatewayError> {
    // Check for profile-based routing (e.g., "auto", "eco", "premium")
    if let Some(profile) = Profile::from_alias(&req.model) {
        let result = scorer::classify(&req.messages, false);
        let model_id = profiles::resolve_model(profile, result.tier);
        return Ok((
            model_id.to_string(),
            req.model.clone(),
            format!("{:?}", result.tier),
            result.score,
        ));
    }

    // Check for model aliases (e.g., "gpt5", "sonnet")
    if let Some(canonical) = profiles::resolve_alias(&req.model) {
        return Ok((
            canonical.to_string(),
            "direct".to_string(),
            "N/A".to_string(),
            0.0,
        ));
    }

    // Check if it's a direct model ID
    if state.model_registry.get(&req.model).is_some() {
        return Ok((
            req.model.clone(),
            "direct".to_string(),
            "N/A".to_string(),
            0.0,
        ));
    }

    Err(GatewayError::ModelNotFound(req.model.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // MAX_MESSAGES validation
    // =========================================================================

    #[test]
    fn test_max_messages_constant() {
        assert_eq!(MAX_MESSAGES, 256);
    }

    #[test]
    fn test_heartbeat_sentinel_is_defined() {
        assert_eq!(
            crate::providers::heartbeat::HEARTBEAT_SENTINEL,
            "__heartbeat__"
        );
    }

    #[test]
    fn test_fallback_header_name_is_valid() {
        use axum::http::HeaderName;
        let name = HeaderName::from_static("x-rcr-fallback");
        assert_eq!(name.as_str(), "x-rcr-fallback");
    }
}
