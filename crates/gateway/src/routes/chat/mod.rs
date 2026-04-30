//! POST /v1/chat/completions — OpenAI-compatible chat endpoint.
//!
//! Submodules:
//! - [`cost`] — USDC computation and token estimation
//! - [`payment`] — Payment extraction, validation, escrow claims
//! - [`provider`] — Shared provider call pipeline (cache, fallback, SSE)
//! - [`response`] — Debug headers, session tokens, response construction

pub(crate) mod cost;
mod payment;
mod provider;
mod response;

use std::sync::Arc;
use std::time::Instant;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode};
use axum::response::Response;
use metrics::{counter, histogram};
use tracing::{info, warn};

use solvela_protocol::ChatRequest;
use solvela_router::profiles::{self, Profile};
use solvela_router::scorer;

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
/// 2. Idempotency dedup — short-circuit on identical canonical body within 30 s
/// 3. Check for PAYMENT-SIGNATURE header
/// 4. If missing -> return 402 Payment Required with cost breakdown
/// 5. If present -> verify payment via Facilitator -> proxy to provider -> return response
/// 6. Support both JSON and SSE streaming responses
pub async fn chat_completions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body_bytes: Bytes,
) -> Result<Response, GatewayError> {
    let request_start = Instant::now();
    let debug_enabled = is_debug_enabled(&headers);

    // Parse the JSON body. We accept the raw `Bytes` (rather than the typical
    // `Json` extractor) so we can compute a canonical SHA-256 hash for the
    // request-deduplication cache before deserialization.
    let mut req: ChatRequest = serde_json::from_slice(&body_bytes)
        .map_err(|e| GatewayError::BadRequest(format!("invalid JSON request body: {e}")))?;

    // ── Step 1.5: Idempotency dedup ───────────────────────────────────────
    //
    // If the same canonical request body arrives twice within the dedup TTL
    // we serve the cached response instead of recomputing cost / requiring a
    // new payment. This prevents the double-charge race when an agent times
    // out and retries with a fresh timestamp prefix in a system message.
    //
    // Streaming requests are NOT cached — the cached representation can't
    // be replayed as an SSE stream, and the cap of 30s makes a streaming
    // retry an unusual case.
    let dedup_disabled = crate::cache::request_dedup::is_disabled();
    if dedup_disabled {
        crate::cache::request_dedup::warn_disabled_once();
    }

    let dedup_hash = if !dedup_disabled && !req.stream {
        Some(crate::cache::request_dedup::canonical_hash(&body_bytes))
    } else {
        None
    };

    if let Some(ref hash) = dedup_hash {
        // Try Redis first, then in-memory fallback.
        let cached: Option<crate::cache::request_dedup::CachedResponse> =
            if let Some(cache) = &state.cache {
                crate::cache::request_dedup::redis_get(cache, hash).await
            } else {
                None
            }
            .or_else(|| state.dedup_store.get(hash));

        if let Some(entry) = cached {
            counter!("solvela_dedup_total", "result" => "hit").increment(1);
            info!(
                hash = %hash,
                status = entry.status,
                bytes = entry.body.len(),
                "request dedup cache hit — replaying cached response"
            );
            let mut resp = Response::builder()
                .status(StatusCode::from_u16(entry.status).unwrap_or(StatusCode::OK))
                .header(header::CONTENT_TYPE, &entry.content_type)
                .header(HeaderName::from_static("x-solvela-dedup"), "hit")
                .header(HeaderName::from_static("x-rcr-dedup"), "hit")
                .body(axum::body::Body::from(entry.body))
                .map_err(|e| {
                    GatewayError::Internal(format!("failed to build dedup response: {e}"))
                })?;
            // The middleware-set request-id headers will be applied by the
            // outer layer; nothing else to do here.
            let _ = resp.headers_mut(); // no-op, retained for clarity
            return Ok(resp);
        }
        counter!("solvela_dedup_total", "result" => "miss").increment(1);
    }

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

    // Step 2c: Sanitize tool-use schemas. Some models (notably OpenAI's o3
    // family) reject array-typed parameters that lack an `items` field, so
    // we walk every forwarded tool's parameters and inject a permissive
    // `items: {}` wherever one is missing. Pattern from Franklin
    // `src/mcp/client.ts:53-80`.
    if let Some(tools) = req.tools.as_mut() {
        for tool in tools.iter_mut() {
            if let Some(params) = tool.function.parameters.as_mut() {
                crate::util::schema_sanitize::sanitize_array_items(params);
            }
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
        counter!("solvela_payments_total", "status" => "dev_bypass").increment(1);

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

        let result = provider::execute_provider_call(&ctx)
            .await
            .map(|r| r.response)
            .map_err(|e| match e {
                ProviderCallError::AllProvidersFailed { model, error, .. } => {
                    let rid = request_id
                        .clone()
                        .unwrap_or_else(crate::error::fresh_request_id);
                    tracing::error!(
                        request_id = %rid,
                        model = %model,
                        error = %error,
                        "all providers failed (dev bypass)"
                    );
                    GatewayError::UpstreamUnavailable(rid)
                }
                ProviderCallError::Internal(msg) => GatewayError::Internal(msg),
            });

        return match result {
            Ok(mut resp) => {
                if let Some(ref hash) = dedup_hash {
                    if resp.status().is_success() && !req.stream {
                        resp = persist_dedup_response(&state, hash, resp).await;
                    }
                }
                Ok(resp)
            }
            Err(e) => Err(e),
        };
    }

    if payment_header.is_none() {
        // Compute cost up front so we can short-circuit zero-cost models
        // before issuing a 402 challenge. README marks `gpt-oss-120b` as Free
        // (input/output cost = 0), and a zero-cost model should never demand
        // payment. The dev-bypass branch above already skips payment; this
        // branch mirrors that flow when the resolved cost is zero.
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

        // Free-tier short-circuit: if the resolved atomic cost is zero,
        // skip payment verification and proxy to the provider directly.
        // Use atomic-integer comparison rather than parsing `cost.total` as
        // f64 (avoids epsilon issues; "0" is the canonical zero string).
        let atomic_zero = atomic_amount == "0";
        if atomic_zero {
            info!(
                model = %req.model,
                "zero-cost model — skipping payment verification"
            );
            counter!("solvela_payments_total", "status" => "free").increment(1);

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
                payment_status: PaymentStatus::Free,
            };

            let result = provider::execute_provider_call(&ctx)
                .await
                .map(|r| r.response)
                .map_err(|e| match e {
                    ProviderCallError::AllProvidersFailed { model, .. } => {
                        let rid = request_id
                            .clone()
                            .unwrap_or_else(crate::error::fresh_request_id);
                        tracing::error!(
                            request_id = %rid,
                            model = %model,
                            "all providers failed for free-tier model"
                        );
                        GatewayError::UpstreamUnavailable(rid)
                    }
                    ProviderCallError::Internal(msg) => GatewayError::Internal(msg),
                });

            return match result {
                Ok(mut resp) => {
                    if let Some(ref hash) = dedup_hash {
                        if resp.status().is_success() && !req.stream {
                            resp = persist_dedup_response(&state, hash, resp).await;
                        }
                    }
                    Ok(resp)
                }
                Err(e) => Err(e),
            };
        }

        // Non-zero cost — return 402 with pricing info
        counter!("solvela_payments_total", "status" => "none").increment(1);
        info!(model = %req.model, "no payment signature, returning 402");

        let mut accepts = vec![solvela_x402::types::PaymentAccept {
            scheme: "exact".to_string(),
            network: solvela_x402::types::SOLANA_NETWORK.to_string(),
            amount: atomic_amount.clone(),
            asset: solvela_x402::types::USDC_MINT.to_string(),
            pay_to: state.config.solana.recipient_wallet.clone(),
            max_timeout_seconds: solvela_x402::types::MAX_TIMEOUT_SECONDS,
            escrow_program_id: None,
        }];

        // Offer escrow scheme if configured
        if state.escrow_claimer.is_some() {
            accepts.push(solvela_x402::types::PaymentAccept {
                scheme: "escrow".to_string(),
                network: solvela_x402::types::SOLANA_NETWORK.to_string(),
                amount: atomic_amount,
                asset: solvela_x402::types::USDC_MINT.to_string(),
                pay_to: state.config.solana.recipient_wallet.clone(),
                max_timeout_seconds: solvela_x402::types::MAX_TIMEOUT_SECONDS,
                escrow_program_id: state.config.solana.escrow_program_id.clone(),
            });
        }

        let payment_required = solvela_x402::types::PaymentRequired {
            x402_version: solvela_x402::types::X402_VERSION,
            resource: solvela_x402::types::Resource {
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
    // Gateway-advertised amount — used as defense-in-depth cap when deposit amount is unknown
    let client_amount: u64;

    match payment_payload {
        Some(payload) => {
            // --- H2: Validate all `accepted` fields ---

            // GHSA-cgqx-mg48-949v: error responses must not echo attacker-controlled
            // payment fields (reflected-injection vector) or expose server-internal
            // values like the recipient wallet. The full mismatch is logged at warn!
            // server-side; the client receives a fixed category string. The accepted
            // payment schemes returned with a 402 already tell the client what the
            // correct values are.

            // Verify the resource URL matches this endpoint
            if payload.resource.url != "/v1/chat/completions" {
                warn!(
                    resource_url = %payload.resource.url,
                    "payment resource URL mismatch"
                );
                return Err(GatewayError::InvalidPayment(
                    "Payment resource does not match this endpoint.".to_string(),
                ));
            }

            // Verify the resource method is POST
            if !payload.resource.method.eq_ignore_ascii_case("POST") {
                warn!(
                    method = %payload.resource.method,
                    "payment resource method mismatch"
                );
                return Err(GatewayError::BadRequest(
                    "Payment resource method must be POST.".to_string(),
                ));
            }

            // Verify network is Solana
            if !payload
                .accepted
                .network
                .eq_ignore_ascii_case(solvela_x402::types::SOLANA_NETWORK)
            {
                warn!(
                    network = %payload.accepted.network,
                    expected = %solvela_x402::types::SOLANA_NETWORK,
                    "payment network mismatch"
                );
                return Err(GatewayError::BadRequest(
                    "Payment network is unsupported. Use the network advertised in the 402 response."
                        .to_string(),
                ));
            }

            // Verify asset is USDC-SPL mint
            if payload.accepted.asset != solvela_x402::types::USDC_MINT {
                warn!(
                    asset = %payload.accepted.asset,
                    expected = %solvela_x402::types::USDC_MINT,
                    "payment asset mismatch"
                );
                return Err(GatewayError::BadRequest(
                    "Payment asset is unsupported. Use the asset advertised in the 402 response."
                        .to_string(),
                ));
            }

            // Verify pay_to matches the gateway's recipient wallet
            if payload.accepted.pay_to != state.config.solana.recipient_wallet {
                warn!(
                    pay_to = %payload.accepted.pay_to,
                    expected = %state.config.solana.recipient_wallet,
                    "payment pay_to mismatch"
                );
                return Err(GatewayError::BadRequest(
                    "Payment recipient does not match. Use the pay_to advertised in the 402 response."
                        .to_string(),
                ));
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
            client_amount = payload.accepted.amount.parse().map_err(|_| {
                warn!(
                    amount = %payload.accepted.amount,
                    "client supplied non-integer payment amount"
                );
                GatewayError::BadRequest(
                    "Payment amount must be a valid integer (atomic USDC units).".to_string(),
                )
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
                ("exact", solvela_x402::types::PayloadData::Escrow(_)) => {
                    return Err(GatewayError::BadRequest(
                        "scheme is 'exact' but payload contains escrow data".to_string(),
                    ));
                }
                ("escrow", solvela_x402::types::PayloadData::Direct(_)) => {
                    return Err(GatewayError::BadRequest(
                        "scheme is 'escrow' but payload contains direct transfer data".to_string(),
                    ));
                }
                _ => {}
            }

            // Track scheme and escrow info
            payment_scheme = payload.accepted.scheme.clone();
            if let solvela_x402::types::PayloadData::Escrow(ref ep) = payload.payload {
                escrow_service_id = Some(ep.service_id.clone());
                escrow_agent_pubkey = Some(ep.agent_pubkey.clone());
            }

            // --- C2: Mandatory replay attack prevention ---
            let tx_raw = match &payload.payload {
                solvela_x402::types::PayloadData::Direct(p) => &p.transaction,
                solvela_x402::types::PayloadData::Escrow(p) => &p.deposit_tx,
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
                // No Redis — fall back to in-memory LRU replay set with TTL.
                //
                // GHSA-fq3f-c8p7-873f: durable-nonce transactions carry a 24-hour replay
                // window. The 10 k-entry LRU cannot reliably cover that window, so we deny
                // the request rather than accept it with degraded replay protection.
                // Regular (recent-blockhash) transactions have a ~90s window and are safe
                // to accept under LRU fallback.
                if is_durable_nonce {
                    // Log only the signature prefix, not the full base64 tx — the
                    // full payload is attacker-controlled and would pollute log
                    // pipelines with arbitrary bytes (Datadog/Loki indexing cost,
                    // log-injection surface).
                    warn!(
                        tx_prefix = &tx_raw[..tx_raw.len().min(88)],
                        "durable-nonce payment rejected: Redis unavailable (GHSA-fq3f-c8p7-873f)"
                    );
                    return Err(GatewayError::InvalidPayment(
                        "Payment service is temporarily degraded; please retry shortly."
                            .to_string(),
                    ));
                }

                // GHSA-wc9q-wc6q-gwmq: recover from poisoned lock instead of panicking,
                // which would propagate a poisoned state to every subsequent payment request.
                // Same pattern as crates/x402/src/fee_payer.rs and a2a/handler.rs.
                let mut replay_set = state
                    .replay_set
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
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
                counter!("solvela_replay_rejections_total").increment(1);
                counter!("solvela_payments_total", "status" => "failed").increment(1);
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
                    counter!("solvela_payments_total", "status" => "failed").increment(1);
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
                    counter!("solvela_payments_total", "status" => "verified").increment(1);
                    histogram!("solvela_payment_amount_usdc")
                        .record(client_amount as f64 / 1_000_000.0);
                    info!(
                        tx_signature = ?settlement.tx_signature,
                        network = %settlement.network,
                        verified_amount = ?settlement.verified_amount,
                        "payment verified and settled"
                    );
                }
                Err(e) => {
                    // GHSA-cgqx-mg48-949v: do not echo the verifier error to clients;
                    // it can carry the internal RPC URL, raw RPC error JSON, and other
                    // server-internal context. Full detail is in the warn! line above.
                    counter!("solvela_payments_total", "status" => "failed").increment(1);
                    warn!(error = %e, "payment verification failed");
                    return Err(GatewayError::InvalidPayment(
                        "Payment verification failed. Check your transaction and retry."
                            .to_string(),
                    ));
                }
            }
        }
        None => {
            counter!("solvela_payments_total", "status" => "failed").increment(1);
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

    // GHSA-86cr-h3rx-vj6j: budget-enforcement guard. A corrupted model registry
    // entry can produce NaN or ±Infinity in the cost total. NaN parses back to
    // f64::NAN successfully, then every comparison in `check_budget` against
    // wallet limits is `false` — silently bypassing the budget gate. Reject
    // non-finite or negative values here, fail-closed, before the gate runs.
    if !estimated_cost.is_finite() || estimated_cost < 0.0 {
        warn!(
            estimated_cost,
            model = %req.model,
            "model registry produced a non-finite or negative cost; refusing"
        );
        return Err(GatewayError::Internal(
            "estimated cost is not a valid finite non-negative number".to_string(),
        ));
    }

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
                            .insert(HeaderName::from_static("x-solvela-session"), hv.clone());
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
                        .unwrap_or_else(|e| {
                            warn!(
                                error = %e,
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
                    client_amount,
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
                            solvela_router::models::ModelRegistryError::ParseError(e.to_string())
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

            // Persist the response in the dedup cache so retries within the
            // TTL replay this exact response instead of re-executing. Only
            // cache non-streaming successful responses with a 2xx status.
            if let Some(ref hash) = dedup_hash {
                if response.status().is_success() && !req.stream {
                    response = persist_dedup_response(&state, hash, response).await;
                }
            }

            Ok(response)
        }
        Err(ProviderCallError::AllProvidersFailed {
            model, provider, ..
        }) => {
            // SECURITY: A paid request reached the stub path — all providers failed.
            // Verbose context goes to the structured log; the public response
            // body returns a generic message plus a correlation id so support
            // can find the matching log line without leaking internal detail.
            let rid = request_id
                .clone()
                .unwrap_or_else(crate::error::fresh_request_id);
            tracing::error!(
                request_id = %rid,
                provider = %provider,
                model = %model,
                wallet = %wallet_address,
                tx_signature = ?tx_signature,
                "paid request failed: no provider available — all upstream providers exhausted"
            );
            counter!("solvela_paid_stub_rejections_total").increment(1);

            Err(GatewayError::UpstreamUnavailable(rid))
        }
        Err(ProviderCallError::Internal(msg)) => Err(GatewayError::Internal(msg)),
    }
}

/// Buffer the response body, store it in the dedup cache (Redis or in-memory),
/// and return a fresh `Response` reconstituted from those bytes.
///
/// On any I/O failure the original response cannot be reconstructed, so we
/// log the failure and surface a generic 500 response. Callers must check
/// `response.status().is_success()` before calling this.
async fn persist_dedup_response(state: &Arc<AppState>, hash: &str, response: Response) -> Response {
    let (parts, body) = response.into_parts();
    let bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b,
        Err(e) => {
            warn!(error = %e, "failed to buffer response body for dedup cache");
            // Reconstruct a minimal error response — original body is gone.
            return Response::from_parts(
                parts,
                axum::body::Body::from(
                    serde_json::json!({
                        "error": {
                            "type": "upstream_error",
                            "code": "internal_error",
                            "message": "failed to buffer response body"
                        }
                    })
                    .to_string(),
                ),
            );
        }
    };

    let content_type = parts
        .headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json")
        .to_string();

    let entry = crate::cache::request_dedup::CachedResponse {
        body: bytes.to_vec(),
        content_type,
        status: parts.status.as_u16(),
    };

    // Store in Redis (fire-and-forget) and the in-memory fallback.
    if let Some(cache) = &state.cache {
        crate::cache::request_dedup::redis_put(cache, hash, &entry).await;
    }
    state.dedup_store.put(hash.to_string(), entry);

    Response::from_parts(parts, axum::body::Body::from(bytes))
}

/// Resolve model ID from aliases, smart routing profiles, or direct model IDs.
///
/// Returns (resolved_model, profile_name, tier_name, score) for debug headers.
fn resolve_model_with_debug(
    req: &ChatRequest,
    state: &AppState,
) -> Result<(String, String, String, f64), GatewayError> {
    // Demo-provider short-circuit: when the requested model is the demo
    // model itself, OR when `auto` is requested and the demo is the only
    // registered provider, route directly to the demo. This lets a fresh
    // clone of the repo answer requests without any provider API keys.
    // See `providers/demo.rs` for activation rules.
    if crate::providers::demo::is_demo_model(&req.model)
        || (req.model.eq_ignore_ascii_case("auto") && demo_only_providers(state))
    {
        return Ok((
            crate::providers::demo::DEMO_MODEL_ID.to_string(),
            "demo".to_string(),
            "N/A".to_string(),
            0.0,
        ));
    }

    // Implicit agentic profile: when the inbound request carries a non-empty
    // `tools` array AND the caller didn't pin a specific model, route
    // through the agentic profile so we pick a tool-fidelity-strong model.
    // Prior art: ClawRouter's `agenticTask` dimension and Franklin's
    // AGENTIC keyword scoring (`src/router/index.ts:174-284`).
    let has_tools = req.tools.as_ref().is_some_and(|tools| !tools.is_empty());
    let is_profile_alias = Profile::from_alias(&req.model).is_some();
    if has_tools && is_profile_alias {
        // Override the requested profile with `agentic` whenever tools are
        // present. Explicit `model: "agentic"` callers also land here.
        let result = scorer::classify(&req.messages, true);
        let model_id = profiles::resolve_model(profiles::Profile::Agentic, result.tier);
        return Ok((
            model_id.to_string(),
            "agentic".to_string(),
            format!("{:?}", result.tier),
            result.score,
        ));
    }

    // Check for profile-based routing (e.g., "auto", "eco", "premium", "agentic")
    if let Some(profile) = Profile::from_alias(&req.model) {
        let result = scorer::classify(&req.messages, has_tools);
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

/// `true` when the only registered provider is the demo provider.
fn demo_only_providers(state: &AppState) -> bool {
    let configured = state.providers.configured_providers();
    configured.len() == 1 && configured[0] == crate::providers::demo::DEMO_PROVIDER_NAME
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
        let name = HeaderName::from_static("x-solvela-fallback");
        assert_eq!(name.as_str(), "x-solvela-fallback");
        let legacy = HeaderName::from_static("x-rcr-fallback");
        assert_eq!(legacy.as_str(), "x-rcr-fallback");
    }
}
