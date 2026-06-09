//! POST /v1/chat/completions — OpenAI-compatible chat endpoint.
//!
//! Submodules:
//! - [`cost`] — USDC computation and token estimation
//! - [`payment`] — Payment extraction, validation, escrow claims
//! - [`provider`] — Shared provider call pipeline (cache, fallback, SSE)
//! - [`response`] — Debug headers, session tokens, response construction

pub(crate) mod cost;
pub(crate) mod payment;
mod provider;
mod response;

use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use axum::response::Response;
use axum::Json;
use metrics::{counter, histogram};
use tracing::{debug, info, warn};

use solvela_protocol::{ChatRequest, MessageContent, Role, SettlementFailureKind};
use solvela_router::profiles::{self, Profile};
use solvela_router::scorer;

use crate::error::GatewayError;
use crate::middleware::prompt_guard::{self, GuardResult, PromptGuardConfig};
use crate::routes::debug_headers::{is_debug_enabled, PaymentStatus};
use crate::usage::SpendLogEntry;
use crate::AppState;

use cost::{
    cap_usage_to_request_limits, completion_token_ceiling, compute_actual_atomic_cost,
    estimate_input_tokens, estimated_atomic_cost, is_free_estimate, scheme_realized_discount,
    spend_cost_atomic, usdc_atomic_amount_checked, usdc_f64_to_atomic_safe, PaymentScheme,
};
use payment::{decode_payment_from_header, extract_payment_info, fire_escrow_claim};
use provider::{ProviderCallContext, ProviderCallError, ProviderCallResult};
use response::{build_session_token, validate_session_id, validate_tenant};

// Re-export `uses_durable_nonce` for use by `crate::routes::proxy`
pub(crate) use payment::uses_durable_nonce;

/// Maximum number of messages allowed in a single chat request.
/// Prevents excessive memory usage and cost from very long conversations.
const MAX_MESSAGES: usize = 256;

/// Maximum number of content parts allowed in a single message's `Parts`
/// content array. Defense-in-depth against part-flooding within the 10MB
/// body limit (each part is small individually but a multi-thousand-element
/// array forces excessive per-part work on the hot path).
const MAX_CONTENT_PARTS: usize = 64;

/// Maximum number of image parts allowed across an ENTIRE request.
///
/// `MAX_MESSAGES` (256) × `MAX_CONTENT_PARTS` (64) otherwise admits up to ~16K
/// image parts, and URL images are ~30 bytes each so thousands fit inside the
/// 10MB body limit. Each image also adds a per-image token contribution to the
/// upfront 402 estimate, so an unbounded count inflates the quote and forces
/// per-image work on the hot path. 100 is the tightest hard limit across the
/// supported vision providers (Anthropic's API caps a request at 100 images),
/// so an accepted count never settles payment then gets rejected upstream for
/// image-count. Enforced BEFORE payment.
const MAX_IMAGES_PER_REQUEST: usize = 100;

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
    // Infallible peer-address extractor. Yields `None` when `ConnectInfo` is
    // absent (e.g. `oneshot` integration tests, proxy misconfig) so the free-tier
    // path degrades to the stricter "unknown" bucket rather than 500-ing.
    peer_addr: crate::middleware::rate_limit::PeerAddr,
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

    // Content-shape validation — runs BEFORE payment, guard, and provider
    // dispatch. Caps the per-message parts array to bound hot-path work.
    //
    // The IMAGE capability gate is deliberately NOT here: it needs the
    // resolved model's `supports_vision` flag, which is only known after model
    // resolution + registry lookup below. Image content for a non-vision model
    // is rejected there (415); for a vision model it is translated to the
    // provider's native multimodal format. Note: the empty-prompt check below
    // still requires at least one non-empty User *text* message, so an
    // image-only request with no text is rejected as an empty prompt.
    for msg in &req.messages {
        if let MessageContent::Parts(parts) = &msg.content {
            if parts.len() > MAX_CONTENT_PARTS {
                return Err(GatewayError::BadRequest(format!(
                    "too many content parts in a message: {} exceeds maximum of {}",
                    parts.len(),
                    MAX_CONTENT_PARTS
                )));
            }
        }
    }

    // Aggregate image-count cap — runs BEFORE payment and model resolution so a
    // request carrying more images than any provider accepts is a client 4xx,
    // never billed-then-rejected upstream. Bounds the abuse case where thousands
    // of small URL images fit inside the body limit (see MAX_IMAGES_PER_REQUEST).
    let total_images: usize = req
        .messages
        .iter()
        .map(|msg| msg.content.image_urls().count())
        .sum();
    if total_images > MAX_IMAGES_PER_REQUEST {
        return Err(GatewayError::BadRequest(format!(
            "too many images: {total_images} exceeds maximum of {MAX_IMAGES_PER_REQUEST}"
        )));
    }

    // Empty-prompt rejection — runs BEFORE payment so an empty request is never
    // billed (the cost path floors token estimates at `.max(1)`, and a
    // `Parts([])` value additionally serializes to `"content":[]`, which
    // OpenAI-format providers 400 on — i.e. a paid request would 5xx AFTER the
    // agent settled on-chain).
    //
    // null/absent content deserializes to `Text("")`, and all-whitespace or
    // image-only `Parts` flatten to empty text; all of those pass the gates
    // above. Reject when NO `User`-role message carries non-empty text — i.e.
    // there is no actual user prompt anywhere in the request.
    //
    // Assistant turns with `content: null` + `tool_calls` (legitimate OpenAI
    // multi-turn), and System/Developer/Tool messages, are NOT user prompts and
    // do not satisfy this check on their own — but they are never the *reason*
    // for rejection either, since we only require at least one non-empty User
    // message to exist.
    let has_user_prompt = req
        .messages
        .iter()
        .any(|msg| msg.role == Role::User && !msg.content.as_text().trim().is_empty());
    if !has_user_prompt {
        warn!("rejected request with no non-empty user prompt content");
        return Err(GatewayError::BadRequest(
            "request contains no user message with non-empty text content".to_string(),
        ));
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

    // Extract and validate the x-tenant attribution tag. Unauthenticated,
    // free-form header set by a trusted upstream proxy; recorded on the spend
    // row for reporting only. Attribution-only — it never gates the request, so
    // an invalid value simply yields `None` (untagged) rather than an error.
    let tenant: Option<String> = headers
        .get("x-tenant")
        .and_then(|v| v.to_str().ok())
        .and_then(validate_tenant);

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

    // Step 1b: Prompt guard — check for injection, jailbreak, and PII.
    //
    // security 92 / DoS amplification: the guard does expensive scans (notably a
    // large allocation on the PII path). It MUST NOT run on the unauthenticated
    // no-payment 402 path — that path only needs a cost estimate, and running the
    // guard there lets any anonymous caller force full guard work for free. So
    // the guard is deferred and invoked only on paths that actually reach a
    // provider: the dev-bypass branch and the verified/paid path below. Defining
    // it as a closure keeps a single source of truth for both call sites.
    let run_prompt_guard =
        |messages: &[solvela_protocol::ChatMessage]| -> Result<(), GatewayError> {
            let guard_config = PromptGuardConfig::default();
            match prompt_guard::check(messages, &guard_config) {
                GuardResult::Blocked { reason } => {
                    warn!(reason = %reason, "request blocked by prompt guard");
                    Err(GatewayError::BadRequest(
                        "Request blocked by content policy".to_string(),
                    ))
                }
                GuardResult::PiiDetected { fields } => {
                    warn!(
                        pii_fields = ?fields,
                        "PII detected in request — forwarding with warning logged"
                    );
                    Ok(())
                }
                GuardResult::Clean => Ok(()),
            }
        };

    // Step 2: Look up model in registry for pricing
    let model_info = state
        .model_registry
        .get(&req.model)
        .ok_or_else(|| GatewayError::ModelNotFound(req.model.clone()))?;

    // Step 2a: Vision capability gate. Image content is only accepted for a
    // model whose registry entry declares `supports_vision`. A non-vision model
    // must reject (415) rather than silently strip the image (which would
    // change the prompt's meaning while still billing the agent) or forward it
    // to a provider that doesn't accept it (a paid 5xx after settlement). Runs
    // BEFORE payment so an unsupported request is never billed.
    let request_has_images = req.messages.iter().any(|m| m.content.has_image_parts());
    if request_has_images && !model_info.supports_vision {
        warn!(
            model = %req.model,
            "rejected image content for a non-vision model"
        );
        return Err(GatewayError::UnsupportedMediaType(format!(
            "model '{}' does not support image input; \
             use a vision-capable model or send text-only content",
            req.model
        )));
    }

    // Step 2a': Validate every image part at the route BEFORE payment, so a
    // malformed image (bad scheme, non-base64 data URI, oversize, unsupported
    // media type) is a client 4xx and the agent is never billed for it — rather
    // than surfacing as a post-payment provider 5xx. This is the single route
    // chokepoint; the adapters re-validate as defense-in-depth. Also reject
    // images in system/developer messages here (the provider `system` channels
    // are text-only and would otherwise silently drop the image after payment).
    //
    // Image parts in user/assistant messages are always allowed (a valid
    // multi-turn vision conversation carries images in those roles). Tool-role
    // images are accepted here at the protocol layer — Gemini forwards them as
    // user-role parts — but provider support varies: the Anthropic adapter
    // rejects tool-role images (with a clear error) rather than silently
    // dropping them, since it does not yet translate tool results to
    // `tool_result` blocks. Only system/developer-role images are rejected
    // unconditionally below.
    //
    // Per-image bytes are bounded by `MAX_DATA_URI_BYTES` inside `parse()`;
    // total image bytes across the whole request are additionally bounded by the
    // 10 MB `RequestBodyLimitLayer` on the HTTP body (see lib.rs `build_router`),
    // so no separate aggregate image-byte cap is needed here.
    if request_has_images {
        for msg in &req.messages {
            let is_system = matches!(msg.role, Role::System | Role::Developer);
            if is_system && msg.content.has_image_parts() {
                return Err(GatewayError::BadRequest(
                    "image content is not supported in system/developer messages; \
                     place images in a user message"
                        .to_string(),
                ));
            }
            for image_url in msg.content.image_urls() {
                if let Err(e) = image_url.parse() {
                    return Err(GatewayError::BadRequest(format!("invalid image: {e}")));
                }
            }
        }
    }

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
        counter!("solvela_payments_total", "status" => "dev_bypass").increment(1);

        // Dev-bypass still reaches a provider, so it must run the guard.
        run_prompt_guard(&req.messages)?;

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

    // Compute the upfront cost estimate ONCE. This same `atomic_amount` is the
    // single source of truth for two decisions below:
    //   1. free-ness (is_free_estimate → zero-cost bypass), and
    //   2. the amount advertised in the 402 `accepts[]` for a paid model.
    // Deriving both from one estimate means a pricing change can never make a
    // paid model silently bypass payment (or a free model wrongly 402).
    //
    // FINDING 1: the free-ness decision is hoisted ABOVE the `payment_header`
    // branch so it is reachable whether or not a payment header is present. A
    // free (estimate == 0) request must be SERVED at $0 regardless of any
    // header — previously the bypass lived inside `if payment_header.is_none()`,
    // so a free model carrying a (stray/legacy) payment header fell through to
    // decode/verify and failed with InvalidPayment. The quoted amount for a free
    // model is 0, so there is nothing to claim; a present header is simply
    // ignored. `is_free_estimate` fails closed on any non-"0" value, so a paid
    // model can never take this path.
    let cost = state
        .model_registry
        .estimate_cost(
            &req.model,
            estimate_input_tokens(&req),
            // #500: reserve for the SAME completion-token ceiling billing
            // will cap to (not a flat 1000), so an omitted-max_tokens
            // request can never bill above its reservation.
            completion_token_ceiling(req.max_tokens, model_info),
        )
        .map_err(|e| GatewayError::Internal(e.to_string()))?;

    let atomic_amount = usdc_atomic_amount_checked(&cost.total).map_err(|e| {
        GatewayError::Internal(format!(
            "failed to compute USDC atomic amount for model '{}': {}",
            req.model, e
        ))
    })?;

    // ── Zero-cost (free-tier) bypass ──────────────────────────────────────────
    //
    // A model is free IFF its computed estimate atomic cost is exactly 0
    // (e.g. a 0.0/0.0-priced model). Such a request must be SERVED, not 402'd
    // for a $0 payment, and never reaches payment decode/verify/settlement —
    // there is nothing to charge or claim at $0. A paid model (atomic > 0) falls
    // through to the 402 / paid path below unchanged — `is_free_estimate` fails
    // closed on any non-"0" value, so it is the single source of truth for
    // free-ness and a paid request can never accidentally take this branch.
    if is_free_estimate(&atomic_amount) {
        // Anti-abuse: the free path is anonymous (no payer wallet), so it is
        // rate-limited per client IP, STRICTER than the paid limit. Enforced
        // BEFORE any provider work so a limited free request never reaches the
        // upstream provider quota. Keyed on the TCP peer IP (never a
        // client-supplied header — GHSA-6ggq-cvwx-4f67); absent ConnectInfo
        // falls back to the shared stricter "unknown" bucket.
        let free_client_id = crate::middleware::rate_limit::connect_info_client_id(peer_addr.0);
        // FINDING 2: surface the `unknown` bucket collapse for operators. When
        // ConnectInfo is absent every free client keys to one shared bucket
        // (a trivial free-tier DoS under proxy misconfig). Production always
        // sets ConnectInfo (`into_make_service_with_connect_info` in main.rs),
        // so this counter staying at 0 confirms correct keying; a nonzero value
        // is an alertable misconfiguration. This does NOT change the limiting
        // behavior — only its observability.
        if free_client_id == "unknown" {
            counter!("solvela_free_tier_unknown_bucket_total").increment(1);
        }
        if state
            .free_rate_limiter
            .check(&free_client_id)
            .await
            .is_err()
        {
            counter!("solvela_payments_total", "status" => "free_rate_limited").increment(1);
            warn!(
                client_id = %free_client_id,
                model = %req.model,
                "free-tier rate limit exceeded"
            );
            return Ok(crate::middleware::rate_limit::rate_limited_response(
                state.free_rate_limiter.config(),
            ));
        }

        // Gate order on the free path is deliberate:
        //   1. per-IP free check (above) — cheap, in-memory, rejects a single
        //      spamming IP without a Redis round-trip.
        //   2. aggregate cap (here) — global across ALL clients, bounds total
        //      free throughput below the upstream provider's SHARED ceiling
        //      (the per-IP check can't: many distinct IPs each under their
        //      per-IP cap can still collectively exceed Google's ~15 RPM).
        //   3. prompt guard → provider (below).
        // Both rate gates run BEFORE any provider call so a rejected request
        // never reaches the upstream free-tier quota. The cap uses Redis when
        // configured (cross-instance) and degrades to in-memory on Redis error
        // (it never goes unbounded); see `FreeTierGlobalCap::check`.
        if state
            .free_global_cap
            .check(state.cache.as_ref())
            .await
            .is_err()
        {
            counter!("solvela_payments_total", "status" => "free_global_rate_limited").increment(1);
            warn!(
                model = %req.model,
                cap = state.free_global_cap.cap(),
                "free-tier aggregate (global) rate cap exceeded — returning gateway 429 before upstream provider 429s"
            );
            return Ok(crate::middleware::rate_limit::rate_limited_response(
                &state.free_global_cap.as_rate_limit_config(),
            ));
        }

        counter!("solvela_payments_total", "status" => "free").increment(1);
        // FINDING 1: a present payment header on a free model is ignored — the
        // quoted amount is 0, so there is nothing to verify, settle, or claim.
        // The request is served at $0 and never touches the payment path.
        if payment_header.is_some() {
            debug!(
                model = %req.model,
                "free model carried a payment header — ignored (quoted amount is 0, nothing to claim)"
            );
        }
        info!(model = %req.model, "zero-cost model — serving free-tier request at $0");

        // Free requests reach a provider, so they must run the guard (same as
        // the dev-bypass and paid paths).
        run_prompt_guard(&req.messages)?;

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

        return match provider::execute_provider_call(&ctx).await {
            Ok(result) => {
                // Log spend at $0 via the existing fire-and-forget path so the
                // free tier still shows up in usage/observability. `cost_usdc`
                // is 0.0 and `estimated_cost_usdc` is None, so `log_spend`
                // increments counters by 0 (no divide-by-zero / no fail-closed
                // path is reachable for a zero amount). Anonymous: there is no
                // payer wallet, so a sentinel marks the row as free-tier.
                let usage = result
                    .usage
                    .as_ref()
                    .map(|u| cap_usage_to_request_limits(u, &req, model_info));
                state.usage.log_spend(SpendLogEntry {
                    wallet_address: "free-tier".to_string(),
                    model: req.model.clone(),
                    provider: result
                        .actual_provider
                        .unwrap_or_else(|| model_info.provider.clone()),
                    input_tokens: usage.as_ref().map(|u| u.prompt_tokens).unwrap_or(0),
                    output_tokens: usage.as_ref().map(|u| u.completion_tokens).unwrap_or(0),
                    cost_usdc: 0.0,
                    tx_signature: None,
                    request_id: request_id.clone(),
                    session_id: session_id.clone(),
                    tenant: tenant.clone(),
                    tenant_enforced: false,
                    estimated_cost_usdc: None,
                });
                Ok(result.response)
            }
            Err(e) => Err(match e {
                ProviderCallError::AllProvidersFailed { model, error, .. } => {
                    GatewayError::Internal(format!(
                        "all providers failed for free model '{}': {}",
                        model, error
                    ))
                }
                ProviderCallError::Internal(msg) => GatewayError::Internal(msg),
            }),
        };
    }

    // Not free (estimate > 0). A paid model with NO payment header returns 402.
    // (A paid model WITH a header falls through to decode/verify/settle below.)
    if payment_header.is_none() {
        // Return 402 with pricing info (paid model, no payment header)
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

        // Emit the PaymentRequired body at the top level of the 402 response
        // per x402 spec — NOT wrapped in the OpenAI-style error envelope.
        // See `GatewayError::PaymentChallenge` doc and issue #217.
        return Err(GatewayError::PaymentChallenge(Box::new(payment_required)));
    }

    // Payment header is present — this request will reach a provider on success,
    // so run the (deferred) prompt guard now, before decode/verify/proxy. This is
    // the security-92 reorder: the guard runs for every request that reaches a
    // provider, and never on the pure no-payment 402 return above.
    run_prompt_guard(&req.messages)?;

    // Step 4: Payment present — try to decode and verify via Facilitator
    let payment_payload = decode_payment_from_header(payment_header.unwrap());

    // Track escrow-specific info for post-response claim
    let payment_scheme: PaymentScheme;
    let mut escrow_service_id: Option<String> = None;
    let mut escrow_agent_pubkey: Option<String> = None;
    // FIX 3: Track the verified deposit amount to cap claim amounts
    let escrow_deposited_amount: Option<u64>;
    // #486: for the `exact` scheme the on-chain broadcast is DEFERRED until after
    // a successful provider response, so we hold the verified payload here to
    // settle it post-call. `None` for escrow (its deposit already settled).
    let mut payload_for_settle: Option<solvela_x402::types::PaymentPayload> = None;
    // Gateway-advertised amount — used as defense-in-depth cap when deposit amount is unknown
    let client_amount: u64;
    // M3: budget is checked + reserved BEFORE settlement inside the arm below, so
    // an over-budget request never settles on-chain. These are bound there and
    // consumed after the match (the `None` arm diverges).
    let wallet_address: String;
    let tx_signature: Option<String>;
    let estimated_cost: f64;
    let budget_reservation: crate::usage::BudgetReservation;

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
                    // #500: same completion-token ceiling as the 402 quote above
                    // and as the settlement-time `cap_usage_to_request_limits`.
                    // This figure feeds both the client-amount validation (C1)
                    // and the budget reservation (M3), so the reservation is an
                    // upper bound on the billable cost — an omitted-max_tokens
                    // request cannot reserve under cap and then overshoot via the
                    // `log_spend` reconciliation delta.
                    completion_token_ceiling(req.max_tokens, model_info),
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

            // Track scheme and escrow info. Parse the scheme at the boundary so
            // every downstream financial branch (discount gate, escrow claim,
            // spend ledger) operates on an exhaustive enum, not a free string:
            // a typo or future scheme becomes a 400 here rather than silently
            // mis-routing through the financial path.
            payment_scheme =
                PaymentScheme::from_accepted_str(&payload.accepted.scheme).map_err(|e| {
                    warn!(scheme = %payload.accepted.scheme, "unknown payment scheme");
                    GatewayError::BadRequest(e)
                })?;
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
                    .for_path(crate::ReplayPath::Chat)
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
                        tx_prefix = &tx_raw[..tx_raw.len().min(88)],
                        "payment accepted under degraded in-memory replay protection (no Redis)"
                    );
                    false
                }
            };

            if replay_detected {
                counter!("solvela_replay_rejections_total").increment(1);
                counter!("solvela_payments_total", "status" => "failed").increment(1);
                warn!(
                    tx_prefix = &tx_raw[..tx_raw.len().min(88)],
                    "replay attack detected — transaction already used"
                );
                return Err(GatewayError::InvalidPayment(
                    "transaction has already been used; each payment signature may only be submitted once".to_string()
                ));
            }

            // --- M3: check + reserve budget BEFORE settling on-chain ---
            // The wallet's spend limit must be enforced before the payment is
            // broadcast. Otherwise an over-budget request settles on-chain and is
            // then rejected with a 4xx — the agent pays and gets no service. The
            // reservation is released on any settlement failure below so reserving
            // early never leaks budget for a payment that did not happen.
            (wallet_address, tx_signature) = extract_payment_info(payment_header.unwrap());

            // Reuse the C1 `expected_cost` computed above rather than calling
            // `estimate_cost` again — one hot-path computation, and the budget
            // gate is guaranteed to reserve against the exact figure the payment
            // amount was validated against (no boundary divergence between the
            // two sites).
            estimated_cost = expected_cost.total.parse::<f64>().map_err(|_| {
                GatewayError::Internal("failed to parse estimated cost as f64".to_string())
            })?;

            // GHSA-86cr-h3rx-vj6j: budget-enforcement guard. A corrupted model
            // registry entry can produce NaN or ±Infinity in the cost total. NaN
            // parses back to f64::NAN successfully, then every comparison in
            // `check_budget` against wallet limits is `false` — silently bypassing
            // the budget gate. Reject non-finite or negative values here,
            // fail-closed, before the gate (and before settlement) runs.
            if !estimated_cost.is_finite() || estimated_cost < 0.0 {
                warn!(
                    estimated_cost,
                    model = %req.model,
                    "model registry produced a non-finite or negative cost; refusing"
                );
                // No reservation committed yet — this guard fires BEFORE
                // `check_budget`, so there is nothing to release here. Keep this
                // guard ahead of the reserve: moving it after would turn this
                // early return into a budget leak.
                return Err(GatewayError::Internal(
                    "estimated cost is not a valid finite non-negative number".to_string(),
                ));
            }

            // Pass the (forgeable, attribution-only) x-tenant tag so the
            // per-tenant budget gate can enforce a provisioned `(wallet, tenant)`
            // bucket and apply the require_tenant fail-closed policy. All tenant
            // rejections (TenantRequired / TenantNotProvisioned) and budget
            // overruns map to a 400 via BadRequest below — and they fire here,
            // pre-settlement / pre-provider-call.
            budget_reservation = match state
                .usage
                .check_budget(&wallet_address, estimated_cost, tenant.as_deref())
                .await
            {
                Ok(reservation) => reservation,
                Err(e) => return Err(GatewayError::BadRequest(e.to_string())),
            };

            // #486: charge-before-deliver fix. The settlement step broadcasts
            // the customer's payment on-chain. Doing that BEFORE the provider
            // call means a provider failure leaves the customer charged with no
            // completion — and on `exact` there is no refund path. We split by
            // scheme:
            //
            //   - `exact`: VERIFY only here (validate + simulate, NO broadcast).
            //     The transfer is deferred and broadcast AFTER a successful
            //     provider response (see `settle_exact` below). If the provider
            //     fails, settlement never happens → the customer is not charged.
            //
            //   - `escrow`: the deposit MUST be on-chain before serving (the
            //     scheme's trustless commitment), so we still verify_and_settle
            //     the deposit here. The no-charge lever for escrow is the CLAIM:
            //     on provider failure we simply do not claim, and the deposit
            //     refunds at expiry (the claim only fires in the success arm).
            //
            // `verify_payment` is non-mutating and repeatable, so verifying now
            // for `exact` and settling later does not double-anything; replay
            // protection above already guards against re-submission of the tx.
            match payment_scheme {
                PaymentScheme::Exact => {
                    match state.facilitator.verify(&payload).await {
                        Ok(verification) if !verification.valid => {
                            counter!("solvela_payments_total", "status" => "failed").increment(1);
                            warn!(
                                reason = ?verification.reason,
                                "exact payment verification rejected (pre-settlement)"
                            );
                            state.usage.release_reservation(&budget_reservation).await;
                            return Err(GatewayError::InvalidPayment(
                                "Payment verification failed. Check your transaction and retry."
                                    .to_string(),
                            ));
                        }
                        Ok(verification) => {
                            // Deferred settlement: no broadcast yet. `verified_amount`
                            // is unused for `exact` (no escrow claim), so this stays
                            // `None`. The actual broadcast happens post-response via
                            // the held `payload_for_settle`. Settlement on `exact`
                            // bills the `client_amount` the agent signed, NOT the
                            // verifier's `verified_amount` — log the intentional
                            // divergence rather than silently dropping a money-path
                            // field.
                            debug!(
                                verified_amount = ?verification.verified_amount,
                                client_amount,
                                "exact: settlement uses client_amount, not verified_amount"
                            );
                            escrow_deposited_amount = None;
                            payload_for_settle = Some(payload.clone());
                            counter!("solvela_payments_total", "status" => "verified").increment(1);
                            info!("exact payment verified (settlement deferred until provider delivers)");
                        }
                        Err(e) => {
                            // GHSA-cgqx-mg48-949v: do not echo verifier internals.
                            counter!("solvela_payments_total", "status" => "failed").increment(1);
                            warn!(error = %e, "exact payment verification failed (pre-settlement)");
                            state.usage.release_reservation(&budget_reservation).await;
                            return Err(GatewayError::InvalidPayment(
                                "Payment verification failed. Check your transaction and retry."
                                    .to_string(),
                            ));
                        }
                    }
                }
                // Escrow: deposit must land on-chain before serving. Verify and
                // settle (broadcast) the deposit now — hard enforcement.
                // R1 FIX: Check settlement.success flag
                PaymentScheme::Escrow => {
                    match state.facilitator.verify_and_settle(&payload).await {
                        Ok(settlement) if !settlement.success => {
                            // Settlement returned Ok but the transaction did not succeed.
                            // Distinguish a deterministic on-chain rejection (a dead end —
                            // retrying the same signed tx can never confirm) from a
                            // transient timeout/transport failure (issue #435). The full
                            // error detail stays server-side; the client only ever sees the
                            // numeric program error code, never raw RPC internals
                            // (GHSA-cgqx-mg48-949v).
                            counter!("solvela_payments_total", "status" => "failed").increment(1);
                            tracing::warn!(
                                tx_signature = %settlement.tx_signature.as_deref().unwrap_or("unknown"),
                                error = ?settlement.error,
                                failure_kind = ?settlement.failure_kind,
                                "payment settlement failed"
                            );
                            // Exhaustive on purpose — no `_` wildcard, so a future
                            // `SettlementFailureKind` variant can't be silently funneled
                            // into the retryable bucket and re-open #435.
                            let message = match settlement.failure_kind {
                                Some(SettlementFailureKind::Rejected { program_error_code }) => {
                                    match program_error_code {
                                        Some(code) => format!(
                                    "Payment was rejected on-chain (program error {code}); \
                                     this transaction cannot succeed and should not be retried."
                                ),
                                        None => "Payment was rejected on-chain; this transaction \
                                     cannot succeed and should not be retried."
                                            .to_string(),
                                    }
                                }
                                // Transient failures, plus the unclassified `None` case
                                // (only reachable from an external verifier — retry is the
                                // safe default since replay protection prevents double-spend).
                                Some(SettlementFailureKind::Timeout)
                                | Some(SettlementFailureKind::Submission)
                                | None => {
                                    "Payment transaction could not be confirmed. Please retry."
                                        .to_string()
                                }
                            };
                            // M3: settlement did not happen — give the reserved budget back.
                            state.usage.release_reservation(&budget_reservation).await;
                            return Err(GatewayError::InvalidPayment(message));
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
                            // M3: settlement did not happen — give the reserved budget back.
                            state.usage.release_reservation(&budget_reservation).await;
                            return Err(GatewayError::InvalidPayment(
                                "Payment verification failed. Check your transaction and retry."
                                    .to_string(),
                            ));
                        }
                    }
                } // end PaymentScheme::Escrow arm
            } // end match payment_scheme
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

    // `wallet_address`, `tx_signature`, `estimated_cost`, and `budget_reservation`
    // were bound inside the Some arm above — budget is now checked + reserved
    // BEFORE settlement (M3), and the reservation reconciled by `log_spend`'s
    // `(billed − estimated)` delta on success.

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
            cost_outcome,
        }) => {
            // Cap provider-reported token counts to what the gateway has
            // actually priced for (req.max_tokens, model context window).
            // Without this, a misbehaving / compromised provider could
            // inflate completion_tokens to over-bill the agent, or deflate
            // prompt_tokens to under-bill the gateway. Both flow into the
            // escrow claim and spend log below.
            let usage = usage
                .as_ref()
                .map(|u| cap_usage_to_request_limits(u, &req, model_info));

            // #486: deferred `exact` settlement. The provider delivered, so it is
            // now safe to broadcast the customer's transfer on-chain. We do this
            // BEFORE returning the response. A broadcast failure here is logged
            // but does NOT fail the request: the provider already produced output
            // and `verify` proved the tx valid + simulated successful on-chain;
            // the (rare) residual of delivery-without-charge is the acceptable
            // backstop, and the inverse — charge-without-delivery — is exactly
            // what this fix eliminates. `escrow` already settled its deposit
            // pre-call (the deposit must commit before serving), so this is a
            // no-op for escrow.
            // When the post-delivery `exact` settle fails, the payment was NOT
            // collected on-chain, so the budget reservation committed pre-settlement
            // must be reconciled HERE — on every failure branch — rather than via the
            // downstream `log_spend` delta. The downstream reconciliation does not
            // cover this case: on the STREAMING path `usage`/`cost_outcome` are both
            // `None`, so neither `log_spend` branch fires and the reservation would
            // stay permanently committed (budget over-counted); on the non-streaming
            // path `log_spend` WOULD fire and record `billed` spend for a charge that
            // never landed. Either way is wrong. We release the reservation (the wallet
            // was not charged → its budget returns) and set this flag so the downstream
            // logging is skipped. `escrow` never reaches here (`payload_for_settle` is
            // `None`; it settled pre-call), so this is exact-only.
            let mut settle_after_deliver_failed = false;
            if let Some(ref payload) = payload_for_settle {
                match state.facilitator.settle(payload).await {
                    Ok(settlement) if settlement.success => {
                        counter!("solvela_payments_total", "status" => "settled").increment(1);
                        histogram!("solvela_payment_amount_usdc")
                            .record(client_amount as f64 / 1_000_000.0);
                        info!(
                            tx_signature = ?settlement.tx_signature,
                            "exact payment settled on-chain after provider delivery"
                        );
                    }
                    Ok(settlement) => {
                        // Provider already delivered; do not fail the request.
                        // Surface for operator follow-up — the customer received a
                        // completion the gateway could not collect payment for.
                        settle_after_deliver_failed = true;
                        counter!("solvela_payments_total", "status" => "settle_after_deliver_failed")
                            .increment(1);
                        // Reconcile the reservation: payment uncollected → release it
                        // so the wallet's budget is not consumed (covers streaming,
                        // where no downstream `log_spend` branch would otherwise fire).
                        state.usage.release_reservation(&budget_reservation).await;
                        warn!(
                            error = ?settlement.error,
                            failure_kind = ?settlement.failure_kind,
                            stream = req.stream,
                            reservation_released = true,
                            spend_recorded = false,
                            "exact settlement failed AFTER provider delivery — completion delivered, payment uncollected; reservation released (no spend recorded)"
                        );
                    }
                    Err(e) => {
                        settle_after_deliver_failed = true;
                        counter!("solvela_payments_total", "status" => "settle_after_deliver_failed")
                            .increment(1);
                        // Same reconciliation as the non-success branch above.
                        state.usage.release_reservation(&budget_reservation).await;
                        warn!(
                            error = %e,
                            stream = req.stream,
                            reservation_released = true,
                            spend_recorded = false,
                            "exact settlement errored AFTER provider delivery — completion delivered, payment uncollected; reservation released (no spend recorded)"
                        );
                    }
                }
            }
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

            // merged_005: the semantic discount is only *realised* on the escrow
            // scheme — the gateway claims the reduced amount and the remainder
            // refunds to the agent. On the direct-transfer "exact" scheme the
            // agent already settled the FULL amount on-chain before the cache was
            // consulted, with no refund path, so the discount must NOT touch
            // either the claim or the spend ledger there. Gate it by scheme once,
            // here, and feed `realized_discount` to both the claim and the log.
            let realized_discount = scheme_realized_discount(payment_scheme, cost_outcome);

            // Compute escrow claim amount: prefer actual usage, fall back to estimate
            // E2 FIX: Use minimum 1 atomic unit for streaming when estimation fails
            let claim_atomic = if let Some(outcome) = realized_discount {
                // Escrow semantic-cache hit: claim only the discounted price; the
                // remainder refunds to the agent. This is how the Phase 1 cache
                // discount is realised on-chain.
                Some(outcome.billable_atomic())
            } else if let Some(ref u) = usage {
                // `compute_actual_atomic_cost` returns `None` when the model
                // registry pricing is non-finite/negative or the result would
                // overflow u64. In those cases we skip the claim rather than
                // firing for a garbage amount; the warn! inside the function
                // surfaces the corrupt registry entry to operators.
                compute_actual_atomic_cost(u.prompt_tokens, u.completion_tokens, model_info)
            } else {
                Some(
                    estimated_atomic_cost(&state.model_registry, &req.model, &req, model_info)
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
                    payment_scheme,
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

            // Log spend with actual usage (non-streaming) or estimated (streaming).
            //
            // Two cases must both reconcile the `estimated_cost` reservation that
            // `check_budget` committed to the Redis counters (the H1 fix):
            //   (a) usage present — price from the actual (capped) tokens.
            //   (b) usage ABSENT but a semantic hit occurred (`cost_outcome` is
            //       Some) — the cached response carried no `usage`. We MUST still
            //       log, or the reservation is never settled down to the realised
            //       price and the wallet's budget stays consumed at the full
            //       reservation forever (merged_005 part 2 — ~70% over-consume).
            //
            // SKIP entirely when the post-delivery `exact` settle failed: the
            // reservation was already reconciled (released) at the settle-failure
            // branch above and the payment was never collected, so we must NOT also
            // record spend here (which would over-count the wallet's budget for an
            // uncollected charge) or release a second time.
            if settle_after_deliver_failed {
                // No-op: reservation already released, no spend to record.
            } else if let Some(ref u) = usage {
                match state
                    .model_registry
                    .estimate_cost(&req.model, u.prompt_tokens, u.completion_tokens)
                    .and_then(|c| {
                        c.total.parse::<f64>().map_err(|e| {
                            solvela_router::models::ModelRegistryError::ParseError(e.to_string())
                        })
                    }) {
                    Ok(cost) => {
                        // On an ESCROW semantic-cache hit the agent is billed the
                        // discounted price, so the spend ledger must record that —
                        // not the full computed `cost`. On the exact scheme
                        // `realized_discount` is None, so the FULL cost is logged
                        // (the agent paid it on-chain with no refund). The counter
                        // delta `(billed − reserved)` then settles the wallet's
                        // budget to the right amount.
                        //
                        // Bill in atomic units end-to-end: `cost` is the f64
                        // estimate from the registry, which we convert through
                        // `usdc_f64_to_atomic_safe` (NaN/∞/negative/overflow
                        // fail-closed → `None`). Both branches of
                        // `spend_cost_atomic` then operate in `u64`, so the
                        // ledger value is path-invariant. The legacy
                        // `SpendLogEntry.cost_usdc: f64` shape gets the single
                        // `/1_000_000.0` conversion right at the write site.
                        let Some(cost_atomic) = usdc_f64_to_atomic_safe(cost) else {
                            warn!(
                                model = %req.model,
                                wallet = %wallet_address,
                                raw_cost = cost,
                                "skipping spend log: computed cost is NaN/∞/negative/overflow — refusing to write a corrupt ledger entry"
                            );
                            return Ok(response);
                        };
                        let billed_atomic = spend_cost_atomic(realized_discount, cost_atomic);
                        let billed_cost = billed_atomic as f64 / 1_000_000.0;
                        // Pass the estimated_cost that was committed to Redis
                        // counters in `check_budget` so log_spend can adjust
                        // by the (billed − estimated) delta. Without this,
                        // counters would be double-incremented.
                        state.usage.log_spend(SpendLogEntry {
                            wallet_address: wallet_address.clone(),
                            model: req.model.clone(),
                            provider: actual_provider.unwrap_or_else(|| provider_name.to_string()),
                            input_tokens: u.prompt_tokens,
                            output_tokens: u.completion_tokens,
                            cost_usdc: billed_cost,
                            tx_signature: tx_signature.clone(),
                            request_id: request_id.clone(),
                            session_id: session_id.clone(),
                            tenant: tenant.clone(),
                            // Reconcile per-tenant counters only when
                            // check_budget actually enforced a provisioned bucket
                            // (decision == Enforce). Threaded from the reservation
                            // so the Skip path never accumulates per-tenant spend.
                            tenant_enforced: budget_reservation.tenant_enforced(),
                            estimated_cost_usdc: Some(estimated_cost),
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
            } else if cost_outcome.is_some() {
                // Usage-less semantic hit: reconcile the reservation. On escrow,
                // bill the discount; on exact, `realized_discount` is None so we
                // bill the full reservation (`estimated_cost`) — a zero delta that
                // correctly leaves the on-chain-settled full amount in the ledger.
                // We have no token counts (the cached response omitted usage), so
                // record the input estimate and zero output.
                //
                // Same atomic-domain billing as the usage-present arm. The pre-
                // provider validator at line 561 already gated `estimated_cost`
                // as finite + non-negative, but we still route through
                // `usdc_f64_to_atomic_safe` so a corrupted re-derivation (or a
                // future refactor that drops the early guard) cannot write a
                // NaN/∞ entry to the spend ledger.
                let Some(estimated_atomic) = usdc_f64_to_atomic_safe(estimated_cost) else {
                    warn!(
                        model = %req.model,
                        wallet = %wallet_address,
                        raw_estimated = estimated_cost,
                        "skipping spend log: estimated_cost is NaN/∞/negative/overflow on usage-less semantic-hit fallback"
                    );
                    return Ok(response);
                };
                let billed_atomic = spend_cost_atomic(realized_discount, estimated_atomic);
                let billed_cost = billed_atomic as f64 / 1_000_000.0;
                state.usage.log_spend(SpendLogEntry {
                    wallet_address: wallet_address.clone(),
                    model: req.model.clone(),
                    provider: actual_provider.unwrap_or_else(|| provider_name.to_string()),
                    input_tokens: estimate_input_tokens(&req),
                    output_tokens: 0,
                    cost_usdc: billed_cost,
                    tx_signature: tx_signature.clone(),
                    request_id: request_id.clone(),
                    session_id: session_id.clone(),
                    tenant: tenant.clone(),
                    // Same gating as the usage-present arm: reconcile per-tenant
                    // counters only when a provisioned bucket was enforced.
                    tenant_enforced: budget_reservation.tenant_enforced(),
                    estimated_cost_usdc: Some(estimated_cost),
                });
            }

            Ok(response)
        }
        Err(ProviderCallError::AllProvidersFailed {
            model, provider, ..
        }) => {
            // #486: a paid request that no provider could fulfil. Because of the
            // settlement reorder above, the customer is NOT charged for this
            // undelivered completion:
            //   - `exact`: the transfer was DEFERRED (never broadcast), so
            //     nothing settled on-chain. Release the budget reservation.
            //   - `escrow`: the deposit settled (it had to, pre-call), but we do
            //     NOT fire a claim here (the claim only runs in the success arm),
            //     so the deposit refunds at expiry. Also release the reservation.
            // Either way we return a RETRYABLE 503, not a bare 500.
            warn!(
                provider = %provider,
                model = %model,
                wallet = %wallet_address,
                scheme = ?payment_scheme,
                "paid request failed: no provider available — no charge taken (exact deferred / escrow unclaimed)"
            );
            counter!("solvela_paid_stub_rejections_total").increment(1);

            // No settlement (exact) or no claim (escrow) happened, so the
            // reserved budget must be returned — otherwise the wallet's spend
            // counter stays consumed for a request that cost it nothing.
            state.usage.release_reservation(&budget_reservation).await;

            // Client-facing message must not leak provider/RPC internals OR the
            // internal model ID (GHSA-cgqx-mg48-949v; `UpstreamUnavailable`'s
            // variant contract requires the inner string stay free of internal
            // detail). The model ID is already in the `warn!` above; the client
            // gets a static, internals-free message per scheme.
            let message = match payment_scheme {
                PaymentScheme::Exact => {
                    "No provider could serve your request right now and your payment was \
                     NOT charged. Please retry shortly."
                }
                PaymentScheme::Escrow => {
                    // An escrow deposit DID settle on-chain (pre-call) but no claim
                    // fires here, so the deposit stays locked until it refunds at
                    // expiry. Emit a scheme-specific metric so operators can alert on
                    // "N escrow deposits currently locked awaiting expiry" from
                    // metrics, not just by scraping logs.
                    counter!(
                        "solvela_payments_total",
                        "status" => "escrow_unclaimed_provider_failure"
                    )
                    .increment(1);
                    "No provider could serve your request right now; no claim was made against \
                     your escrow deposit and it refunds at expiry. Please retry shortly."
                }
            };
            Err(GatewayError::UpstreamUnavailable(message.to_string()))
        }
        Err(ProviderCallError::Internal(msg)) => {
            // #486: same accounting contract as the `AllProvidersFailed` arm — the
            // provider never delivered, so no `exact` transfer was broadcast and no
            // `escrow` claim was fired. The budget reservation committed pre-settlement
            // must be returned, or the wallet's spend counter stays consumed for a
            // request that cost it nothing. (The previous code returned here WITHOUT
            // releasing — the leak both money-path reviewers flagged.)
            warn!(
                wallet = %wallet_address,
                scheme = ?payment_scheme,
                "paid request failed: internal provider-call error — no charge taken; releasing budget reservation"
            );
            state.usage.release_reservation(&budget_reservation).await;
            Err(GatewayError::Internal(msg))
        }
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
        let name = HeaderName::from_static("x-solvela-fallback");
        assert_eq!(name.as_str(), "x-solvela-fallback");
        let legacy = HeaderName::from_static("x-rcr-fallback");
        assert_eq!(legacy.as_str(), "x-rcr-fallback");
    }
}
