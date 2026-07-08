//! v0 spend-down channel CHAT draw (PR-B) — the channel-voucher fork for
//! `POST /v1/chat/completions` and `POST /v1/messages`.
//!
//! Extends the shipped `/v1/search` draw contract (`routes/search.rs`,
//! PR #664/#665) to the chat money path, per the converged channel plan
//! (2026-07-03, Decisions A/E/G LOCKED):
//!
//! - **Sign-the-quote (Decision A):** the voucher's delta is the fee-inclusive
//!   402 ESTIMATE (`billed`) — `verify_voucher` enforces
//!   `cumulative − last == billed` exactly. The post-serve ACTUAL advances the
//!   synchronous `realized` counter by `min(actual, billed)` (§6b clamp), so
//!   the close refund `deposited − realized` returns the quote−actual gap.
//!   Streaming never produces an actual (`EstimateFallback`), so realized ==
//!   billed there — the decisive case behind Decision A.
//! - **Search budget posture (Decision E):** NO `check_budget` / reservation —
//!   the on-chain-verified deposit IS the budget; `require_tenant = TRUE`
//!   wallets are rejected (#499), sourced from the DB `agent_wallet`, never
//!   the voucher.
//! - **Deliver-then-record (#486):** verify pre-serve, persist post-serve, in
//!   ONE transaction (`persist_voucher_and_advance`) — the ONLY debit site.
//!   Provider failure and a persist that lost the invariant-11 close race
//!   record NOTHING (positive receipt ⟺ `last` advanced ⟺ agent debited).
//!   NOTE — the "record" side commits when the serve BEGINS, not when the
//!   client has received every byte: for the buffered arms the debit follows a
//!   completed provider response, but for the STREAMING arms (OpenAI SSE and
//!   #651 native Anthropic SSE) the persist runs once the SSE `Response` is
//!   built (`provider.rs`), so a mid-stream client break debits the full signed
//!   quote (`EstimateFallback`) with no compensating refund. `#486`
//!   "deliver-then-record" therefore means "record once the serve is handed
//!   off", not "once delivery completes".
//!   `ponytail:` known limitation — byte-accounted partial-delivery streaming
//!   settlement (refund the undelivered fraction) is a separate future feature,
//!   deliberately out of PR-B scope; the residual is one quote per broken
//!   stream, bounded and metered like the `exact` overage posture.
//! - **Never on-chain per call (plan HALT 3/5):** the fork bypasses
//!   `verify_and_settle`, the tx-replay cache, and `fire_escrow_claim`
//!   entirely — a voucher has no transaction to settle, replay, or claim.
//! - **Per-channel lock (Decision G):** held across the WHOLE draw (serve
//!   included — chat serves legitimately run to the 600s native-relay
//!   deadline), 900s TTL crash backstop, token-guarded Lua release via an RAII
//!   guard whose `Drop` best-effort-releases (a cancelled/panicked serve still
//!   frees the lock). The serve itself is bounded by a hard timeout well below
//!   the TTL, and a channel draw is a SINGLE quoted provider/model call — the
//!   client fallback-preference header is neutralised so no unbounded provider
//!   chain can run under the lock (Decision E).

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::http::HeaderMap;
use axum::response::Response;
use metrics::{counter, histogram};
use tracing::{error, warn};

use solvela_protocol::ChatRequest;

use crate::cache::ResponseCache;
use crate::channels::{ChannelDrawLockGuard, ChannelRepoError, CHANNEL_SLOT_MAX_STALENESS};
use crate::error::GatewayError;
use crate::receipts;
use crate::usage::SpendLogEntry;
use crate::AppState;

// `ChannelDrawLockGuard`, `LOCK_RELEASE_TIMEOUT_SECS`, and
// `CHANNEL_SLOT_MAX_STALENESS` were hoisted into `crate::channels` (D7,
// 2026-07-06 channel-on-A2A plan) so chat and the A2A channel leg share ONE
// lock-lifecycle implementation — visibility/motion only, zero behavior
// change. Any future divergent fix to draw orchestration lands there.

use super::cost::{
    cap_usage_to_request_limits, compute_actual_atomic_cost, estimate_input_tokens,
    scheme_realized_discount, select_spend_log_arm, spend_cost_atomic, PaymentScheme, SpendLogArm,
};
use super::provider::{ProviderCallContext, ProviderCallError, ProviderCallResult};
use super::response::build_session_token;
use super::WireDialect;
use crate::routes::debug_headers::PaymentStatus;

/// Everything a chat channel draw needs from the shared money-path core —
/// already validated up-stack (resource/method/network/asset/pay_to checks,
/// model resolution, prompt guard). Grouped so the fork call site stays one
/// reviewable expression (the `ProviderCallContext` precedent).
pub(super) struct ChannelDrawContext<'a> {
    pub state: &'a Arc<AppState>,
    pub headers: &'a HeaderMap,
    pub req: &'a ChatRequest,
    pub model_info: &'a solvela_protocol::ModelRegistration,
    /// Carries the RAW inbound bytes (the voucher digest preimage) and the
    /// native-relay source for `/v1/messages`.
    pub dialect: &'a WireDialect,
    /// The single fee-inclusive estimate (atomic micro-USDC) computed once in
    /// the core — identical to the 402 `exact` entry's `amount` the sidecar
    /// quotes from. This is the voucher's exact required delta.
    pub billed_atomic: u64,
    /// Client-echoed `accepted.amount` — SDK-contract check ONLY; billing
    /// never reads it (the gateway quote is authoritative).
    pub accepted_amount: &'a str,
    pub voucher_payload: &'a solvela_x402::types::ChannelVoucherPayload,
    pub session_id: &'a Option<String>,
    pub request_id: &'a Option<String>,
    /// Attribution-only x-tenant tag (recorded on the spend row; never gates —
    /// require_tenant wallets are rejected outright per Decision E).
    pub tenant: &'a Option<String>,
    pub debug_enabled: bool,
    pub request_start: Instant,
    pub routing_tier: &'a str,
    pub routing_score: f64,
    pub routing_profile: &'a str,
}

/// Handle a chat request paid by a channel voucher. Owns the gates + the lock
/// lifecycle; every money-path invariant is enforced inside
/// [`channel_draw_locked`], which runs entirely under the per-channel lock.
pub(super) async fn channel_draw(ctx: ChannelDrawContext<'_>) -> Result<Response, GatewayError> {
    // Gate: channels ship DISABLED; the draw additionally requires a DB pool
    // (the durable ledger) AND Redis (the per-channel lock). Any missing → 404
    // (the scheme is simply not offered — uniform with open/close/search).
    // Unlike search's raw-body 404, this surfaces through `GatewayError` so
    // `/v1/messages`' error translation produces a well-formed envelope
    // (a raw non-200 Ok(response) would be mangled to a 502 by the native
    // success forwarder).
    if !ctx.state.config.channel.enabled {
        return Err(GatewayError::NotFound("channel not available".to_string()));
    }
    let Some(pool) = ctx.state.db_pool.as_ref() else {
        return Err(GatewayError::NotFound("channel not available".to_string()));
    };
    let Some(cache) = ctx.state.cache.as_ref() else {
        return Err(GatewayError::NotFound("channel not available".to_string()));
    };

    // SDK-contract check (the shipped search contract): `accepted.amount` MUST
    // equal the per-call quote. Billing does NOT read this field — the gateway
    // quote is authoritative — but a mismatch signals an SDK↔gateway price
    // disagreement (e.g. a registry deploy between quote and retry); reject
    // rather than silently ignore it. Static message (never reflect the
    // client-controlled amount string).
    if ctx.accepted_amount != ctx.billed_atomic.to_string() {
        return Err(GatewayError::BadRequest(
            "channel voucher accepted.amount must equal the current per-call quote for this \
             endpoint; re-quote via the 402 challenge"
                .to_string(),
        ));
    }

    // Bind the voucher to THE served request: SHA-256 of the RAW inbound body
    // bytes (both dialects carry them — the sidecar hashes the bytes it sends).
    let request_digest = crate::routes::channel::request_digest(ctx.dialect.raw_body());

    // `current_slot` for the voucher expiry check — RPC-free cached slot (5s
    // TTL), FAIL-CLOSED past [`CHANNEL_SLOT_MAX_STALENESS`]. `None` (no cached
    // value, OR the only cached value is staler than the bound during an RPC
    // outage) → 503: a stale (lower) slot would inflate `verify_voucher`'s
    // expiry buffer and could let a genuinely-expired voucher pass, so we refuse
    // rather than serve on it (FIX 4). Never a per-call `getSlot` (HALT 7).
    let Some(current_slot) =
        crate::routes::escrow::fetch_cached_slot_bounded(ctx.state, CHANNEL_SLOT_MAX_STALENESS)
            .await
    else {
        return Err(GatewayError::ServiceUnavailable(
            "could not reach the Solana cluster to verify the voucher; please retry shortly"
                .to_string(),
        ));
    };

    // Decode the wire voucher into the frozen verifier `Voucher`, fail-closed
    // on any malformed field (incl. the BIGINT-storability guards that close
    // the verify-passes-where-persist-fails free-replay hole).
    let voucher =
        crate::routes::channel::voucher_from_payload(ctx.voucher_payload).map_err(|e| {
            warn!(error = %e, "chat channel draw: malformed voucher payload");
            GatewayError::BadRequest(e.to_string())
        })?;

    let channel_id = ctx.voucher_payload.channel_id.as_str();

    // Per-channel lock (Redis SET NX EX + per-draw token — Decision G). Fail
    // closed if Redis errors (no in-memory fallback — it cannot serialise
    // across instances); reject if a concurrent draw holds it (HALT 6).
    let lock_token = match cache.acquire_channel_draw_lock(channel_id).await {
        Ok(Some(token)) => token,
        Ok(None) => {
            counter!("solvela_channel_draw_lock_rejects_total").increment(1);
            return Err(GatewayError::ServiceUnavailable(
                "a draw is already in progress on this channel; please retry shortly".to_string(),
            ));
        }
        Err(e) => {
            warn!(error = %e, "chat channel draw: lock acquisition failed (Redis)");
            return Err(GatewayError::ServiceUnavailable(
                "payment service is temporarily degraded; please retry shortly".to_string(),
            ));
        }
    };

    // Lock HELD across the whole draw (load → verify → SERVE → persist) via an
    // RAII guard, then RELEASED on ALL paths — the 900s TTL is only the crash
    // backstop. The guard's `Drop` covers cancellation (client disconnect /
    // stop-generation) and panic (neither reaches the explicit release); the
    // token-guarded (Lua compare-and-delete) release means a TTL expiry mid-draw
    // can never cross-delete a successor's lock. The normal path awaits
    // [`ChannelDrawLockGuard::release`] so the next sequential draw proceeds at
    // once.
    let guard =
        ChannelDrawLockGuard::new(Arc::clone(ctx.state), channel_id.to_string(), lock_token);
    let outcome = channel_draw_locked(
        &ctx,
        pool,
        cache,
        guard.token(),
        request_digest,
        current_slot,
        &voucher,
        channel_id,
    )
    .await;
    guard.release().await;
    outcome
}

/// The locked body of a chat channel draw: load state → #499 → verify voucher
/// → SERVE (shared provider dispatch) → post-serve single-transaction persist
/// (`last += billed`, `realized += min(actual, billed)`) → charge-visible
/// spend/receipt. The caller holds the per-channel lock across this whole
/// function and releases it unconditionally afterward.
#[allow(clippy::too_many_arguments)]
async fn channel_draw_locked(
    ctx: &ChannelDrawContext<'_>,
    pool: &sqlx::PgPool,
    cache: &ResponseCache,
    lock_token: &str,
    request_digest: [u8; 32],
    current_slot: u64,
    voucher: &solvela_x402::channel::Voucher,
    channel_id: &str,
) -> Result<Response, GatewayError> {
    // Load the drawable open channel + its DB-sourced `agent_wallet`. A
    // closed/closing/unknown channel is not drawable → 404.
    let drawable =
        match crate::channels::load_open_channel_state(pool, channel_id, request_digest).await {
            Ok(Some(d)) => d,
            Ok(None) => {
                return Err(GatewayError::NotFound(
                    "channel not found or not open".to_string(),
                ));
            }
            Err(e) => {
                warn!(error = %e, "chat channel draw: failed to load channel state");
                return Err(GatewayError::Internal(
                    "could not load the channel".to_string(),
                ));
            }
        };
    let agent_wallet = drawable.agent_wallet;
    let state_view = drawable.state;

    // #499 (Decision E): reject a `require_tenant = TRUE` wallet — the SEARCH
    // budget posture (NO `check_budget`; the deposit is the budget). Wallet
    // sourced from the DB `agent_wallet`, NEVER the voucher (which carries no
    // payer identity). Runs BEFORE any serve so a rejected draw takes no spend.
    if ctx
        .state
        .usage
        .require_tenant_for_wallet(&agent_wallet)
        .await
    {
        warn!("chat channel draw rejected: agent wallet requires per-tenant budgeting (#499)");
        return Err(GatewayError::Forbidden(
            "this wallet requires per-tenant budgeting; channel draws are outside the tenant \
             budget matrix — pay per-request instead"
                .to_string(),
        ));
    }

    // Sign-the-quote (Decision A): the voucher's required delta is the
    // fee-inclusive estimate. `verify_voucher` enforces
    // `cumulative − last == billed` EXACTLY, fail-closed on every rule. On an
    // AUTHENTICATED rejection surface the authoritative `last_cumulative`
    // (structured §4b resync field); on a pre-auth rejection surface nothing.
    let billed = ctx.billed_atomic;
    if let Err(e) =
        solvela_x402::channel::verify_voucher(&state_view, voucher, billed, current_slot)
    {
        counter!("solvela_payments_total", "status" => "failed", "scheme" => "channel")
            .increment(1);
        return Err(crate::routes::channel::map_voucher_rejection(
            e,
            state_view.last_cumulative_atomic,
        ));
    }
    counter!("solvela_payments_total", "status" => "verified", "scheme" => "channel").increment(1);

    // Decision E: a channel draw is a SINGLE quoted provider/model call. Strip
    // the client fallback-preference headers (`x-solvela-fallback-preference` /
    // legacy `x-rcr-...`) so the buffered path cannot assemble an UNBOUNDED
    // client-supplied provider chain under the draw lock — each hop is ~273s
    // worst-case (3 retries × 90s + backoff), and four chained failing providers
    // would exceed the 900s lock TTL and reopen the close-mid-serve grief loop
    // (FIX 1a). exact/escrow are untouched; the native relay is already
    // single-provider by construction (CLAUDE.md #4).
    let mut channel_headers = ctx.headers.clone();
    channel_headers.remove("x-solvela-fallback-preference");
    channel_headers.remove("x-rcr-fallback-preference");

    // SERVE — the SAME dispatch as the paid path (native relay for an
    // Anthropic-resolved `/v1/messages` request, the OpenAI pipeline with
    // cache otherwise; a cache hit STILL draws — operator-approved). The fork
    // NEVER touches `verify_and_settle`, the tx-replay cache, or
    // `fire_escrow_claim` (HALT 3/5).
    let native_source = if ctx.model_info.provider == "anthropic" {
        ctx.dialect.anthropic_native_source()
    } else {
        None
    };
    let pctx = ProviderCallContext {
        state: ctx.state,
        req: ctx.req,
        model_info: ctx.model_info,
        // Fallback-stripped (above): channel serves one quoted provider.
        headers: &channel_headers,
        debug_enabled: ctx.debug_enabled,
        request_start: ctx.request_start,
        routing_tier: ctx.routing_tier,
        routing_score: ctx.routing_score,
        routing_profile: ctx.routing_profile,
        session_id: ctx.session_id,
        payment_status: PaymentStatus::Verified,
    };
    // FIX 1b: bound the serve HELD under the draw lock with a hard timeout well
    // below the 900s TTL (config `draw_serve_timeout_secs`, default 650s: above
    // the 600s native-relay ceiling). A serve that exceeds it is treated exactly
    // like a provider failure — total non-charge (nothing persisted, deposit
    // intact); the lock is released by the RAII guard on return.
    let serve_timeout = Duration::from_secs(ctx.state.config.channel.draw_serve_timeout_secs());
    let serve = async {
        if let Some((body, version, beta)) = native_source {
            super::run_native_relay(
                ctx.state,
                ctx.req,
                ctx.model_info,
                body,
                version,
                beta,
                ctx.session_id,
            )
            .await
        } else {
            super::provider::execute_provider_call(&pctx).await
        }
    };
    let provider_call = match tokio::time::timeout(serve_timeout, serve).await {
        Ok(r) => r,
        Err(_elapsed) => {
            counter!("solvela_channel_draw_serve_timeout_total").increment(1);
            warn!(
                channel_id,
                timeout_secs = serve_timeout.as_secs(),
                "chat channel draw: provider serve exceeded the draw-lock timeout — total \
                 non-charge (deposit intact; lock released)"
            );
            return Err(GatewayError::UpstreamUnavailable(
                "No provider could serve your request in time and your channel was NOT drawn. \
                 Please retry shortly."
                    .to_string(),
            ));
        }
    };

    let ProviderCallResult {
        mut response,
        usage,
        actual_provider,
        cost_outcome,
    } = match provider_call {
        Ok(r) => r,
        Err(ProviderCallError::AllProvidersFailed {
            model, provider, ..
        }) => {
            // Provider failure: NO persist → `last`/`realized` do NOT advance →
            // the agent keeps its draw and was NOT debited. LEDGER INVARIANT:
            // a positive spend/receipt exists IFF `last` advanced IFF the agent
            // was debited, so a non-charge writes NOTHING. Retryable 503 with a
            // static, internals-free message (GHSA-cgqx-mg48-949v).
            counter!("solvela_paid_stub_rejections_total").increment(1);
            warn!(
                provider = %provider,
                model = %model,
                channel_id,
                "chat channel draw: no provider available — total non-charge (deposit intact)"
            );
            return Err(GatewayError::UpstreamUnavailable(
                "No provider could serve your request right now and your channel was NOT \
                 drawn. Please retry shortly."
                    .to_string(),
            ));
        }
        Err(ProviderCallError::Internal(msg)) => {
            // Same non-charge contract: nothing persisted, nothing recorded.
            warn!(
                channel_id,
                "chat channel draw: internal provider-call error — total non-charge"
            );
            return Err(GatewayError::Internal(msg));
        }
    };

    // Cap provider-reported token counts BEFORE pricing (solvela-fintech §3):
    // a misbehaving provider must not inflate the realized obligation.
    let usage = usage
        .as_ref()
        .map(|u| cap_usage_to_request_limits(u, ctx.req, ctx.model_info));

    // The realized advance (Decision A + §6b). Arm selection is the same pure
    // exhaustive `select_spend_log_arm` the paid path uses (a new arm is a
    // compile error here too). Channel realizes the semantic discount on the
    // realized side ONLY (`scheme_realized_discount`) — the voucher's signed
    // cumulative is untouched.
    let realized_discount = scheme_realized_discount(PaymentScheme::Channel, cost_outcome);
    let arm_billed = match select_spend_log_arm(false, usage.as_ref(), cost_outcome.is_some()) {
        // Structurally unreachable: the first argument is literally `false`
        // (channel draws have no deferred exact settle), so `select_spend_log_arm`
        // never returns `SkipSettleFailed`. Fail LOUD rather than silently bill a
        // value nobody flagged — a future refactor that makes this reachable
        // (e.g. threading a real bool) trips this in CI. Panicking here fails
        // closed (non-charge, lock released by the guard), not a wrong charge.
        SpendLogArm::SkipSettleFailed => {
            unreachable!(
                "channel draws call select_spend_log_arm with `false`; SkipSettleFailed is \
                 unreachable — if reached, a refactor introduced a deferred-settle path that \
                 must be handled explicitly on the channel money path"
            )
        }
        SpendLogArm::ActualUsage(u) => {
            let full = match compute_actual_atomic_cost(
                u.prompt_tokens,
                u.completion_tokens,
                ctx.model_info,
            ) {
                Some(v) => v,
                // Corrupt registry pricing (non-finite/negative/overflow).
                // Refusing to persist would make EVERY draw on this model a
                // free serve (the HALT-11 free-replay class), so bill the
                // SIGNED QUOTE — the amount the agent authorized — loudly.
                None => {
                    counter!(
                        "solvela_channel_realized_fallback_total",
                        "reason" => "corrupt_actual_cost"
                    )
                    .increment(1);
                    warn!(
                        model = %ctx.req.model,
                        channel_id,
                        prompt_tokens = u.prompt_tokens,
                        completion_tokens = u.completion_tokens,
                        billed_atomic = billed,
                        "chat channel draw: actual cost computation failed (corrupt registry \
                         pricing) — realizing the signed quote instead of the actual"
                    );
                    billed
                }
            };
            spend_cost_atomic(realized_discount, full)
        }
        // Usage-less semantic hit: realize the discounted amount (computed at
        // hit time from the full price — `SemanticDiscount` guarantees
        // discounted ≤ full).
        SpendLogArm::UsagelessSemanticHit => spend_cost_atomic(realized_discount, billed),
        // Streaming (both dialects): no actual ever exists — realize the
        // signed quote (refund delta 0 for this call). Decision A's case.
        SpendLogArm::EstimateFallback => billed,
    };
    // §6(b) clamp — LOAD-BEARING: capped actual (prompt side) can exceed the
    // quote; unclamped it would violate the `realized ≤ last` CHECK and make
    // the persist deterministically fail AFTER a successful serve (free-replay
    // under warn-and-continue). The signature IS the cap: the agent authorized
    // `billed`; overage beyond it is bounded gateway loss (the `exact`
    // posture), now metered.
    let realized_advance = arm_billed.min(billed);
    if arm_billed > billed {
        counter!("solvela_channel_overage_clamped_total").increment(1);
        counter!("solvela_channel_overage_clamped_atomic_total").increment(arm_billed - billed);
    }

    // Session-token parity with the paid path (non-streaming responses carry
    // the session header regardless of scheme). Inserted before the persist so
    // every DELIVERED response (including the bounded persist-failure arm
    // below) has the same response shape as an exact/escrow-paid one.
    if !ctx.req.stream {
        if let Some(token) = build_session_token(&agent_wallet, &ctx.state.session_secret) {
            if let Ok(hv) = axum::http::HeaderValue::from_str(&token) {
                response.headers_mut().insert(
                    axum::http::HeaderName::from_static("x-solvela-session"),
                    hv.clone(),
                );
                response
                    .headers_mut()
                    .insert(axum::http::HeaderName::from_static("x-rcr-session"), hv);
            }
        }
    }

    // FIX 3 (belt-and-suspenders behind the bounded serve): re-confirm we STILL
    // hold the draw lock before persisting. If the TTL expired mid-serve a
    // successor draw could have been admitted on the SAME stale snapshot; the DB
    // CAS (`WHERE last < $new AND status = 'open'`) already stops the double
    // DEBIT, but the loser would still run to completion and DELIVER for free.
    // On a confirmed loss, abort the persist and take the bounded non-charge
    // deliver arm with a distinct `reason="lock_lost"` label. A Redis error here
    // is inconclusive → proceed (the DB CAS remains the money backstop; a
    // transient blip must not force a free serve).
    match cache.channel_draw_lock_held(channel_id, lock_token).await {
        Ok(true) => {}
        Ok(false) => {
            counter!(
                "solvela_channel_draw_persist_failed_total",
                "reason" => "lock_lost"
            )
            .increment(1);
            warn!(
                channel_id,
                cumulative_atomic = voucher.cumulative_atomic,
                "chat channel draw: draw lock no longer ours before persist (TTL expired \
                 mid-serve) — aborting persist, bounded one-call non-charge (deposit intact)"
            );
            return Ok(response);
        }
        Err(e) => {
            warn!(
                error = %e,
                channel_id,
                "chat channel draw: could not re-check draw-lock ownership before persist; \
                 proceeding (DB CAS is the money backstop)"
            );
        }
    }

    // POST-serve persist (deliver-then-record, #486): ONE transaction advances
    // `last` by the signed quote AND `realized` by the clamped actual —
    // advancing `last` IS the debit, so the spend/receipt below are contingent
    // on it committing. The `AND status = 'open'` predicate makes a close that
    // froze the refund amount win over this in-flight draw (invariant 11).
    // This awaited write is the sanctioned synchronous exception to
    // fire-and-forget (Decision A: realized must be synchronous).
    if let Err(e) = crate::channels::persist_voucher_and_advance(
        pool,
        &crate::channels::VoucherRecord {
            channel_id,
            cumulative_atomic: voucher.cumulative_atomic,
            call_cost_atomic: billed,
            realized_advance_atomic: realized_advance,
            expiry_slot: voucher.expiry_slot,
            nonce: voucher.nonce,
            request_digest: &voucher.request_digest,
            signature: &voucher.signature,
        },
    )
    .await
    {
        // A persist failure AFTER a successful serve is a BOUNDED one-call
        // gateway loss — the agent still receives its response, but `last` did
        // NOT advance (deposit intact), so NO debit occurred and, by the
        // ledger invariant, NO spend/receipt is written. All arms DELIVER the
        // earned response (invariant-11 behaviour); the shared classifier
        // (`crate::channels::persist_failure_reason` — PR-0, one code site for
        // chat/search) splits ONLY the observability, by NAME, so a DB OUTAGE
        // (which free-serves every draw) PAGES via the #684 cron while the
        // routine close-race loser stays notice-tier:
        //   - `race`      = the accepted invariant-11 loser — notice-only,
        //                   stays a `warn!`;
        //   - `db_error` / `overflow` / `unexpected` = paging `error!`, ALSO
        //                   emitted on `solvela_channel_draw_persist_page_total`.
        let reason = crate::channels::emit_persist_failure_counters(&e);
        if matches!(e, ChannelRepoError::AdvanceNotApplied) {
            warn!(
                error = %e,
                channel_id,
                reason,
                cumulative_atomic = voucher.cumulative_atomic,
                last_cumulative = state_view.last_cumulative_atomic,
                "chat channel draw: persist lost the invariant-11 close race — bounded one-call \
                 gateway loss, NO charge recorded (deposit intact; agent resyncs on next draw)"
            );
        } else {
            error!(
                error = %e,
                channel_id,
                reason,
                cumulative_atomic = voucher.cumulative_atomic,
                last_cumulative = state_view.last_cumulative_atomic,
                "chat channel draw: persist FAILED after a successful serve (not a close race) — \
                 every channel draw is free-serving; investigate immediately (deposit intact, NO \
                 charge recorded)"
            );
        }
        return Ok(response);
    }

    // `last` advanced ⇒ the agent WAS debited ⇒ record the positive spend +
    // receipt for the REALIZED amount (real spend, solvela-fintech §5) — the
    // ONLY site a chat channel spend/receipt row is written. wallet = DB
    // `agent_wallet`, `tx_signature = None` (a voucher has no on-chain tx),
    // scheme = "channel", no vendor leg. Both writes are fire-and-forget.
    counter!("solvela_payments_total", "status" => "settled", "scheme" => "channel").increment(1);
    let billed_usdc = realized_advance as f64 / 1_000_000.0;
    histogram!("solvela_payment_amount_usdc").record(billed_usdc);

    let (input_tokens, output_tokens) = match usage.as_ref() {
        Some(u) => (u.prompt_tokens, u.completion_tokens),
        // No token data (streaming / usage-less hit): request-side input
        // estimate + zero output, consistent with the paid path's usage-less
        // arms. Attribution only — the billed amount is `realized_advance`.
        None => (estimate_input_tokens(ctx.req), 0),
    };
    ctx.state.usage.log_spend(SpendLogEntry {
        wallet_address: agent_wallet.clone(),
        model: ctx.req.model.clone(),
        provider: actual_provider.unwrap_or_else(|| ctx.model_info.provider.clone()),
        input_tokens,
        output_tokens,
        cost_usdc: billed_usdc,
        tx_signature: None,
        request_id: ctx.request_id.clone(),
        session_id: ctx.session_id.clone(),
        tenant: ctx.tenant.clone(),
        // Never enforced on a channel draw (Decision E: no budget matrix; a
        // require_tenant wallet was rejected above).
        tenant_enforced: false,
        // No reservation was committed (no check_budget), so log_spend
        // increments the counters by `cost_usdc` directly (None branch).
        estimated_cost_usdc: None,
        vendor: None,
        routing_tier: Some(ctx.routing_tier.to_string()),
        routing_score: Some(ctx.routing_score),
    });

    // Receipt: the canonical integer split of the REALIZED total —
    //   total = provider × 105/100  ⇒  provider = floor(total × 100 / 105),
    //   fee = total − provider
    // (the discovery-floor / A2A-record idiom), so the three atomics always
    // reconcile and the fee is never re-applied. `amount_paid == total ==
    // realized_advance` mirrors the spend ledger exactly.
    let provider_cost_atomic = (realized_advance as u128 * 100 / 105) as u64;
    let platform_fee_atomic = realized_advance - provider_cost_atomic;
    let record = receipts::ReceiptRecord {
        receipt_id: uuid::Uuid::new_v4(),
        model: ctx.req.model.clone(),
        payment_scheme: PaymentScheme::Channel.as_accepted_str().to_string(),
        tx_signature: None,
        payer_wallet: agent_wallet,
        amount_paid_atomic: realized_advance,
        provider_cost_atomic,
        platform_fee_atomic,
        total_atomic: realized_advance,
        vendor: None,
    };
    let receipt_header_path = receipts::record_receipt(ctx.state.db_pool.as_ref(), record);
    receipts::insert_receipt_header(&mut response, &receipt_header_path);

    Ok(response)
}
