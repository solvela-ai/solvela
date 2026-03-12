use std::convert::Infallible;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use axum::response::{sse, IntoResponse, Response};
use axum::Json;
use base64::Engine;
use futures::StreamExt;
use metrics::{counter, histogram};
use tracing::{info, warn};

use router::profiles::{self, Profile};
use router::scorer;
use rustyclaw_protocol::ChatRequest;
use x402::solana_types::VersionedTransaction;

use crate::error::GatewayError;
use crate::middleware::prompt_guard::{self, GuardResult, PromptGuardConfig};
use crate::middleware::x402::decode_payment_header;
use crate::providers::fallback;
use crate::providers::heartbeat::{HeartbeatConfig, HeartbeatItem, HeartbeatStream};
use crate::routes::debug_headers::{
    attach_debug_headers, is_debug_enabled, CacheStatus, DebugInfo, PaymentStatus,
};
use crate::usage::SpendLogEntry;
use crate::AppState;

/// Maximum length for a client-provided session ID.
const MAX_SESSION_ID_LEN: usize = 128;

/// Maximum number of messages allowed in a single chat request.
/// Prevents excessive memory usage and cost from very long conversations.
const MAX_MESSAGES: usize = 256;

/// Classify a provider error into a bounded set of label values for metrics.
///
/// Returns one of: `"timeout"`, `"auth"`, `"rate_limit"`, `"server_error"`, `"unknown"`.
/// Cardinality is fixed — never use the raw error message as a label.
fn classify_provider_error(err: &impl std::fmt::Display) -> &'static str {
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

/// Platform-wide upper bound for `max_tokens` to prevent unbounded cost exposure.
const MAX_TOKENS_LIMIT: u32 = 128_000;

/// Validate a session ID: max 128 chars, `[a-zA-Z0-9\-_]` only.
fn validate_session_id(value: &str) -> Option<String> {
    if value.is_empty() || value.len() > MAX_SESSION_ID_LEN {
        return None;
    }
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Some(value.to_string())
    } else {
        None
    }
}

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

    // Extract request ID from the incoming header (if valid, it will be echoed
    // by the RequestIdLayer middleware; if invalid, the middleware generates a UUID,
    // but we can only capture the client-provided one here).
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

        let mut accepts = vec![x402::types::PaymentAccept {
            scheme: "exact".to_string(),
            network: x402::types::SOLANA_NETWORK.to_string(),
            amount: usdc_atomic_amount(&cost.total),
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
                amount: usdc_atomic_amount(&cost.total),
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
            let expected_amount: u64 = usdc_atomic_amount(&expected_cost.total)
                .parse()
                .unwrap_or(0);
            let client_amount: u64 = payload.accepted.amount.parse().unwrap_or(0);

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
            // Untagged serde deserialization cannot enforce this; check explicitly.
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
            // Atomically record this transaction signature before verifying.
            // If it was already seen, reject immediately — same signed tx
            // cannot be replayed.
            //
            // H1 TOCTOU mitigation: Replay check must happen before
            // verify_and_settle to prevent TOCTOU double-spend. The tx
            // signature is recorded atomically here, so concurrent requests
            // with the same tx will be rejected before verification begins.
            // Without this ordering, two concurrent requests could both pass
            // verification but only one settlement would succeed on-chain,
            // leaving the other request having consumed LLM resources for free.
            //
            // NOTE: The 120s TTL (in Redis / LRU eviction in-memory) is
            // insufficient for durable nonce transactions, which have no
            // blockhash expiry. A future PR should extend replay persistence
            // for durable nonce payloads (e.g., database-backed dedup).
            let tx_raw = match &payload.payload {
                x402::types::PayloadData::Direct(p) => &p.transaction,
                x402::types::PayloadData::Escrow(p) => &p.deposit_tx,
            };

            let replay_detected = if let Some(cache) = &state.cache {
                cache.check_and_record_tx(tx_raw).await.is_err()
            } else {
                // No Redis — fall back to in-memory LRU replay set
                let mut replay_set = state.replay_set.lock().expect("replay_set mutex poisoned");
                if replay_set.get(tx_raw).is_some() {
                    true
                } else {
                    replay_set.put(tx_raw.to_string(), ());
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
            match state.facilitator.verify_and_settle(&payload).await {
                Ok(settlement) => {
                    // Capture verified deposit amount for escrow claim capping
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
                    // Payment verification failed — reject the request
                    counter!("rcr_payments_total", "status" => "failed").increment(1);
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
            counter!("rcr_payments_total", "status" => "failed").increment(1);
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

    // Track cache status for debug headers
    let mut cache_status = if req.stream {
        counter!("rcr_cache_total", "result" => "skip").increment(1);
        CacheStatus::Skip
    } else {
        CacheStatus::Miss
    };

    // Check cache first (only for non-streaming requests)
    if !req.stream {
        if let Some(cache) = &state.cache {
            if let Some(cached) = cache.get(&req).await {
                counter!("rcr_cache_total", "result" => "hit").increment(1);
                counter!("rcr_payments_total", "status" => "cached").increment(1);
                info!(model = %req.model, "serving from cache");
                cache_status = CacheStatus::Hit;
                let mut resp = Json(
                    serde_json::to_value(&cached)
                        .map_err(|e| GatewayError::Internal(e.to_string()))?,
                )
                .into_response();

                attach_session_id(&mut resp, &session_id);

                if debug_enabled {
                    attach_debug_headers(
                        &mut resp,
                        &build_debug_info(
                            &req.model,
                            &routing_tier,
                            routing_score,
                            &routing_profile,
                            provider_name,
                            cache_status,
                            request_start.elapsed().as_millis() as u64,
                            PaymentStatus::Verified,
                            estimate_input_tokens(&req),
                            req.max_tokens.unwrap_or(1000),
                        ),
                    );
                }

                return Ok(resp);
            } else {
                counter!("rcr_cache_total", "result" => "miss").increment(1);
            }
        } else {
            counter!("rcr_cache_total", "result" => "miss").increment(1);
        }
    }

    // Check for agent-specified fallback preferences
    let fallback_pref = headers
        .get("x-rcr-fallback-preference")
        .and_then(|v| v.to_str().ok());

    if req.stream {
        info!(provider = provider_name, model = %req.model, "streaming to provider (with model fallback)");

        let provider_start = std::time::Instant::now();
        let result = if let Some(pref) = fallback_pref {
            let mut chain: Vec<(String, String)> =
                vec![(provider_name.to_string(), req.model.clone())];
            for (p, m) in parse_fallback_preference(pref) {
                let entry = (p.to_string(), m.to_string());
                if !chain.contains(&entry) {
                    chain.push(entry);
                }
            }
            fallback::stream_with_chain(
                &state.providers,
                &state.provider_health,
                &chain,
                &req.model,
                req.clone(),
            )
            .await
        } else {
            fallback::stream_with_model_fallback(
                &state.providers,
                &state.provider_health,
                provider_name,
                &req.model,
                req.clone(),
            )
            .await
        };

        let provider_duration = provider_start.elapsed();
        histogram!("rcr_provider_request_duration_seconds", "provider" => provider_name.to_string())
            .record(provider_duration.as_secs_f64());

        match result {
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
                let heartbeat_stream =
                    HeartbeatStream::new(result.data, HeartbeatConfig::default());

                let sse_stream = heartbeat_stream.map(|item| match item {
                    HeartbeatItem::Chunk(Ok(chunk)) => {
                        let json = serde_json::to_string(&chunk).unwrap_or_default();
                        Ok::<_, Infallible>(sse::Event::default().data(json))
                    }
                    HeartbeatItem::Chunk(Err(e)) => {
                        warn!(error = %e, "stream chunk error");
                        Ok(sse::Event::default().data(format!("{{\"error\": \"{e}\"}}")))
                    }
                    HeartbeatItem::KeepAlive => Ok(sse::Event::default().comment("keep-alive")),
                });

                let mut resp = sse::Sse::new(sse_stream).into_response();

                // Add fallback header if served by a different model
                if result.was_fallback {
                    let fallback_value =
                        format!("{} -> {}", result.original_model, result.actual_model);
                    if let Ok(hv) = HeaderValue::from_str(&fallback_value) {
                        resp.headers_mut()
                            .insert(HeaderName::from_static("x-rcr-fallback"), hv);
                    }
                }

                attach_session_id(&mut resp, &session_id);

                if debug_enabled {
                    attach_debug_headers(
                        &mut resp,
                        &build_debug_info(
                            &req.model,
                            &routing_tier,
                            routing_score,
                            &routing_profile,
                            provider_name,
                            cache_status,
                            request_start.elapsed().as_millis() as u64,
                            PaymentStatus::Verified,
                            estimate_input_tokens(&req),
                            req.max_tokens.unwrap_or(1000),
                        ),
                    );
                }

                return Ok(resp);
            }
            Err(e) => {
                // All models failed — fall through to stub
                let error_type = classify_provider_error(&e);
                counter!("rcr_provider_errors_total", "provider" => provider_name.to_string(), "error_type" => error_type).increment(1);
            }
        }
    } else {
        info!(provider = provider_name, model = %req.model, "proxying to provider (with model fallback)");

        let provider_start = std::time::Instant::now();
        let result = if let Some(pref) = fallback_pref {
            let mut chain: Vec<(String, String)> =
                vec![(provider_name.to_string(), req.model.clone())];
            for (p, m) in parse_fallback_preference(pref) {
                let entry = (p.to_string(), m.to_string());
                if !chain.contains(&entry) {
                    chain.push(entry);
                }
            }
            fallback::chat_with_chain(
                &state.providers,
                &state.provider_health,
                &chain,
                &req.model,
                req.clone(),
            )
            .await
        } else {
            fallback::chat_with_model_fallback(
                &state.providers,
                &state.provider_health,
                provider_name,
                &req.model,
                req.clone(),
            )
            .await
        };

        let provider_duration = provider_start.elapsed();
        histogram!("rcr_provider_request_duration_seconds", "provider" => provider_name.to_string())
            .record(provider_duration.as_secs_f64());

        match result {
            Ok(result) => {
                if let Some(cache) = &state.cache {
                    cache.set(&req, &result.data).await;
                }

                if let Some(usage) = &result.data.usage {
                    let cost = state
                        .model_registry
                        .estimate_cost(&req.model, usage.prompt_tokens, usage.completion_tokens)
                        .map(|c| c.total.parse::<f64>().unwrap_or(0.0))
                        .unwrap_or(0.0);

                    state.usage.log_spend(SpendLogEntry {
                        wallet_address: wallet_address.clone(),
                        model: req.model.clone(),
                        provider: result.actual_provider.clone(),
                        input_tokens: usage.prompt_tokens,
                        output_tokens: usage.completion_tokens,
                        cost_usdc: cost,
                        tx_signature: tx_signature.clone(),
                        request_id: request_id.clone(),
                        session_id: session_id.clone(),
                    });
                }

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
                    let fallback_value =
                        format!("{} -> {}", result.original_model, result.actual_model);
                    if let Ok(hv) = HeaderValue::from_str(&fallback_value) {
                        resp.headers_mut()
                            .insert(HeaderName::from_static("x-rcr-fallback"), hv);
                    }
                }

                if let Some(token) = build_session_token(&wallet_address, &state.session_secret) {
                    if let Ok(hv) = HeaderValue::from_str(&token) {
                        resp.headers_mut()
                            .insert(HeaderName::from_static("x-rcr-session"), hv);
                    }
                }

                attach_session_id(&mut resp, &session_id);

                if debug_enabled {
                    let actual_tokens_out = result
                        .data
                        .usage
                        .as_ref()
                        .map(|u| u.completion_tokens)
                        .unwrap_or(req.max_tokens.unwrap_or(1000));
                    let actual_tokens_in = result
                        .data
                        .usage
                        .as_ref()
                        .map(|u| u.prompt_tokens)
                        .unwrap_or(estimate_input_tokens(&req));
                    attach_debug_headers(
                        &mut resp,
                        &build_debug_info(
                            &req.model,
                            &routing_tier,
                            routing_score,
                            &routing_profile,
                            provider_name,
                            cache_status,
                            request_start.elapsed().as_millis() as u64,
                            PaymentStatus::Verified,
                            actual_tokens_in,
                            actual_tokens_out,
                        ),
                    );
                }

                return Ok(resp);
            }
            Err(e) => {
                // All models failed — fall through to stub
                let error_type = classify_provider_error(&e);
                counter!("rcr_provider_errors_total", "provider" => provider_name.to_string(), "error_type" => error_type).increment(1);
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

    let mut resp = Json(response).into_response();

    attach_session_id(&mut resp, &session_id);

    if debug_enabled {
        attach_debug_headers(
            &mut resp,
            &build_debug_info(
                &req.model,
                &routing_tier,
                routing_score,
                &routing_profile,
                provider_name,
                cache_status,
                request_start.elapsed().as_millis() as u64,
                PaymentStatus::Verified,
                estimate_input_tokens(&req),
                req.max_tokens.unwrap_or(1000),
            ),
        );
    }

    Ok(resp)
}

/// Try to decode a PaymentPayload from the PAYMENT-SIGNATURE header.
///
/// Returns `None` if decoding fails — this is intentional for backwards
/// compatibility with raw string headers used in tests (e.g., "fake-payment-for-testing").
///
/// Delegates to the shared `decode_payment_header` in the x402 middleware.
fn decode_payment_from_header(header: &str) -> Option<x402::types::PaymentPayload> {
    decode_payment_header(header).ok()
}

/// Extract wallet address and transaction signature from the payment header.
///
/// If the header is a valid PaymentPayload, extracts the actual payer wallet
/// and transaction signature. For escrow payments, uses `agent_pubkey`. For
/// direct payments, decodes the Solana transaction to get the first signer
/// (fee payer). Falls back to "unknown" if extraction fails.
fn extract_payment_info(header: &str) -> (String, Option<String>) {
    match decode_payment_from_header(header) {
        Some(payload) => {
            let wallet = extract_payer_wallet_from_payload(&payload);
            let tx_sig = match &payload.payload {
                x402::types::PayloadData::Direct(p) => Some(p.transaction.clone()),
                x402::types::PayloadData::Escrow(p) => Some(p.deposit_tx.clone()),
            };
            (wallet, tx_sig)
        }
        None => ("unknown".to_string(), None),
    }
}

/// Extract the payer wallet address from a payment payload.
///
/// For escrow payments, uses the `agent_pubkey` field (the depositor).
/// For direct payments, decodes the base64 transaction and extracts the
/// first account key (the fee payer / signer in Solana transactions).
/// Returns "unknown" if extraction fails.
fn extract_payer_wallet_from_payload(payload: &x402::types::PaymentPayload) -> String {
    match &payload.payload {
        x402::types::PayloadData::Escrow(p) => p.agent_pubkey.clone(),
        x402::types::PayloadData::Direct(p) => {
            extract_signer_from_base64_tx(&p.transaction).unwrap_or_else(|| "unknown".to_string())
        }
    }
}

/// Attempt to extract the first signer (fee payer) public key from a
/// base64-encoded Solana versioned transaction.
fn extract_signer_from_base64_tx(b64_tx: &str) -> Option<String> {
    let tx_bytes = base64::engine::general_purpose::STANDARD
        .decode(b64_tx)
        .ok()?;
    let tx = VersionedTransaction::from_bytes(&tx_bytes).ok()?;
    let msg = tx.parse_message().ok()?;
    msg.account_keys.first().map(|pk| pk.to_string())
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
            req.model.clone(), // profile name (e.g., "auto")
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

/// Build a [`DebugInfo`] from the routing data collected during request processing.
#[allow(clippy::too_many_arguments)]
fn build_debug_info(
    model: &str,
    tier: &str,
    score: f64,
    profile: &str,
    provider: &str,
    cache_status: CacheStatus,
    latency_ms: u64,
    payment_status: PaymentStatus,
    token_estimate_in: u32,
    token_estimate_out: u32,
) -> DebugInfo {
    DebugInfo {
        model: model.to_string(),
        tier: tier.to_string(),
        score,
        profile: profile.to_string(),
        provider: provider.to_string(),
        cache_status,
        latency_ms,
        payment_status,
        token_estimate_in,
        token_estimate_out,
    }
}

/// Estimate cost in atomic USDC units using the model registry's cost breakdown.
///
/// Used as a fallback when actual token usage is unavailable (e.g., streaming).
fn estimated_atomic_cost(
    registry: &router::models::ModelRegistry,
    model: &str,
    req: &ChatRequest,
) -> u64 {
    registry
        .estimate_cost(
            model,
            estimate_input_tokens(req),
            req.max_tokens.unwrap_or(1000),
        )
        .ok()
        .and_then(|c| c.total.parse::<f64>().ok())
        .map(|f| (f * 1_000_000.0) as u64)
        .unwrap_or(0)
}

/// Fire an escrow claim transaction if the payment scheme is escrow.
///
/// Prefers the durable claim queue (PostgreSQL) when a DB pool is available,
/// falling back to fire-and-forget via `claim_async` when it is not.
/// Caps the claim amount to the verified deposit to prevent over-claiming.
fn fire_escrow_claim(
    state: &Arc<AppState>,
    payment_scheme: &str,
    escrow_service_id: &Option<String>,
    escrow_agent_pubkey: &Option<String>,
    escrow_deposited_amount: Option<u64>,
    claim_atomic: u64,
) {
    if payment_scheme != "escrow" {
        return;
    }
    if let (Some(ref sid_b64), Some(ref agent_b58)) = (escrow_service_id, escrow_agent_pubkey) {
        // Cap claim amount to the verified deposit amount
        let claim_amount = match escrow_deposited_amount {
            Some(deposited) => claim_atomic.min(deposited),
            None => claim_atomic,
        };

        if let Ok(sid) = decode_service_id(sid_b64) {
            // Prefer durable queue if DB is available
            if let Some(ref pool) = state.db_pool {
                let pool = pool.clone();
                let agent = agent_b58.clone();
                tokio::spawn(async move {
                    if let Err(e) = x402::escrow::claim_queue::enqueue_claim(
                        &pool,
                        &sid,
                        &agent,
                        claim_amount,
                        escrow_deposited_amount,
                    )
                    .await
                    {
                        tracing::warn!(error = %e, "failed to enqueue escrow claim");
                    }
                });
            } else if let Some(claimer) = &state.escrow_claimer {
                // Fallback: fire-and-forget (no DB)
                if let Ok(agent_bytes) = decode_agent_pubkey(agent_b58) {
                    claimer.claim_async(sid, agent_bytes, claim_amount);
                }
            }
        }
    }
}

/// Build a session token for the given wallet, valid for 1 hour.
///
/// Returns `None` if token creation fails — callers should silently skip the
/// header rather than failing the request.
fn build_session_token(wallet: &str, secret: &[u8]) -> Option<String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();

    let claims = crate::session::SessionClaims {
        wallet: wallet.to_string(),
        budget_remaining: 0, // exact-scheme payment — no remaining budget
        issued_at: now,
        expires_at: now + 3600, // 1 hour
        allowed_models: vec![], // all models allowed
    };

    crate::session::create_session_token(&claims, secret).ok()
}

/// Attach the `X-Session-Id` response header if a valid session ID was provided.
fn attach_session_id(resp: &mut Response, session_id: &Option<String>) {
    if let Some(sid) = session_id {
        if let Ok(hv) = HeaderValue::from_str(sid) {
            resp.headers_mut()
                .insert(HeaderName::from_static("x-session-id"), hv);
        }
    }
}

/// Rough token estimate: ~4 chars per token.
fn estimate_input_tokens(req: &ChatRequest) -> u32 {
    let chars: usize = req.messages.iter().map(|m| m.content.len()).sum();
    (chars / 4).max(1) as u32
}

/// Compute the actual cost in atomic USDC units from token usage.
///
/// Uses integer arithmetic to avoid f64 precision loss on financial amounts.
/// Cost per million tokens is converted to micro-USDC (atomic units) early,
/// then all math stays in u128 to prevent overflow on large token counts.
fn compute_actual_atomic_cost(
    prompt_tokens: u32,
    completion_tokens: u32,
    model_info: &rustyclaw_protocol::ModelInfo,
) -> u64 {
    // Convert cost-per-million-tokens from USDC (f64) to atomic micro-USDC (u64)
    // by multiplying by 1_000_000. This is the only f64→int conversion.
    let input_cost_atomic_per_million = (model_info.input_cost_per_million * 1_000_000.0) as u128;
    let output_cost_atomic_per_million = (model_info.output_cost_per_million * 1_000_000.0) as u128;

    // tokens * atomic_cost_per_million / 1_000_000 = atomic cost
    let input_atomic = (prompt_tokens as u128) * input_cost_atomic_per_million / 1_000_000;
    let output_atomic = (completion_tokens as u128) * output_cost_atomic_per_million / 1_000_000;
    let provider_atomic = input_atomic + output_atomic;

    // 5% platform fee: total = provider * 105 / 100
    let total_atomic = provider_atomic * 105 / 100;

    total_atomic as u64
}

/// Decode a base64-encoded service_id into a 32-byte array.
fn decode_service_id(b64: &str) -> Result<[u8; 32], String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| format!("invalid service_id base64: {e}"))?;
    if bytes.len() != 32 {
        return Err(format!("service_id must be 32 bytes, got {}", bytes.len()));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(arr)
}

/// Decode a base58-encoded agent pubkey into a 32-byte array.
fn decode_agent_pubkey(b58: &str) -> Result<[u8; 32], String> {
    let bytes = bs58::decode(b58)
        .into_vec()
        .map_err(|e| format!("invalid agent_pubkey base58: {e}"))?;
    if bytes.len() != 32 {
        return Err(format!(
            "agent_pubkey must be 32 bytes, got {}",
            bytes.len()
        ));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(arr)
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use rustyclaw_protocol::{ChatMessage, ModelInfo, Role};

    fn user_msg(content: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: content.to_string(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    fn make_request(model: &str, messages: Vec<ChatMessage>) -> ChatRequest {
        ChatRequest {
            model: model.to_string(),
            messages,
            max_tokens: None,
            temperature: None,
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
        }
    }

    // =========================================================================
    // usdc_atomic_amount — Financial calculation (100% coverage required)
    // =========================================================================

    #[test]
    fn test_usdc_atomic_basic_decimal() {
        // 1.50 USDC = 1,500,000 atomic
        assert_eq!(usdc_atomic_amount("1.50"), "1500000");
    }

    #[test]
    fn test_usdc_atomic_small_amount() {
        // 0.002625 USDC = 2,625 atomic
        assert_eq!(usdc_atomic_amount("0.002625"), "2625");
    }

    #[test]
    fn test_usdc_atomic_whole_number() {
        // 5 USDC = 5,000,000 atomic
        assert_eq!(usdc_atomic_amount("5"), "5000000");
    }

    #[test]
    fn test_usdc_atomic_zero() {
        assert_eq!(usdc_atomic_amount("0"), "0");
        assert_eq!(usdc_atomic_amount("0.0"), "0");
        assert_eq!(usdc_atomic_amount("0.000000"), "0");
    }

    #[test]
    fn test_usdc_atomic_max_precision() {
        // 0.000001 USDC = 1 atomic (smallest unit)
        assert_eq!(usdc_atomic_amount("0.000001"), "1");
    }

    #[test]
    fn test_usdc_atomic_truncates_beyond_6_decimals() {
        // 0.0000019 should truncate to 0.000001 = 1 atomic
        assert_eq!(usdc_atomic_amount("0.0000019"), "1");
    }

    #[test]
    fn test_usdc_atomic_large_amount() {
        // 1000.000000 USDC = 1,000,000,000 atomic
        assert_eq!(usdc_atomic_amount("1000.000000"), "1000000000");
    }

    #[test]
    fn test_usdc_atomic_whitespace_trimmed() {
        assert_eq!(usdc_atomic_amount("  1.50  "), "1500000");
    }

    #[test]
    fn test_usdc_atomic_exact_six_decimals() {
        assert_eq!(usdc_atomic_amount("0.123456"), "123456");
    }

    #[test]
    fn test_usdc_atomic_fewer_decimals_pads() {
        // 0.5 should pad to 0.500000 = 500,000 atomic
        assert_eq!(usdc_atomic_amount("0.5"), "500000");
    }

    #[test]
    fn test_usdc_atomic_empty_string() {
        assert_eq!(usdc_atomic_amount(""), "0");
    }

    #[test]
    fn test_usdc_atomic_typical_llm_costs() {
        // GPT-4o: ~$0.002625 for a typical request
        assert_eq!(usdc_atomic_amount("0.002625"), "2625");
        // DeepSeek: ~$0.000042 for a small request
        assert_eq!(usdc_atomic_amount("0.000042"), "42");
        // Claude Opus: ~$0.015750 for a complex request
        assert_eq!(usdc_atomic_amount("0.015750"), "15750");
    }

    // =========================================================================
    // estimate_input_tokens
    // =========================================================================

    #[test]
    fn test_estimate_input_tokens_simple() {
        // "Hello" = 5 chars → 5/4 = 1 token
        let req = make_request("m", vec![user_msg("Hello")]);
        assert_eq!(estimate_input_tokens(&req), 1);
    }

    #[test]
    fn test_estimate_input_tokens_longer_message() {
        // 100 chars → 25 tokens
        let msg = "a".repeat(100);
        let req = make_request("m", vec![user_msg(&msg)]);
        assert_eq!(estimate_input_tokens(&req), 25);
    }

    #[test]
    fn test_estimate_input_tokens_multiple_messages() {
        // 50 + 50 = 100 chars → 25 tokens
        let req = make_request(
            "m",
            vec![user_msg(&"b".repeat(50)), user_msg(&"c".repeat(50))],
        );
        assert_eq!(estimate_input_tokens(&req), 25);
    }

    #[test]
    fn test_estimate_input_tokens_empty_message_returns_at_least_one() {
        let req = make_request("m", vec![user_msg("")]);
        assert_eq!(estimate_input_tokens(&req), 1, "minimum should be 1 token");
    }

    // =========================================================================
    // compute_actual_atomic_cost — Financial calculation (100% coverage)
    // =========================================================================

    #[test]
    fn test_compute_actual_atomic_cost_basic() {
        let model_info = ModelInfo {
            id: "openai/gpt-4o".to_string(),
            provider: "openai".to_string(),
            model_id: "gpt-4o".to_string(),
            display_name: "GPT-4o".to_string(),
            input_cost_per_million: 2.50,
            output_cost_per_million: 10.00,
            context_window: 128_000,
            supports_streaming: true,
            supports_tools: true,
            supports_vision: true,
            reasoning: false,
            supports_structured_output: false,
            supports_batch: false,
            max_output_tokens: None,
        };

        // 1000 input tokens, 500 output tokens
        // Input: 1000/1M * 2.50 = 0.0025
        // Output: 500/1M * 10.00 = 0.005
        // Provider: 0.0075
        // Total: 0.0075 * 1.05 = 0.007875
        // Atomic: 0.007875 * 1,000,000 = 7875
        let atomic = compute_actual_atomic_cost(1000, 500, &model_info);
        assert_eq!(atomic, 7875);
    }

    #[test]
    fn test_compute_actual_atomic_cost_zero_tokens() {
        let model_info = ModelInfo {
            id: "test/model".to_string(),
            provider: "test".to_string(),
            model_id: "model".to_string(),
            display_name: "Test".to_string(),
            input_cost_per_million: 2.50,
            output_cost_per_million: 10.00,
            context_window: 128_000,
            supports_streaming: true,
            supports_tools: false,
            supports_vision: false,
            reasoning: false,
            supports_structured_output: false,
            supports_batch: false,
            max_output_tokens: None,
        };

        assert_eq!(compute_actual_atomic_cost(0, 0, &model_info), 0);
    }

    #[test]
    fn test_compute_actual_atomic_cost_includes_5_percent_fee() {
        let model_info = ModelInfo {
            id: "test/model".to_string(),
            provider: "test".to_string(),
            model_id: "model".to_string(),
            display_name: "Test".to_string(),
            input_cost_per_million: 1_000_000.0, // $1 per token for easy math
            output_cost_per_million: 0.0,
            context_window: 128_000,
            supports_streaming: true,
            supports_tools: false,
            supports_vision: false,
            reasoning: false,
            supports_structured_output: false,
            supports_batch: false,
            max_output_tokens: None,
        };

        // 1 input token at $1/token = $1.00 provider cost
        // Total: $1.00 * 1.05 = $1.05
        // Atomic: 1.05 * 1,000,000 = 1,050,000
        let atomic = compute_actual_atomic_cost(1, 0, &model_info);
        assert_eq!(atomic, 1_050_000);
    }

    // =========================================================================
    // decode_service_id
    // =========================================================================

    #[test]
    fn test_decode_service_id_valid() {
        use base64::Engine;
        let bytes = [42u8; 32];
        let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        let result = decode_service_id(&b64);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), bytes);
    }

    #[test]
    fn test_decode_service_id_wrong_length() {
        use base64::Engine;
        let bytes = [42u8; 16]; // 16 bytes, not 32
        let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        let result = decode_service_id(&b64);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("32 bytes"));
    }

    #[test]
    fn test_decode_service_id_invalid_base64() {
        let result = decode_service_id("not-valid-base64!!!");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("base64"));
    }

    // =========================================================================
    // decode_agent_pubkey
    // =========================================================================

    #[test]
    fn test_decode_agent_pubkey_valid() {
        // All-ones pubkey = "11111111111111111111111111111111" (base58)
        let result = decode_agent_pubkey("11111111111111111111111111111111");
        assert!(result.is_ok());
        let bytes = result.unwrap();
        assert_eq!(bytes.len(), 32);
    }

    #[test]
    fn test_decode_agent_pubkey_wrong_length() {
        // Encode a 16-byte value as base58
        let short_bytes = [1u8; 16];
        let b58 = bs58::encode(&short_bytes).into_string();
        let result = decode_agent_pubkey(&b58);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("32 bytes"));
    }

    #[test]
    fn test_decode_agent_pubkey_invalid_base58() {
        // '0', 'I', 'O', 'l' are not in base58 alphabet
        let result = decode_agent_pubkey("00000InvalidBase58lII");
        assert!(result.is_err());
    }

    // =========================================================================
    // decode_payment_from_header
    // =========================================================================

    #[test]
    fn test_decode_payment_from_header_valid_base64() {
        use base64::Engine;
        let payload = x402::types::PaymentPayload {
            x402_version: 2,
            resource: x402::types::Resource {
                url: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
            },
            accepted: x402::types::PaymentAccept {
                scheme: "exact".to_string(),
                network: x402::types::SOLANA_NETWORK.to_string(),
                amount: "2625".to_string(),
                asset: x402::types::USDC_MINT.to_string(),
                pay_to: "TestWallet".to_string(),
                max_timeout_seconds: 300,
                escrow_program_id: None,
            },
            payload: x402::types::PayloadData::Direct(x402::types::SolanaPayload {
                transaction: "dGVzdA==".to_string(),
            }),
        };
        let json = serde_json::to_vec(&payload).unwrap();
        let encoded = base64::engine::general_purpose::STANDARD.encode(&json);

        let result = decode_payment_from_header(&encoded);
        assert!(result.is_some());
        let decoded = result.unwrap();
        assert_eq!(decoded.x402_version, 2);
        assert_eq!(decoded.accepted.scheme, "exact");
    }

    #[test]
    fn test_decode_payment_from_header_raw_json() {
        let payload = x402::types::PaymentPayload {
            x402_version: 2,
            resource: x402::types::Resource {
                url: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
            },
            accepted: x402::types::PaymentAccept {
                scheme: "exact".to_string(),
                network: x402::types::SOLANA_NETWORK.to_string(),
                amount: "2625".to_string(),
                asset: x402::types::USDC_MINT.to_string(),
                pay_to: "TestWallet".to_string(),
                max_timeout_seconds: 300,
                escrow_program_id: None,
            },
            payload: x402::types::PayloadData::Direct(x402::types::SolanaPayload {
                transaction: "dGVzdA==".to_string(),
            }),
        };
        let json_str = serde_json::to_string(&payload).unwrap();

        let result = decode_payment_from_header(&json_str);
        assert!(result.is_some());
    }

    #[test]
    fn test_decode_payment_from_header_invalid_returns_none() {
        assert!(decode_payment_from_header("garbage-data").is_none());
        assert!(decode_payment_from_header("").is_none());
        assert!(decode_payment_from_header("fake-payment-for-testing").is_none());
    }

    // =========================================================================
    // extract_payment_info
    // =========================================================================

    #[test]
    fn test_extract_payment_info_invalid_header() {
        let (wallet, tx_sig) = extract_payment_info("not-a-valid-payment");
        assert_eq!(wallet, "unknown");
        assert!(tx_sig.is_none());
    }

    #[test]
    fn test_extract_payment_info_valid_header_direct_undecodable_tx() {
        use base64::Engine;
        let payload = x402::types::PaymentPayload {
            x402_version: 2,
            resource: x402::types::Resource {
                url: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
            },
            accepted: x402::types::PaymentAccept {
                scheme: "exact".to_string(),
                network: x402::types::SOLANA_NETWORK.to_string(),
                amount: "2625".to_string(),
                asset: x402::types::USDC_MINT.to_string(),
                pay_to: "MyWallet123".to_string(),
                max_timeout_seconds: 300,
                escrow_program_id: None,
            },
            payload: x402::types::PayloadData::Direct(x402::types::SolanaPayload {
                // "dGVzdHR4" decodes to "testtx" -- not a valid Solana tx,
                // so payer extraction falls back to "unknown".
                transaction: "dGVzdHR4".to_string(),
            }),
        };
        let json = serde_json::to_vec(&payload).unwrap();
        let encoded = base64::engine::general_purpose::STANDARD.encode(&json);

        let (wallet, tx_sig) = extract_payment_info(&encoded);
        assert_eq!(wallet, "unknown");
        assert_eq!(tx_sig, Some("dGVzdHR4".to_string()));
    }

    #[test]
    fn test_extract_payment_info_escrow_uses_agent_pubkey() {
        use base64::Engine;
        let payload = x402::types::PaymentPayload {
            x402_version: 2,
            resource: x402::types::Resource {
                url: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
            },
            accepted: x402::types::PaymentAccept {
                scheme: "escrow".to_string(),
                network: x402::types::SOLANA_NETWORK.to_string(),
                amount: "2625".to_string(),
                asset: x402::types::USDC_MINT.to_string(),
                pay_to: "RecipientWallet".to_string(),
                max_timeout_seconds: 300,
                escrow_program_id: None,
            },
            payload: x402::types::PayloadData::Escrow(x402::types::EscrowPayload {
                deposit_tx: "dGVzdA==".to_string(),
                service_id: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_string(),
                agent_pubkey: "9noXzpXnkyEcKF3AeXqUHTdR59V5uvrRBUo9bwsHaByz".to_string(),
            }),
        };
        let json = serde_json::to_vec(&payload).unwrap();
        let encoded = base64::engine::general_purpose::STANDARD.encode(&json);

        let (wallet, tx_sig) = extract_payment_info(&encoded);
        // Escrow: wallet comes from agent_pubkey, NOT from pay_to
        assert_eq!(wallet, "9noXzpXnkyEcKF3AeXqUHTdR59V5uvrRBUo9bwsHaByz");
        assert_eq!(tx_sig, Some("dGVzdA==".to_string()));
    }

    // =========================================================================
    // build_session_token
    // =========================================================================

    #[test]
    fn test_build_session_token_returns_valid_token() {
        let secret = b"test-session-secret-32-bytes!!!!";
        let token = build_session_token("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU", secret);
        assert!(token.is_some(), "should produce a token");

        let token_str = token.unwrap();
        assert_eq!(
            token_str.matches('.').count(),
            1,
            "token should have exactly one dot separator"
        );

        let claims =
            crate::session::verify_session_token(&token_str, secret).expect("token should verify");
        assert_eq!(
            claims.wallet,
            "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"
        );
        assert_eq!(claims.budget_remaining, 0);
        assert!(claims.allowed_models.is_empty());
        assert!(claims.expires_at > claims.issued_at);
        assert_eq!(claims.expires_at - claims.issued_at, 3600);
    }

    #[test]
    fn test_build_session_token_with_empty_secret() {
        let token = build_session_token("wallet123", b"");
        assert!(token.is_some());
    }

    // =========================================================================
    // heartbeat integration
    // =========================================================================

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

    // =========================================================================
    // parse_fallback_preference
    // =========================================================================

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
        // "504 Gateway Timeout" matches "timeout" first — intentional;
        // the timeout bucket is more operationally specific.
        assert_eq!(classify_provider_error(&"504 Gateway Timeout"), "timeout");
    }

    #[test]
    fn test_classify_provider_error_unknown() {
        assert_eq!(classify_provider_error(&"something went wrong"), "unknown");
        assert_eq!(classify_provider_error(&"connection refused"), "unknown");
    }
}
