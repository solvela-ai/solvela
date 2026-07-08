//! POST /v1/chat/completions — OpenAI-compatible chat endpoint.
//!
//! Submodules:
//! - [`channel_draw`] — v0 spend-down channel voucher draw fork (PR-B)
//! - [`cost`] — USDC computation and token estimation
//! - [`payment`] — Payment extraction, validation, escrow claims
//! - [`provider`] — Shared provider call pipeline (cache, fallback, SSE)
//! - [`response`] — Debug headers, session tokens, response construction

mod channel_draw;
pub(crate) mod cost;
pub(crate) mod payment;
mod provider;
mod response;

use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use axum::response::{IntoResponse, Response};
use metrics::{counter, histogram};
use tracing::{debug, info, warn};

use solvela_protocol::{ChatRequest, MessageContent, Role, SettlementFailureKind};
use solvela_router::profiles::{self, Profile};
use solvela_router::scorer;

use crate::error::GatewayError;
use crate::middleware::prompt_guard::{self, GuardResult, PromptGuardConfig};
use crate::receipts;
use crate::routes::debug_headers::{is_debug_enabled, PaymentStatus};
use crate::usage::SpendLogEntry;
use crate::AppState;

use cost::{
    cap_usage_to_request_limits, completion_token_ceiling, compute_actual_atomic_cost,
    discovery_floor_atomic, estimate_input_tokens, estimate_native_anthropic_input_tokens,
    estimated_atomic_cost, is_free_estimate, scheme_realized_discount, select_spend_log_arm,
    spend_cost_atomic, usdc_atomic_amount_checked, usdc_f64_to_atomic_safe, PaymentScheme,
    SpendLogArm,
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

/// The inbound wire dialect a request arrived on. Threaded into
/// [`chat_completions_inner`] alongside `resource_url` so the SAME money-path
/// core serves both the OpenAI route and the Anthropic Messages adapter, while
/// the latter can take the NATIVE passthrough when the resolved model is an
/// Anthropic model.
///
/// CLAUDE.md Rule #4 carve-out: the gateway speaks OpenAI EXCEPT this native
/// `/v1/messages` relay. `OpenAi` is the only dialect `/v1/chat/completions`
/// ever passes, so that endpoint's behavior is byte-for-byte unchanged. For an
/// `AnthropicMessages` request the fork is decided ONLY after model resolution:
/// an Anthropic-resolved model relays natively; a non-Anthropic-resolved model
/// (reachable only via an eco/auto routing alias) falls through to the existing
/// reshape branch — never a silent default-route either way.
pub(crate) enum WireDialect {
    /// `/v1/chat/completions` — always OpenAI-shaped in and out. Carries the
    /// ORIGINAL raw request bytes (PR-B): a channel voucher's `request_digest`
    /// binds to the EXACT bytes the client sent — never a re-serialization,
    /// which could differ byte-wise and fail-close every draw.
    OpenAi { original_body: axum::body::Bytes },
    /// `/v1/messages` — the inbound request is Anthropic Messages JSON. Carries
    /// the ORIGINAL validated request bytes (relayed VERBATIM on the native
    /// path; also the channel-voucher digest source) plus the inbound
    /// version/beta headers (forwarded verbatim upstream; the inbound Solvela
    /// bearer is NEVER forwarded).
    AnthropicMessages {
        original_body: axum::body::Bytes,
        anthropic_version: Option<String>,
        anthropic_beta: Option<String>,
    },
}

impl WireDialect {
    /// The native-passthrough source bytes + forwarded headers, or `None` for
    /// the OpenAI dialect. Returns `Some` regardless of the resolved model — the
    /// caller additionally gates on `model_info.provider == "anthropic"`.
    fn anthropic_native_source(&self) -> Option<(&axum::body::Bytes, Option<&str>, Option<&str>)> {
        match self {
            WireDialect::OpenAi { .. } => None,
            WireDialect::AnthropicMessages {
                original_body,
                anthropic_version,
                anthropic_beta,
            } => Some((
                original_body,
                anthropic_version.as_deref(),
                anthropic_beta.as_deref(),
            )),
        }
    }

    /// The RAW inbound request bytes — the channel-voucher `request_digest`
    /// preimage (SHA-256 of exactly what the client sent, both dialects).
    fn raw_body(&self) -> &axum::body::Bytes {
        match self {
            WireDialect::OpenAi { original_body }
            | WireDialect::AnthropicMessages { original_body, .. } => original_body,
        }
    }
}

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
    // Take the RAW body (not `Json<ChatRequest>`) so we can intercept a
    // malformed/empty body BEFORE Axum's extractor rejects it with a bare
    // 400/422. An x402 registry health-checker probes an UNPAID request with an
    // empty/minimal body to confirm the resource speaks x402; it must see the
    // discovery 402, not a parse error. `Bytes` is the LAST argument because it
    // consumes the request body. The 10MB `RequestBodyLimitLayer` (lib.rs)
    // still bounds it.
    body: axum::body::Bytes,
) -> Result<Response, GatewayError> {
    // Is a payment credential present? This single check decides how a
    // bad/empty body is handled (CLAUDE.md rule #8: the route, not middleware,
    // emits the 402). A `PAYMENT-SIGNATURE` header means a PAYING client, which
    // must get real validation errors on a bad body. NO header means an UNPAID
    // request, which is eligible for the discovery 402 on a bad body.
    let has_payment_header = headers
        .get("payment-signature")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| !v.is_empty());

    // Deserialize the body into a `ChatRequest`.
    //
    //   - Payment header PRESENT → strict parse; on failure return today's 4xx
    //     (a paying client still gets a real validation error — UNCHANGED).
    //   - Payment header ABSENT → on parse failure (empty/garbage/missing
    //     fields) return the DISCOVERY 402, never a 400/422. This is the only
    //     behavior change, and it is confined to the UNPAID path. On parse
    //     SUCCESS the request flows through the existing logic, which returns
    //     the EXACT per-request quote 402 for a paid model (UNCHANGED).
    //
    // The discovery path returns here BEFORE any guard, model resolution,
    // payment verification, settlement, provider call, budget mutation, or
    // spend logging — it is read-only.
    let req: ChatRequest = match serde_json::from_slice::<ChatRequest>(&body) {
        Ok(req) => req,
        Err(e) => {
            if has_payment_header {
                // Paying client, bad body → real validation error (as today).
                return Err(GatewayError::BadRequest(format!(
                    "invalid request body: {e}"
                )));
            }
            // Unpaid probe with a bad/empty body → advertise the resource.
            return Err(chat_completions_discovery(&state, "/v1/chat/completions"));
        }
    };

    // Delegate to the shared inner core (the single home of the money path).
    // `/v1/messages` (the Anthropic Messages adapter) calls the SAME core with
    // its own `resource_url`, so the 402/cost/verify/settle/record logic is
    // never forked — CLAUDE.md requires the payment path to live in one place.
    // `/v1/chat/completions` always passes the `OpenAi` dialect, so this
    // endpoint never takes the native Anthropic passthrough and its behavior is
    // byte-for-byte unchanged.
    chat_completions_inner(
        state,
        peer_addr,
        headers,
        req,
        "/v1/chat/completions",
        // The raw bytes ride the dialect so the channel draw fork can digest
        // EXACTLY what the client sent (`Bytes` clone is a refcount bump).
        WireDialect::OpenAi {
            original_body: body,
        },
    )
    .await
}

/// Shared inner core for every OpenAI-shaped chat request, regardless of the
/// inbound wire dialect.
///
/// Runs the full money path: content validation → model resolution → prompt
/// guard → cost estimate → free-tier bypass → 402 → payment decode/verify/
/// settle → provider call → escrow claim / spend log / receipt → response.
///
/// `resource_url` is the endpoint the client posted to (`/v1/chat/completions`
/// for the OpenAI route, `/v1/messages` for the Anthropic Messages adapter). It
/// is used for BOTH (a) the `resource.url` the signed payment payload must match
/// and (b) the `resource.url` advertised in the 402 challenge, so a payment is
/// always bound to the exact endpoint it was signed for. Extracting this core
/// (rather than duplicating it) keeps a single home for the payment logic; the
/// `/v1/chat/completions` behavior is byte-for-byte unchanged because it passes
/// the same `"/v1/chat/completions"` it always hard-coded.
pub(crate) async fn chat_completions_inner(
    state: Arc<AppState>,
    peer_addr: crate::middleware::rate_limit::PeerAddr,
    headers: HeaderMap,
    mut req: ChatRequest,
    resource_url: &str,
    dialect: WireDialect,
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
    // Ledger encoding of the router's output. The debug headers below keep the
    // raw "N/A"/0.0 sentinel; every spend row instead stores NULL/NULL when the
    // router never ran, so 'N/A' can't become a pseudo-tier and a non-routed
    // request can't be mistaken for a genuine 0.0 score.
    let (log_routing_tier, log_routing_score) = routing_telemetry(&routing_tier, routing_score);

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

    // Native Anthropic passthrough fork (CLAUDE.md Rule #4 carve-out). Decided
    // ONLY after model resolution: an `/v1/messages` request whose resolved
    // model is an Anthropic-provider model takes the native relay; an
    // Anthropic-shaped request that resolves to a NON-Anthropic provider (only
    // reachable via an eco/auto routing alias) falls through to the existing
    // reshape/OpenAI path. `/v1/chat/completions` always passes `OpenAi`, so it
    // is never native. The `original_body` + forwarded headers are captured here
    // for the relay (and to compute the native input-token estimate below). The
    // relay handle being absent (no `ANTHROPIC_API_KEY`) is handled at the call
    // site — it fails closed into the all-providers-failed arm, never silently
    // reshaping or serving free.
    let native_source = if model_info.provider == "anthropic" {
        dialect.anthropic_native_source()
    } else {
        None
    };

    // The upfront input-token estimate that feeds BOTH the 402 quote AND the C1
    // client-amount validation / M3 budget reservation. Computed ONCE here so
    // the two sites can never diverge. On the native path it is computed
    // DIRECTLY from the original Anthropic body (counting system + every
    // message's text + serialized tools + thinking history), so the quote can
    // never UNDER-reserve relative to the native bill (#500 class); on every
    // other path it is the existing OpenAI-shaped `estimate_input_tokens`.
    let input_token_estimate: u32 = match native_source {
        Some((body, _, _)) => estimate_native_anthropic_input_tokens(body),
        None => estimate_input_tokens(&req),
    };

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

        // Dev-bypass must honor the SAME native-vs-reshape fork as the paid path
        // (the dispatch at "Step 5" below). An Anthropic-resolved `/v1/messages`
        // request (`native_source` is `Some`) takes the byte-verbatim native
        // relay so the extended-thinking `signature` survives; without this, the
        // dev-bypass branch would reshape through the OpenAI pipeline (losing the
        // signature → Claude Code hard-400s the next multi-turn request) and
        // serve a `chat.completion` shape for an endpoint that promises native
        // Anthropic bytes. Mirrors the verified-path dispatch verbatim except for
        // the (here irrelevant) settlement that only the paid path performs.
        let dev_bypass_call = if let Some((body, version, beta)) = native_source {
            run_native_relay(&state, &req, model_info, body, version, beta, &session_id).await
        } else {
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
            provider::execute_provider_call(&ctx).await
        };

        return dev_bypass_call.map(|r| r.response).map_err(|e| match e {
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
            // `input_token_estimate` is the SINGLE estimate source (native
            // Anthropic body estimate on the native path, OpenAI-shaped estimate
            // otherwise), so the 402 quote and the C1 validation below price from
            // the same figure.
            input_token_estimate,
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

        // Honor the SAME native-vs-reshape fork as the dev-bypass and paid paths.
        // No Anthropic model is currently priced at $0 (so this branch is not
        // reachable for one today), but keeping the dispatch consistent across ALL
        // three provider-call sites is exactly what prevents the class of bug where
        // one site silently reshapes an Anthropic-resolved `/v1/messages` request
        // (losing the thinking `signature`) while the others relay natively. A
        // future free-tier Anthropic model can never regress to the OpenAI shape.
        let free_call = if let Some((body, version, beta)) = native_source {
            run_native_relay(&state, &req, model_info, body, version, beta, &session_id).await
        } else {
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
            provider::execute_provider_call(&ctx).await
        };

        return match free_call {
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
                    vendor: None,
                    routing_tier: log_routing_tier.clone(),
                    routing_score: log_routing_score,
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

        // Reuse the single 402 challenge builder so the per-request quote and
        // the discovery challenge are byte-shape-identical (same accepts /
        // legacy body / canonical PAYMENT-REQUIRED header). The CONFIGURED mint
        // and recipient are baked in there — never the compile-time constant.
        // The challenge advertises THIS endpoint's `resource_url` so the client
        // signs a payment bound to the endpoint it called.
        let payment_required = build_payment_challenge(&state, atomic_amount, cost, resource_url);

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
    let payment_payload = match decode_payment_from_header(payment_header.unwrap()) {
        Ok(payload) => Some(payload),
        Err(reason) => {
            // Canonical-surface rejections (unsupported scheme / proof /
            // version, missing accepted/resource) and garbled headers are
            // distinct, money-relevant failures — log the SPECIFIC reason
            // server-side so they are never silently conflated. The client
            // receives only the fixed string in the `None` arm below
            // (GHSA-cgqx-mg48-949v: never reflect the parse error; the only
            // attacker-controlled bytes in `reason` are the 32-char-capped
            // scheme echo from `CanonicalPaymentError::UnsupportedScheme`).
            warn!(error = %reason, "PAYMENT-SIGNATURE header decode failed");
            None
        }
    };

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
    // P2 receipts: the estimate's full CostBreakdown (the C1 `expected_cost`
    // the 402 quote, payment validation, and budget reservation all derive
    // from). The streaming / usage-less receipt arms record THIS breakdown —
    // the same figure the spend ledger bills on those paths.
    let estimated_cost_breakdown: solvela_protocol::CostBreakdown;
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

            // Verify the resource URL matches this endpoint. `resource_url` is
            // the endpoint the client actually posted to (`/v1/chat/completions`
            // or `/v1/messages`), so a payment signed for one endpoint can never
            // be replayed against the other.
            if payload.resource.url != resource_url {
                // `resource.url` is client-controlled and unbounded (up to the
                // 50KB header guard), so log a truncated copy server-side —
                // mirrors the proxy-route truncation (chars-based so a
                // multibyte boundary can never panic).
                let resource_url: String = payload.resource.url.chars().take(256).collect();
                warn!(
                    resource_url = %resource_url,
                    "payment resource URL mismatch"
                );
                return Err(GatewayError::InvalidPayment(
                    "Payment resource does not match this endpoint.".to_string(),
                ));
            }

            // Verify the resource method is POST
            if !payload.resource.method.eq_ignore_ascii_case("POST") {
                // `resource.method` is client-controlled — truncate defensively
                // before logging (same posture as `resource.url` above).
                let method: String = payload.resource.method.chars().take(16).collect();
                warn!(
                    method = %method,
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

            // Verify asset is the CONFIGURED USDC-SPL mint — the same one the
            // verifier enforces — not the compile-time constant.
            if payload.accepted.asset != state.config.solana.usdc_mint {
                // GHSA-cgqx-mg48-949v posture: `asset` is client-controlled —
                // cap to the max base58 pubkey length (44 chars) before
                // logging (mirrors the tx_prefix truncation in a2a/handler.rs;
                // chars-based so a multibyte boundary can never panic).
                let asset_prefix: String = payload.accepted.asset.chars().take(44).collect();
                warn!(
                    asset = %asset_prefix,
                    expected = %state.config.solana.usdc_mint,
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

            // --- v0 spend-down channel DRAW fork (PR-B) ----------------------
            //
            // Inserted after the resource/method/network/asset/pay_to
            // validation above (which applies to a channel voucher too) and
            // BEFORE the exact/escrow-only machinery below (C1 client-amount
            // floor, M6 variant check, tx-replay cache, budget reservation,
            // verify/settle) — mirroring the shipped `/v1/search` fork. It
            // forks iff the scheme is "channel" AND the payload is a channel
            // voucher; ANY channel-ish mismatch is a fail-closed reject — no
            // silent fallback to an exact transfer (solvela-x402 §4). The fork
            // never touches `verify_and_settle`, the tx-replay cache,
            // `check_budget` (Decision E — the deposit IS the budget), or
            // `fire_escrow_claim` (plan HALT 3/5). Model resolution + prompt
            // guard already ran above (fail before locking — the #574 order).
            match (payload.accepted.scheme.as_str(), &payload.payload) {
                ("channel", solvela_x402::types::PayloadData::Channel(voucher_payload)) => {
                    // `billed` = the single fee-inclusive estimate computed
                    // once above (the same figure quoted as the 402 `exact`
                    // entry's amount — the sidecar's quote source, §4). The
                    // checked-string parse cannot fail after
                    // `usdc_atomic_amount_checked` succeeded; fail closed
                    // anyway (never a silent 0-quote draw).
                    let billed_atomic: u64 = atomic_amount.parse().map_err(|_| {
                        GatewayError::Internal(
                            "failed to parse the channel draw quote as u64".to_string(),
                        )
                    })?;
                    return channel_draw::channel_draw(channel_draw::ChannelDrawContext {
                        state: &state,
                        headers: &headers,
                        req: &req,
                        model_info,
                        dialect: &dialect,
                        billed_atomic,
                        accepted_amount: &payload.accepted.amount,
                        voucher_payload,
                        session_id: &session_id,
                        request_id: &request_id,
                        tenant: &tenant,
                        debug_enabled,
                        request_start,
                        routing_tier: &routing_tier,
                        routing_score,
                        routing_profile: &routing_profile,
                    })
                    .await;
                }
                ("channel", _) => {
                    return Err(GatewayError::InvalidPayment(
                        "scheme is 'channel' but the payload is not a channel voucher".to_string(),
                    ));
                }
                (_, solvela_x402::types::PayloadData::Channel(_)) => {
                    return Err(GatewayError::InvalidPayment(
                        "payment payload is a channel voucher but the scheme is not 'channel'"
                            .to_string(),
                    ));
                }
                // Not a channel request — fall through to the exact/escrow
                // machinery below.
                _ => {}
            }

            // --- C1: Recompute expected cost and validate client amount ---
            let expected_cost = state
                .model_registry
                .estimate_cost(
                    &req.model,
                    // SAME single estimate source as the 402 quote above (native
                    // body estimate on the native path), so the client-amount
                    // validation can never disagree with the advertised quote.
                    input_token_estimate,
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
                // Fail-closed defense-in-depth (PR-B): every channel voucher
                // forks out (or is rejected) at the channel fork above, so
                // this arm is unreachable — but a voucher must NEVER enter the
                // tx-replay cache (it has no on-chain tx; plan HALT 5).
                // Reject, never panic.
                solvela_x402::types::PayloadData::Channel(_) => {
                    return Err(GatewayError::InvalidPayment(
                        "channel voucher is not accepted on the exact/escrow path".to_string(),
                    ));
                }
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
            estimated_cost_breakdown = expected_cost;

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
                // Fail-closed defense-in-depth: the channel fork above returns
                // (or rejects) every request whose scheme string is "channel"
                // BEFORE `from_accepted_str` runs, so this arm is unreachable
                // today — but a channel draw must NEVER flow into the
                // exact/escrow settlement machinery (plan HALT 3/5), so if a
                // future reorder ever exposes it, reject rather than settle.
                // The budget reservation committed above is released (nothing
                // was settled).
                PaymentScheme::Channel => {
                    warn!(
                        "channel-scheme payment reached the exact/escrow settlement match — \
                         rejecting fail-closed (the draw fork should have handled it)"
                    );
                    state.usage.release_reservation(&budget_reservation).await;
                    return Err(GatewayError::InvalidPayment(
                        "channel vouchers are not settled on this path".to_string(),
                    ));
                }
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

    // The provider call. On the NATIVE Anthropic passthrough this is the
    // byte-verbatim relay (no cache, no cross-provider fallback — it succeeds
    // against Anthropic or fails loudly into the all-providers-failed arm,
    // releasing the reservation and never settling). On every other path it is
    // the existing OpenAI-shaped pipeline (cache → semantic → fallback chain).
    // BOTH yield the SAME `Result<ProviderCallResult, ProviderCallError>` so the
    // downstream settle / claim / spend-log / receipt arms are shared verbatim
    // — the native path adds NO new financial math.
    let provider_call = if let Some((body, version, beta)) = native_source {
        run_native_relay(&state, &req, model_info, body, version, beta, &session_id).await
    } else {
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
        provider::execute_provider_call(&ctx).await
    };

    match provider_call {
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
            // Arm selection is the pure `select_spend_log_arm` (unit-tested
            // exhaustively in cost.rs); the match below is exhaustive so a new
            // arm is a compile error, never a silent fall-through. Three of the
            // four arms must reconcile the `estimated_cost` reservation that
            // `check_budget` committed to the Redis counters (the H1 fix):
            //   (a) `ActualUsage` — price from the actual (capped) tokens.
            //   (b) `UsagelessSemanticHit` — usage ABSENT but a semantic hit
            //       occurred (`cost_outcome` is Some): the cached response
            //       carried no `usage`. We MUST still log, or the reservation is
            //       never settled down to the realised price and the wallet's
            //       budget stays consumed at the full reservation forever
            //       (merged_005 part 2 — ~70% over-consume).
            //   (c) `EstimateFallback` — usage AND `cost_outcome` both absent:
            //       the delivered+settled STREAMING path. Bill the estimate —
            //       the amount reserved in `check_budget` and (for `exact`)
            //       settled on-chain. Before this arm existed such requests
            //       logged NOTHING: no spend_logs row, no tenant attribution,
            //       and a never-reconciled reservation.
            //
            // `SkipSettleFailed` (post-delivery `exact` settle failed) records
            // nothing: the reservation was already reconciled (released) at the
            // settle-failure branch above and the payment was never collected,
            // so recording spend here would over-count the wallet's budget for
            // an uncollected charge (or release a second time).
            match select_spend_log_arm(
                settle_after_deliver_failed,
                usage.as_ref(),
                cost_outcome.is_some(),
            ) {
                SpendLogArm::SkipSettleFailed => {
                    // No spend: reservation already released at the settle-failure
                    // branch above, nothing was collected.
                    //
                    // No receipt either: the payment was NOT collected on-chain
                    // (the post-delivery `exact` settle failed), so a receipt
                    // header would promise audit evidence of a payment that
                    // didn't happen. The settle-failure branch already counted
                    // `solvela_payments_total{status="settle_after_deliver_failed"}`;
                    // this counter completes the receipt-skip taxonomy.
                    counter!("solvela_receipt_skipped_total", "reason" => "settle_failed")
                        .increment(1);
                }
                SpendLogArm::ActualUsage(u) => {
                    match state
                        .model_registry
                        .estimate_cost(&req.model, u.prompt_tokens, u.completion_tokens)
                        .and_then(|c| {
                            // Keep the full breakdown alongside the parsed
                            // total: the P2 receipt records the same pricing
                            // computation the bill derives from.
                            c.total.parse::<f64>().map(|total| (c, total)).map_err(|e| {
                                solvela_router::models::ModelRegistryError::ParseError(
                                    e.to_string(),
                                )
                            })
                        }) {
                        Ok((actual_cost_breakdown, cost)) => {
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
                                counter!("solvela_spend_log_skipped_total", "reason" => "corrupt_actual_cost")
                                    .increment(1);
                                // Intentionally do NOT release the budget
                                // reservation here: the payment WAS collected
                                // (settled), so keeping the Redis reservation at
                                // the estimate is the correct, conservative
                                // accounting — releasing it would let the
                                // wallet's budget under-count collected spend.
                                // Only the DB ledger row is skipped.
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
                                provider: actual_provider
                                    .unwrap_or_else(|| provider_name.to_string()),
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
                                vendor: None,
                                routing_tier: log_routing_tier.clone(),
                                routing_score: log_routing_score,
                            });
                            // P2 receipt: same write point as the spend row —
                            // every paid completion that records spend records
                            // a receipt (and advertises it via the header).
                            emit_chat_receipt(
                                &state,
                                &mut response,
                                ChatReceiptInputs {
                                    model: &req.model,
                                    scheme: payment_scheme,
                                    tx_signature: &tx_signature,
                                    payer_wallet: &wallet_address,
                                    amount_paid_atomic: billed_atomic,
                                    breakdown: &actual_cost_breakdown,
                                },
                            );
                        }
                        Err(e) => {
                            warn!(
                                error = %e,
                                model = %req.model,
                                wallet = %wallet_address,
                                "failed to compute actual cost — skipping spend log to avoid $0 entry"
                            );
                            // P2 receipt: intentionally OMITTED so receipts stay
                            // lock-step with the spend ledger (no spend row → no
                            // receipt; a receipt here would attest to a bill the
                            // ledger never recorded). The exact payment DID
                            // settle, so this settled-but-unledgered arm is a
                            // known pre-existing gap (#541 family — the spend
                            // half is deliberately left unchanged here); the
                            // counter makes the receipt gap observable.
                            counter!("solvela_receipt_skipped_total", "reason" => "cost_estimation_error")
                                .increment(1);
                        }
                    }
                }
                SpendLogArm::UsagelessSemanticHit => {
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
                        counter!("solvela_spend_log_skipped_total", "reason" => "corrupt_estimate_semantic_hit")
                            .increment(1);
                        // Intentionally do NOT release the budget reservation
                        // here: the payment WAS collected (settled), so keeping
                        // the Redis reservation at the estimate is the correct,
                        // conservative accounting — releasing it would let the
                        // wallet's budget under-count collected spend. Only the
                        // DB ledger row is skipped.
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
                        vendor: None,
                        routing_tier: log_routing_tier.clone(),
                        routing_score: log_routing_score,
                    });
                    // P2 receipt: billed amount mirrors the ledger (discounted
                    // on an escrow semantic hit); the breakdown is the C1
                    // estimate the reservation/settlement derived from.
                    emit_chat_receipt(
                        &state,
                        &mut response,
                        ChatReceiptInputs {
                            model: &req.model,
                            scheme: payment_scheme,
                            tx_signature: &tx_signature,
                            payer_wallet: &wallet_address,
                            amount_paid_atomic: billed_atomic,
                            breakdown: &estimated_cost_breakdown,
                        },
                    );
                }
                SpendLogArm::EstimateFallback => {
                    // Delivered + settled, but usage AND cost_outcome are both
                    // absent — the streaming path (a streaming provider call
                    // carries no usage data and missed the semantic cache).
                    // Bill the ESTIMATE: it is the amount `check_budget`
                    // reserved and, on the exact scheme, precisely what the
                    // agent settled on-chain — so the ledger matches the money
                    // collected. `realized_discount` is structurally None here
                    // (no `cost_outcome`), so `spend_cost_atomic` is the
                    // identity and billed == estimate; we still route through
                    // it so all logging arms share one atomic-domain billing
                    // path. `estimated_cost_usdc: Some(estimated_cost)` makes
                    // `log_spend`'s `(billed − estimated)` reconciliation delta
                    // zero — the reservation correctly stays at the estimate.
                    //
                    // Same fail-closed conversion as the other arms: the pre-
                    // provider validator already gated `estimated_cost` as
                    // finite + non-negative, but we still route through
                    // `usdc_f64_to_atomic_safe` so a corrupted re-derivation
                    // (or a future refactor that drops the early guard) cannot
                    // write a NaN/∞ entry to the spend ledger.
                    let Some(estimated_atomic) = usdc_f64_to_atomic_safe(estimated_cost) else {
                        warn!(
                            model = %req.model,
                            wallet = %wallet_address,
                            raw_estimated = estimated_cost,
                            "skipping spend log: estimated_cost is NaN/∞/negative/overflow on streaming estimate fallback"
                        );
                        counter!("solvela_spend_log_skipped_total", "reason" => "corrupt_estimate_fallback")
                            .increment(1);
                        // Intentionally do NOT release the budget reservation
                        // here: the payment WAS collected (settled), so keeping
                        // the Redis reservation at the estimate is the correct,
                        // conservative accounting — releasing it would let the
                        // wallet's budget under-count collected spend. Only the
                        // DB ledger row is skipped.
                        return Ok(response);
                    };
                    let billed_atomic = spend_cost_atomic(realized_discount, estimated_atomic);
                    let billed_cost = billed_atomic as f64 / 1_000_000.0;
                    state.usage.log_spend(SpendLogEntry {
                        wallet_address: wallet_address.clone(),
                        model: req.model.clone(),
                        provider: actual_provider.unwrap_or_else(|| provider_name.to_string()),
                        // No token data on this path: record the request-side
                        // input estimate and zero output, consistent with the
                        // usage-less semantic-hit arm.
                        input_tokens: estimate_input_tokens(&req),
                        output_tokens: 0,
                        cost_usdc: billed_cost,
                        tx_signature: tx_signature.clone(),
                        request_id: request_id.clone(),
                        session_id: session_id.clone(),
                        tenant: tenant.clone(),
                        // Same gating as the other arms: reconcile per-tenant
                        // counters only when a provisioned bucket was enforced.
                        tenant_enforced: budget_reservation.tenant_enforced(),
                        estimated_cost_usdc: Some(estimated_cost),
                        vendor: None,
                        routing_tier: log_routing_tier.clone(),
                        routing_score: log_routing_score,
                    });
                    // P2 receipt — the STREAMING arm (the #541 bug class):
                    // every settled streaming completion records a receipt at
                    // the billed estimate. `response` is the constructed (not
                    // yet transmitted) SSE response, so the receipt header is
                    // decided BEFORE the body starts.
                    emit_chat_receipt(
                        &state,
                        &mut response,
                        ChatReceiptInputs {
                            model: &req.model,
                            scheme: payment_scheme,
                            tx_signature: &tx_signature,
                            payer_wallet: &wallet_address,
                            amount_paid_atomic: billed_atomic,
                            breakdown: &estimated_cost_breakdown,
                        },
                    );
                }
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
                // Unreachable today: a channel draw forks out before the
                // provider dispatch above and handles its own provider-failure
                // arm (record-nothing non-charge). Kept exhaustive + truthful
                // (a channel draw that failed here took no charge either).
                PaymentScheme::Channel => {
                    "No provider could serve your request right now and your channel was \
                     NOT drawn. Please retry shortly."
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

/// Run the NATIVE Anthropic `/v1/messages` passthrough as a drop-in for
/// [`provider::execute_provider_call`].
///
/// Returns the SAME [`ProviderCallResult`] / [`ProviderCallError`] shape so the
/// caller's settle / claim / spend-log / receipt arms are reused verbatim — the
/// native path introduces NO new financial math:
/// - On success: `response` is the upstream Anthropic response bytes relayed
///   UNTOUCHED (200, `application/json`), and `usage` is derived from the parsed
///   [`AnthropicUsage`] via the shared [`AnthropicUsage::to_billed_usage`] fold
///   (#614–616) — identical to the reshape path. `actual_provider` is
///   `"anthropic"`; `cost_outcome` is `None` (no semantic-cache discount on the
///   relay — caching is bypassed entirely).
/// - On ANY failure (no relay handle configured, transport error, upstream
///   non-2xx, or unbillable body): returns
///   [`ProviderCallError::AllProvidersFailed`], so the caller RELEASES the
///   budget reservation and NEVER settles (exact deferred / escrow unclaimed).
///   This is the fail-closed edge: the native relay does NOT fall back to the
///   cross-provider chain — it succeeds against Anthropic or fails loudly.
///
/// SECRET-REDACTION: the relay's [`NativeRelayError`] never carries the gateway
/// key, the raw upstream body, or a raw reqwest error; the `error` string put
/// into `AllProvidersFailed` here is a fixed category, and the caller's arm maps
/// it to a static, internals-free client message (GHSA-cgqx-mg48-949v).
async fn run_native_relay(
    state: &Arc<AppState>,
    req: &ChatRequest,
    model_info: &solvela_protocol::ModelRegistration,
    body: &axum::body::Bytes,
    anthropic_version: Option<&str>,
    anthropic_beta: Option<&str>,
    session_id: &Option<String>,
) -> Result<ProviderCallResult, ProviderCallError> {
    use crate::providers::anthropic::NativeRelayError;

    // No relay handle configured (no ANTHROPIC_API_KEY) → fail CLOSED. We do NOT
    // silently reshape or serve free: the Anthropic-resolved model cannot be
    // served natively, so this is the all-providers-failed condition (the caller
    // releases the reservation and never settles).
    let Some(relay) = state.native_anthropic.as_ref() else {
        warn!(
            model = %req.model,
            "native /v1/messages relay requested but no Anthropic relay handle is configured \
             (ANTHROPIC_API_KEY unset) — failing closed (no charge)"
        );
        return Err(ProviderCallError::AllProvidersFailed {
            model: req.model.clone(),
            provider: model_info.provider.clone(),
            error: "native Anthropic relay not configured".to_string(),
        });
    };

    // MODEL-ID REWRITE (inbound contract → upstream contract): the inbound body's
    // `model` is the gateway-facing id — either the bare Anthropic id Claude Code
    // sent, or (when a client used the canonical form, or the route canonicalized
    // a bare id for routing) `anthropic/<id>`. `api.anthropic.com` ONLY accepts
    // the bare id, so the relayed body's `model` MUST be the registry entry's bare
    // `model_id` (`model_info.model_id`, the internal-only upstream address).
    // Everything else in the body is forwarded UNCHANGED so thinking-block
    // `signature`s, `redacted_thinking`, native `tool_use` blocks, and tools all
    // survive. If the rewrite cannot be applied (body is not a JSON object, or
    // serialization fails), fail CLOSED via the all-providers-failed arm rather
    // than relaying a body that would 404 upstream or silently mis-bill.
    let relay_body = match rewrite_relayed_model(body, &model_info.model_id) {
        Ok(rewritten) => rewritten,
        Err(e) => {
            warn!(
                model = %req.model,
                error = %e,
                "failed to set the upstream model id on the native /v1/messages body \
                 — failing closed (no charge)"
            );
            return Err(ProviderCallError::AllProvidersFailed {
                model: req.model.clone(),
                provider: "anthropic".to_string(),
                error: "could not prepare native request for upstream".to_string(),
            });
        }
    };

    // STREAMING native passthrough: relay the upstream SSE bytes VERBATIM. This
    // is the streaming twin of the buffered path below. It returns
    // `usage: None` / `cost_outcome: None`, so the caller takes the
    // `EstimateFallback` settlement arm — IDENTICAL to the OpenAI streaming path:
    // bill the request-side estimate (the amount reserved in `check_budget` and,
    // on `exact`, settled on-chain), never `.await` settlement on the streamed
    // bytes, and read no per-token usage out of the stream. The native streaming
    // path adds NO new financial math. On any failure (transport / non-2xx /
    // unbillable) the relay returns a redacted `NativeRelayError` mapped to the
    // all-providers-failed arm below (reservation released, no settle, no body
    // leak) — never a silent reshape fallback.
    if req.stream {
        info!(model = %req.model, "native /v1/messages streaming passthrough to Anthropic");
        // No `solvela_provider_request_duration_seconds` here: `relay_native_stream`
        // returns as soon as upstream HEADERS arrive (the body streams lazily), so
        // recording it would measure TTFB under a metric name whose other (buffered)
        // users mean TOTAL duration — a mislabel. Streaming latency observability
        // can be added later under a properly-named TTFB/stream metric.
        let stream_result = relay
            .relay_native_stream(relay_body, anthropic_version, anthropic_beta)
            .await;

        let mut response = match stream_result {
            Ok(resp) => resp,
            Err(e) => {
                let error_type = match &e {
                    NativeRelayError::Transport => "timeout",
                    NativeRelayError::UpstreamStatus(_) => "server_error",
                    NativeRelayError::Unbillable => "unknown",
                };
                counter!(
                    "solvela_provider_errors_total",
                    "provider" => "anthropic".to_string(),
                    "error_type" => error_type
                )
                .increment(1);
                warn!(
                    model = %req.model,
                    error = %e,
                    "native /v1/messages streaming relay failed — failing closed (no charge)"
                );
                return Err(ProviderCallError::AllProvidersFailed {
                    model: req.model.clone(),
                    provider: "anthropic".to_string(),
                    // Fixed category only — never the upstream body / raw error.
                    error: e.to_string(),
                });
            }
        };
        response::attach_session_id(&mut response, session_id);
        return Ok(ProviderCallResult {
            response,
            // Streaming carries no response-side usage; the caller bills the
            // estimate via the EstimateFallback arm (same as OpenAI streaming).
            usage: None,
            actual_provider: Some("anthropic".to_string()),
            cost_outcome: None,
        });
    }

    info!(model = %req.model, "native /v1/messages passthrough to Anthropic");
    let provider_start = Instant::now();
    let relay_result = relay
        // Forward the body with ONLY the top-level `model` rewritten to the bare
        // upstream id; every other byte of meaning is preserved.
        .relay_native(relay_body, anthropic_version, anthropic_beta)
        .await;
    histogram!(
        "solvela_provider_request_duration_seconds",
        "provider" => "anthropic".to_string()
    )
    .record(provider_start.elapsed().as_secs_f64());

    let (raw, anthropic_usage) = match relay_result {
        Ok(pair) => pair,
        Err(e) => {
            // Map the relay error to a metrics bucket + the all-providers-failed
            // arm WITHOUT leaking internals. The numeric upstream status (if any)
            // is logged server-side; the client gets the static message the
            // caller's arm produces.
            let error_type = match &e {
                NativeRelayError::Transport => "timeout",
                NativeRelayError::UpstreamStatus(_) => "server_error",
                NativeRelayError::Unbillable => "unknown",
            };
            counter!(
                "solvela_provider_errors_total",
                "provider" => "anthropic".to_string(),
                "error_type" => error_type
            )
            .increment(1);
            warn!(
                model = %req.model,
                error = %e,
                "native /v1/messages relay failed — failing closed (no charge)"
            );
            return Err(ProviderCallError::AllProvidersFailed {
                model: req.model.clone(),
                provider: "anthropic".to_string(),
                // Fixed category only — never the upstream body / raw error.
                error: e.to_string(),
            });
        }
    };

    // Build the response from the RAW Anthropic bytes — relayed UNTOUCHED so the
    // thinking-block `signature`, `redacted_thinking`, native `tool_use` blocks,
    // and cache-token usage survive byte-for-byte to the client.
    let mut response = (
        [(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/json"),
        )],
        raw,
    )
        .into_response();
    response::attach_session_id(&mut response, session_id);

    // Bill via the EXISTING path: fold the cache-token usage with the shared
    // helper (so the relay and `from_anthropic_response` can never drift), then
    // the caller caps it (`cap_usage_to_request_limits`) and prices it
    // (`compute_actual_atomic_cost`) exactly as for any other usage.
    let usage = anthropic_usage.to_billed_usage();
    // OBSERVABILITY ONLY: emit the cross-provider cache-token counters, same as
    // the reshape path (the relay would otherwise be a metrics blind spot).
    anthropic_usage.emit_cache_metrics(&req.model);

    Ok(ProviderCallResult {
        response,
        usage: Some(usage),
        actual_provider: Some("anthropic".to_string()),
        cost_outcome: None,
    })
}

/// Rewrite ONLY the top-level `model` field of an inbound Anthropic Messages
/// request body to `upstream_model_id` (the bare Anthropic id), preserving every
/// other field.
///
/// `api.anthropic.com` accepts ONLY the bare model id (e.g. `claude-sonnet-4-6`),
/// never the gateway-facing `anthropic/<id>` form. The inbound body may carry
/// either form (a bare id from Claude Code, or the canonical form from an x402
/// client / after route canonicalization), so the relayed body must always be
/// normalized to the bare upstream id before forwarding.
///
/// Re-serializing through `serde_json::Value` may reorder keys, but it preserves
/// every value byte-exactly — including the cryptographic thinking-block
/// `signature` strings, which Anthropic validates by content, not by request-byte
/// position. The native RESPONSE is still relayed untouched (the byte-identity
/// guarantee is on the response, not the request).
///
/// Returns `Err` (caller fails closed) if the body is not a JSON object or
/// re-serialization fails — never a silently-unmodified or empty body.
fn rewrite_relayed_model(
    body: &axum::body::Bytes,
    upstream_model_id: &str,
) -> Result<axum::body::Bytes, serde_json::Error> {
    let mut value: serde_json::Value = serde_json::from_slice(body)?;
    let obj = value.as_object_mut().ok_or_else(|| {
        serde_json::Error::io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "native /v1/messages body is not a JSON object",
        ))
    })?;
    obj.insert(
        "model".to_string(),
        serde_json::Value::String(upstream_model_id.to_string()),
    );
    let bytes = serde_json::to_vec(&value)?;
    Ok(axum::body::Bytes::from(bytes))
}

/// Build the x402 [`PaymentRequired`] challenge for `/v1/chat/completions`.
///
/// The SINGLE source of truth for the 402 `accepts[]` shape, shared by both the
/// per-request quote path ([`chat_completions`]) and the discovery path
/// ([`chat_completions_discovery`]). Sharing one builder guarantees the
/// discovery challenge is byte-shape-identical to the real quote — same
/// scheme(s), same legacy snake_case body, and (via
/// `GatewayError::PaymentChallenge`) the same canonical `PAYMENT-REQUIRED`
/// header.
///
/// - `amount` is the atomic-USDC string to advertise (the per-request quote on
///   the quote path; the non-binding discovery floor on the discovery path).
/// - `cost_breakdown` is the matching breakdown to embed.
/// - `resource_url` is the endpoint the challenge is for (`/v1/chat/completions`
///   or `/v1/messages`); it becomes the `resource.url` the client must echo in
///   its signed payment, binding the payment to the exact endpoint.
///
/// The CONFIGURED mint / recipient are baked in here — never the compile-time
/// constant — so a deployment with a non-default mint (e.g. devnet) advertises
/// an asset its verifier will actually accept. The `escrow` scheme is offered
/// iff an escrow claimer is configured, exactly mirroring the quote path.
fn build_payment_challenge(
    state: &AppState,
    amount: String,
    cost_breakdown: solvela_protocol::CostBreakdown,
    resource_url: &str,
) -> solvela_x402::types::PaymentRequired {
    let mut accepts = vec![solvela_x402::types::PaymentAccept {
        scheme: "exact".to_string(),
        network: solvela_x402::types::SOLANA_NETWORK.to_string(),
        amount: amount.clone(),
        asset: state.config.solana.usdc_mint.clone(),
        pay_to: state.config.solana.recipient_wallet.clone(),
        max_timeout_seconds: solvela_x402::types::MAX_TIMEOUT_SECONDS,
        escrow_program_id: None,
    }];

    // Offer escrow scheme if configured
    if state.escrow_claimer.is_some() {
        accepts.push(solvela_x402::types::PaymentAccept {
            scheme: "escrow".to_string(),
            network: solvela_x402::types::SOLANA_NETWORK.to_string(),
            amount,
            asset: state.config.solana.usdc_mint.clone(),
            pay_to: state.config.solana.recipient_wallet.clone(),
            max_timeout_seconds: solvela_x402::types::MAX_TIMEOUT_SECONDS,
            escrow_program_id: state.config.solana.escrow_program_id.clone(),
        });
    }

    solvela_x402::types::PaymentRequired {
        x402_version: solvela_x402::types::X402_VERSION,
        resource: solvela_x402::types::Resource {
            url: resource_url.to_string(),
            method: "POST".to_string(),
        },
        accepts,
        cost_breakdown,
        error: "Payment required".to_string(),
        // Static Coinbase-Bazaar discovery block so x402scan / agentcash index
        // this resource as INVOCABLE (they read invocability from the live 402
        // body, not OpenAPI). Identical on every challenge — no wallet/amount/
        // time data — and purely additive: it never touches `accepts`,
        // `cost_breakdown`, verification, or settlement. Clients sign `accepts`,
        // not `extensions`. Embedded directly (a tolerated non-canonical embed)
        // because Solvela self-settles and is not on Coinbase's facilitator.
        extensions: Some(solvela_x402::types::bazaar_discovery_extension()),
    }
}

/// Build the DISCOVERY 402 challenge — a non-binding advertisement that
/// `/v1/chat/completions` is a payable x402 resource.
///
/// Returned for UNPAID requests that an x402 registry health-checker sends to
/// probe the protocol: a `GET`, or a `POST` with an empty/unparseable/invalid
/// body and NO `PAYMENT-SIGNATURE` header. Without this, those probes saw
/// 405/400/422 (Axum rejects a bad body before the handler runs) and the
/// service was marked "degraded / unknown protocol".
///
/// The advertised amount is the [`discovery_floor_atomic`] — the minimum
/// non-zero per-request cost across the model registry, an honest LOWER BOUND.
/// It is explicitly NOT a binding quote: the price is model- and
/// token-dependent and is only known once a valid request is parsed (that path
/// returns the exact per-request quote, unchanged). The embedded
/// `cost_breakdown` surfaces that floor as the `total` (the 5% platform fee is
/// already folded in), with a `provider_cost`/`platform_fee` split derived by
/// the canonical integer fee math so the parts sum back to the total and the
/// fee is never re-applied.
///
/// This is a pure builder (no payment verification, no settlement, no provider
/// call, no budget or spend mutation) — the discovery path is read-only.
///
/// `resource_url` is the endpoint being advertised (`/v1/chat/completions` or
/// `/v1/messages`), so a discovery probe to either endpoint advertises a
/// challenge bound to that endpoint.
pub(crate) fn chat_completions_discovery(state: &AppState, resource_url: &str) -> GatewayError {
    let floor_atomic = discovery_floor_atomic(&state.model_registry);
    // Atomic → decimal string (6 dp) for the wire `amount`/breakdown. Integer
    // math only (solvela-fintech): split the atomic value into whole + 6-digit
    // fractional USDC. This is a display projection of an exact integer, not a
    // float computation.
    let amount = floor_atomic.to_string();

    // The discovery floor is a single all-in figure (the registry estimate
    // already folds in the 5% platform fee exactly once). Present it honestly:
    // the total is the floor; the provider_cost / platform_fee split is derived
    // by the canonical integer fee math so the breakdown sums to the total and
    // never re-applies the fee.
    //   total = provider * 105/100  ⇒  provider = floor(total * 100 / 105)
    let total_atomic = floor_atomic;
    let provider_atomic = (total_atomic as u128 * 100 / 105) as u64;
    let fee_atomic = total_atomic.saturating_sub(provider_atomic);
    let to_usdc =
        |atomic: u64| -> String { format!("{}.{:06}", atomic / 1_000_000, atomic % 1_000_000) };
    let cost_breakdown = solvela_protocol::CostBreakdown {
        provider_cost: to_usdc(provider_atomic),
        platform_fee: to_usdc(fee_atomic),
        total: to_usdc(total_atomic),
        currency: "USDC".to_string(),
        fee_percent: solvela_protocol::PLATFORM_FEE_PERCENT,
    };

    info!(
        floor_atomic,
        "returning x402 discovery challenge (non-binding floor; exact quote requires a valid request)"
    );
    counter!("solvela_payments_total", "status" => "discovery").increment(1);

    GatewayError::PaymentChallenge(Box::new(build_payment_challenge(
        state,
        amount,
        cost_breakdown,
        resource_url,
    )))
}

/// GET /v1/chat/completions — x402 discovery probe.
///
/// A GET carries no body and cannot be paid, so it always returns the discovery
/// 402 challenge (constraint: discovery is for UNPAID requests only). Registry
/// health-checkers use this to confirm the resource speaks x402 before ever
/// building a payment.
pub async fn chat_completions_discovery_get(
    State(state): State<Arc<AppState>>,
) -> Result<Response, GatewayError> {
    Err(chat_completions_discovery(&state, "/v1/chat/completions"))
}

/// Inputs for one chat-path receipt (settlement-platform P2). Groups the
/// money-relevant fields so [`emit_chat_receipt`] stays a single, reviewable
/// signature across the three spend-log arms.
struct ChatReceiptInputs<'a> {
    model: &'a str,
    scheme: PaymentScheme,
    tx_signature: &'a Option<String>,
    payer_wallet: &'a str,
    /// Amount actually billed (mirrors the spend ledger: discounted on an
    /// escrow semantic-cache hit, full otherwise), atomic USDC.
    ///
    /// This is the BILLED amount from the gateway ledger's perspective —
    /// identical to the spend ledger — and can differ from the raw on-chain
    /// transfer when an agent overpays the 402 quote (`client_amount` >
    /// expected): the receipt records what was billed, not what moved.
    amount_paid_atomic: u64,
    /// The registry CostBreakdown that produced the bill (actual usage on the
    /// non-streaming arm; the C1 estimate on the usage-less/streaming arms).
    breakdown: &'a solvela_protocol::CostBreakdown,
}

/// Allocate, persist (fire-and-forget), and advertise a client-facing receipt
/// for a PAID chat completion.
///
/// Called at exactly the points where spend logging fires — every paid
/// completion that writes a spend row writes a receipt row (the #541
/// streaming bug class). For STREAMING responses this runs while the SSE
/// `Response` is constructed but not yet transmitted, so the
/// `x-solvela-receipt` header is decided BEFORE the body starts; the recorded
/// amounts are the same estimate the spend ledger bills on that path.
///
/// Fail-closed, never over-promise:
/// - no `DATABASE_URL` → no receipt row and NO header (rule 12 graceful
///   degradation — never advertise an unfetchable receipt);
/// - a breakdown string that fails the checked atomic conversion → skip the
///   receipt (counter + warn), header not emitted; the spend row and billing
///   are unaffected.
fn emit_chat_receipt(
    state: &Arc<AppState>,
    response: &mut Response,
    inputs: ChatReceiptInputs<'_>,
) {
    if state.db_pool.is_none() {
        return;
    }
    // Convert the breakdown's decimal strings to canonical atomic units via
    // the same checked conversion the 402 quote uses (rejects empty/
    // non-numeric/negative/overflow — solvela-fintech fail-closed rule).
    let atomic = |decimal: &str| -> Option<u64> {
        usdc_atomic_amount_checked(decimal)
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
    };
    // `total` is authoritative (it is what was billed). DERIVE the fee component
    // from it so the three receipt atomics always reconcile:
    //   total = provider * 105/100  ⇒  fee = total - provider
    // The registry estimate rounds `provider_cost`, `platform_fee`, and `total`
    // INDEPENDENTLY as float strings (`format!("{:.6}")` in
    // `ModelRegistry::estimate_cost`), so parsing `platform_fee` from its own
    // string can disagree with `total - provider` by 1 atomic unit (1 micro-USDC).
    // Letting the ±1 integer-USDC rounding skew land in the fee component is the
    // accepted treatment everywhere else (see `chat_completions_discovery` above).
    // We keep `provider_cost_atomic` (the real upstream cost) and `total_atomic`
    // (what was billed) from the breakdown, and never parse `platform_fee`
    // independently.
    let (Some(provider_cost_atomic), Some(total_atomic)) = (
        atomic(&inputs.breakdown.provider_cost),
        atomic(&inputs.breakdown.total),
    ) else {
        counter!("solvela_receipt_skipped_total", "reason" => "corrupt_breakdown").increment(1);
        warn!(
            model = %inputs.model,
            provider_cost = %inputs.breakdown.provider_cost,
            total = %inputs.breakdown.total,
            "skipping receipt: cost breakdown failed the checked atomic conversion — header not emitted"
        );
        return;
    };
    // Checked subtraction: `total ≈ provider * 1.05`, so `total ≥ provider`
    // always holds for a well-formed breakdown. If a corrupt breakdown ever
    // reports `provider_cost > total`, fail closed the SAME way as a bad atomic
    // conversion (skip + counter) rather than underflow into a negative/wrapped
    // fee — never produce a fee from saturating-to-zero either.
    let Some(platform_fee_atomic) = total_atomic.checked_sub(provider_cost_atomic) else {
        counter!("solvela_receipt_skipped_total", "reason" => "corrupt_breakdown").increment(1);
        warn!(
            model = %inputs.model,
            provider_cost = %inputs.breakdown.provider_cost,
            total = %inputs.breakdown.total,
            provider_cost_atomic,
            total_atomic,
            "skipping receipt: breakdown provider_cost exceeds total (cannot derive a \
             non-negative platform fee) — header not emitted"
        );
        return;
    };
    let record = receipts::ReceiptRecord {
        receipt_id: uuid::Uuid::new_v4(),
        model: inputs.model.to_string(),
        payment_scheme: inputs.scheme.as_accepted_str().to_string(),
        tx_signature: inputs.tx_signature.clone(),
        payer_wallet: inputs.payer_wallet.to_string(),
        amount_paid_atomic: inputs.amount_paid_atomic,
        provider_cost_atomic,
        platform_fee_atomic,
        total_atomic,
        // Chat-path receipts never carry vendor settlement — that is the
        // services-proxy P1 path.
        vendor: None,
    };
    let path = receipts::record_receipt(state.db_pool.as_ref(), record);
    receipts::insert_receipt_header(response, &path);
}

/// Resolve the inbound request's model to its registry PROVIDER (e.g.
/// `"anthropic"`, `"openai"`), for the `/v1/messages` native-vs-reshape fork.
///
/// Reuses the SAME [`resolve_model_with_debug`] resolution the money-path core
/// uses (aliases, eco/auto profiles via the scorer, direct IDs), then looks up
/// the resolved model's registry entry — so `create_message`'s fork decision can
/// never disagree with the core's. Returns `None` when the model does not
/// resolve to a registered model (an unknown model). The core re-resolves and
/// re-checks `model_info.provider == "anthropic"` itself, so this is only an
/// upstream hint that selects the strict-vs-lenient inbound translation; it is
/// not authoritative over the core's own fork decision.
pub(crate) fn resolve_model_provider(req: &ChatRequest, state: &AppState) -> Option<String> {
    let (resolved_model, _, _, _) = resolve_model_with_debug(req, state).ok()?;
    state
        .model_registry
        .get(&resolved_model)
        .map(|m| m.provider.clone())
}

/// Tier sentinel meaning "the smart router never ran" — emitted verbatim in the
/// `X-Solvela-Tier` debug header (paired with a `0.0` `X-Solvela-Score`) for the
/// alias and direct-model-ID branches of [`resolve_model_with_debug`].
///
/// A **header convention only**. It must never reach the spend ledger: run it
/// through [`routing_telemetry`] first.
pub const ROUTING_TIER_NOT_ROUTED: &str = "N/A";

/// Translate the debug-header tier/score pair into what `spend_logs` stores.
///
/// "The router did not run" is a single semantic state, so it gets a single
/// encoding — `(None, None)`, i.e. the same NULL/NULL the genuinely router-less
/// paths (service proxy, search, A2A) already write. Persisting the
/// `"N/A"`/`0.0` sentinel instead would make `'N/A'` a pseudo-tier in `GROUP BY`,
/// drag every `AVG(routing_score)`/percentile toward zero (direct model IDs are
/// plausibly the bulk of paid traffic), and leave `WHERE routing_tier IS NULL`
/// silently missing those rows.
///
/// A real router run that happens to score `0.0` is real data and is preserved.
pub fn routing_telemetry(tier: &str, score: f64) -> (Option<String>, Option<f64>) {
    if tier == ROUTING_TIER_NOT_ROUTED {
        (None, None)
    } else {
        (Some(tier.to_string()), Some(score))
    }
}

/// Resolve model ID from aliases, smart routing profiles, or direct model IDs.
///
/// Returns (resolved_model, profile_name, tier_name, score) for debug headers.
/// The non-routed branches return the [`ROUTING_TIER_NOT_ROUTED`] sentinel —
/// callers persisting to the ledger must map it via [`routing_telemetry`].
fn resolve_model_with_debug(
    req: &ChatRequest,
    state: &AppState,
) -> Result<(String, String, String, f64), GatewayError> {
    // Check for profile-based routing (e.g., "auto", "eco", "premium")
    if let Some(profile) = Profile::from_alias(&req.model) {
        // Scorer dimension 13 (tool usage). A request that ships tool
        // definitions is agentic work and should score up. An EMPTY `tools: []`
        // array is not tool usage — clients send it as a no-op.
        //
        // MONEY PATH: on paid profiles this can push a tool-carrying request up
        // a tier and therefore select a costlier model. Pass-through pricing +
        // the 5% fee are unchanged; the amount charged follows whichever model
        // actually serves.
        let has_tools = req.tools.as_ref().is_some_and(|t| !t.is_empty());
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
            ROUTING_TIER_NOT_ROUTED.to_string(),
            0.0,
        ));
    }

    // Check if it's a direct model ID
    if state.model_registry.get(&req.model).is_some() {
        return Ok((
            req.model.clone(),
            "direct".to_string(),
            ROUTING_TIER_NOT_ROUTED.to_string(),
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

    /// The ledger must never persist the "N/A"/0.0 debug-header sentinel:
    /// "router did not run" is NULL/NULL on the spend row, exactly like the
    /// router-less paths (proxy/search/A2A), so routing_score aggregates are
    /// not biased toward zero by direct-model-ID traffic.
    #[test]
    fn routing_telemetry_maps_sentinel_to_none() {
        assert_eq!(
            routing_telemetry(ROUTING_TIER_NOT_ROUTED, 0.0),
            (None, None)
        );
        assert_eq!(
            routing_telemetry("Complex", 0.87),
            (Some("Complex".to_string()), Some(0.87))
        );
        // A genuine router run that scores 0.0 is still real output.
        assert_eq!(
            routing_telemetry("Simple", 0.0),
            (Some("Simple".to_string()), Some(0.0))
        );
    }
}
