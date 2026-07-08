//! The A2A channel-voucher settlement leg (channel-on-A2A plan Slice A,
//! 2026-07-06 — operator-GREENLIT with D1-c/D2-a/D3-a/D4-a/D5-a/D6-a/D7-a).
//!
//! Extends the spend-down channel (merged #664–#669) to the A2A paid leg:
//! `settle_paid_task`'s former `PayloadData::Channel` reject arm now enters
//! here, carrying the four donor-side guards the reused functions do NOT
//! contain — the #499 tenant re-check from the DB wallet, DB-wallet ledger
//! attribution, the `channel.enabled` kill switch (enforced at intake,
//! `handle_payment_submitted`), and the chat pre-fork network/asset/pay_to
//! static checks.
//!
//! Money-path shape (§5 of the plan; every REUSED step is byte-identical
//! machinery from the chat/search draws):
//!
//! - **Billed = the task's stored quote atomic EXACTLY** — A2A is
//!   quote == cap == settlement (#504), so `realized_advance = billed` (the
//!   chat §6b `min(actual, billed)` clamp is degenerate here). No new fee or
//!   amount computation anywhere.
//! - **Deliver-then-record for this payment kind (#486/D4-a):** the debit
//!   (advancing `last` in `persist_voucher_and_advance`'s single transaction)
//!   fires only AFTER a successful serve; provider failure / serve timeout is
//!   a total non-charge and the SAME voucher retries. This inverts the
//!   exact/escrow legs' settle-then-serve order FOR THIS KIND ONLY and is
//!   strictly client-favorable (no charge-without-delivery arm exists).
//! - **Locks (D2-a):** per-task settle lock OUTER (already held on entry,
//!   with the `Working` marker written), per-channel draw lock INNER — the
//!   only acquisition order any request ever takes, deadlock-free by
//!   construction. The draw guard (D7, `crate::channels`) releases FIRST on
//!   every arm; pre-debit failures then revert Working and release the settle
//!   lock; terminal arms (debited AND deliver-free) leave the settle lock
//!   HELD (self-expires) exactly like the exact leg.
//! - **Replay defense = the monotonic-cumulative triad** (D3 digest binding +
//!   `cumulative − last == billed` + draw lock + DB CAS +
//!   `UNIQUE(channel_id, cumulative_atomic)`), NOT `check_and_record_tx` — a
//!   voucher has no tx signature; the bypass is named and sanctioned (§9
//!   item 3), mirroring the chat draw.
//! - **Deliver-free arms (D5-a):** a persist that lost the invariant-11 close
//!   race or the lock-ownership recheck still DELIVERS the earned response,
//!   completes the task with the `TaskRecord::deliver_free` marker, omits
//!   BOTH `x402.payment.status` and `x402.payment.receipts` (key OMISSION
//!   only — names never change), writes NO ledger row, and emits PR-0's tier
//!   split via the shared classifier (page-tier on `db_error`/`overflow`/
//!   `unexpected`, notice-tier on `race`/`lock_lost`).
//!
//! Five clocks (§5): settle lock 120s (the `Working` marker, not the TTL,
//! bounds task-level re-entry) < serve bound
//! `channel.a2a_draw_serve_timeout_secs` (540s default, D6, boot-validated) <
//! TASK_TTL 600s (the record provably outlives every serve, keeping
//! `tasks/get` recovery alive for a debited client) < draw lock 900s (TTL
//! loss covered by the pre-persist ownership recheck). The fifth clock — the
//! voucher's own `expiry_slot` — is checked ONCE in `verify_voucher`
//! (acceptance gate, ≥ 50-slot buffer) and deliberately never re-checked at
//! persist: a voucher verified in time is legitimately consumed up to ~540s
//! past its signed expiry (documented client guidance: sign
//! `expiry_slot ≥ current slot + ~1,500 slots`).

use std::sync::Arc;
use std::time::Duration;

use metrics::counter;
use serde_json::{json, Value};
use tracing::{error, info, warn};

use solvela_protocol::{ChatMessage, ChatRequest, ModelRegistration};

use crate::a2a::handler::{
    new_wire_id, now_timestamp, record_a2a_settlement, revert_working_and_release, ERR_INTERNAL,
    ERR_PAYMENT_FAILED, ERR_PROVIDER_ERROR,
};
use crate::a2a::task_store::{self, TaskRecord};
use crate::a2a::types::*;
use crate::cache::ResponseCache;
use crate::channels::{ChannelDrawLockGuard, ChannelRepoError, CHANNEL_SLOT_MAX_STALENESS};
use crate::error::GatewayError;
use crate::providers::fallback::chat_with_model_fallback;
use crate::routes::chat::cost::{estimate_input_tokens, usdc_atomic_amount_checked};
use crate::AppState;

/// D3(a) versioned digest domain — the PERMANENT cross-repo client contract:
/// an A2A channel voucher signs `request_digest =
/// SHA-256("solvela-a2a-channel-digest-v1:" || task_id)` (UTF-8 bytes). The
/// task id is a 128-bit random capability minted at quote time, and the
/// served request is derived entirely from the record's `original_message`,
/// so binding the voucher to the task binds it to the request content
/// transitively. The versioned prefix keeps this preimage class disjoint from
/// the chat/search raw-body digests (and every future class) forever —
/// mirroring `solvela_x402::channel::CLOSE_DOMAIN_SEP`. Changing it is a
/// breaking wire change.
pub(crate) const A2A_CHANNEL_DIGEST_DOMAIN_V1: &str = "solvela-a2a-channel-digest-v1:";

/// The gateway-side D3 digest recomputation for a task id.
fn a2a_channel_request_digest(task_id: &str) -> [u8; 32] {
    crate::routes::channel::request_digest(
        format!("{A2A_CHANNEL_DIGEST_DOMAIN_V1}{task_id}").as_bytes(),
    )
}

/// Static, redacted `ERR_PAYMENT_FAILED` (§4 error table row 2).
fn payment_failed(message: &str) -> JsonRpcErrorData {
    JsonRpcErrorData {
        code: ERR_PAYMENT_FAILED,
        message: message.to_string(),
        data: None,
    }
}

/// Static, redacted `ERR_INTERNAL` — the codebase's retry-signal class
/// (§4 error table row 4; mirrors chat's 503 on the same arms).
fn internal_retryable(message: &str) -> JsonRpcErrorData {
    JsonRpcErrorData {
        code: ERR_INTERNAL,
        message: message.to_string(),
        data: None,
    }
}

/// `GatewayError → JsonRpcErrorData` adapter (§9 item 7): post-auth voucher
/// rejections carry the authoritative `last_cumulative` as a structured
/// STRING field — the SAME name and type as the REST resync surface
/// (`error.rs` / `map_voucher_rejection`), so SDK trackers share one
/// contract across both surfaces. Pre-auth rejections carry nothing (an
/// unauthenticated caller must never learn a channel's balance). Input comes
/// only from `map_voucher_rejection` (static, redacted messages).
fn voucher_rejection_to_jsonrpc(err: GatewayError) -> JsonRpcErrorData {
    match err {
        GatewayError::InvalidPaymentWithResync {
            message,
            last_cumulative,
        } => JsonRpcErrorData {
            code: ERR_PAYMENT_FAILED,
            message,
            data: Some(json!({ "last_cumulative": last_cumulative.to_string() })),
        },
        GatewayError::InvalidPayment(message) => JsonRpcErrorData {
            code: ERR_PAYMENT_FAILED,
            message,
            data: None,
        },
        other => {
            // `map_voucher_rejection` only produces the two arms above; keep
            // a fail-closed static fallback rather than echoing an unexpected
            // variant (GHSA-cgqx-mg48-949v posture).
            warn!(error = %other, "unexpected GatewayError variant on the voucher-rejection adapter");
            payment_failed("channel voucher rejected")
        }
    }
}

/// The task's stored quote in atomic micro-USDC — `cost_breakdown.total`
/// through the SAME checked conversion that produced the offer amounts at
/// intake (`usdc_atomic_amount_checked`, handler `handle_new_request`), so
/// the voucher delta, the amount echo, the persist advance, and the ledger
/// row all agree on ONE figure by construction. Fail-closed: a stored quote
/// that fails to parse/convert denies the draw (never a silent $0 — fintech
/// §4).
fn quoted_total_atomic(
    task_id: &str,
    payment_required_value: &Value,
) -> Result<u64, JsonRpcErrorData> {
    let pr: solvela_x402::types::PaymentRequired =
        serde_json::from_value(payment_required_value.clone()).map_err(|e| {
            error!(
                task_id,
                error = %e,
                "stored payment_required failed to parse — denying channel draw"
            );
            payment_failed("Internal: stored payment record is malformed")
        })?;
    usdc_atomic_amount_checked(&pr.cost_breakdown.total)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .ok_or_else(|| {
            error!(
                task_id,
                total = %pr.cost_breakdown.total,
                "stored quote failed the checked atomic conversion — denying channel draw"
            );
            payment_failed("Internal: stored payment amount is malformed")
        })
}

/// §5 step 6 money-free static checks (chat-parity, §4 / Rev-3 item 5): the
/// amount echo — `accepted.amount` must equal the quote EXACTLY; billing
/// never reads it (the gateway quote is authoritative), but a mismatch
/// signals an SDK↔gateway price disagreement and rejects loudly — plus the
/// three equality checks against CONFIG (the same values the stored snapshot
/// was built from; never snapshot-entry matching). Static messages only
/// (never reflect a client-controlled string).
fn validate_accepted_static(
    state: &AppState,
    accepted: &solvela_x402::types::PaymentAccept,
    billed: u64,
) -> Result<(), JsonRpcErrorData> {
    if accepted.amount != billed.to_string() {
        return Err(payment_failed(
            "channel voucher accepted.amount must equal the task's quoted amount; re-read the \
             quote from the input-required task metadata",
        ));
    }
    if !accepted
        .network
        .eq_ignore_ascii_case(solvela_x402::types::SOLANA_NETWORK)
    {
        return Err(payment_failed(
            "Payment network is unsupported. Use the network advertised in the payment-required \
             metadata.",
        ));
    }
    if accepted.asset != state.config.solana.usdc_mint {
        return Err(payment_failed(
            "Payment asset is unsupported. Use the asset advertised in the payment-required \
             metadata.",
        ));
    }
    if accepted.pay_to != state.config.solana.recipient_wallet {
        return Err(payment_failed(
            "Payment recipient does not match. Use the pay_to advertised in the payment-required \
             metadata.",
        ));
    }
    Ok(())
}

/// Outcome of the locked section — split so the DRAW lock (inner) provably
/// releases before the settle lock (outer) on every failure arm (§6
/// lock-ordering trace: "draw release then `revert_working_and_release`").
enum ChannelLegOutcome {
    /// Terminal `Completed` Task built (debited or deliver-free). The settle
    /// lock stays HELD — terminal semantics, identical to the exact leg.
    Done(Task),
    /// Pre-debit failure: after the draw guard releases, the caller reverts
    /// `Working → InputRequired` and releases the settle lock. No funds
    /// moved on any of these arms.
    RevertAndFail(JsonRpcErrorData),
}

/// The channel leg of `settle_paid_task` (§5 steps 6–15). Called from the
/// tx-extraction seam with the settle lock HELD and the `Working` marker
/// WRITTEN; owns everything from the static validations to the terminal Task.
// too_many_arguments: every argument is an owned/borrowed copy of
// already-validated `settle_paid_task` state; a params struct would be
// one-use ceremony (the `settle_paid_task` precedent).
#[allow(clippy::too_many_arguments)]
pub(super) async fn settle_channel_task(
    state: &Arc<AppState>,
    task_id: &str,
    record: &TaskRecord,
    payload: &solvela_x402::types::PaymentPayload,
    voucher_payload: &solvela_x402::types::ChannelVoucherPayload,
    resolved_model: &str,
    model_info: &ModelRegistration,
    messages: Vec<ChatMessage>,
    enforced_max_tokens: u32,
) -> Result<Value, JsonRpcErrorData> {
    // Defense-in-depth behind the step-1b intake kill switch (these AppState
    // fields are fixed at boot; config is process-static) — byte-identical
    // reject, never a lock-less draw.
    let (Some(pool), Some(cache)) = (state.db_pool.as_ref(), state.cache.as_ref()) else {
        revert_working_and_release(state, task_id).await;
        return Err(payment_failed(
            "channel scheme is not accepted on this endpoint",
        ));
    };

    // ── §5 step 6 — money-free static validations ───────────────────────────
    let billed = match quoted_total_atomic(task_id, &record.payment_required) {
        Ok(v) => v,
        Err(e) => {
            revert_working_and_release(state, task_id).await;
            return Err(e);
        }
    };
    if let Err(e) = validate_accepted_static(state, &payload.accepted, billed) {
        revert_working_and_release(state, task_id).await;
        return Err(e);
    }
    // Decode the wire voucher (incl. the BIGINT-storability guards that close
    // the verify-passes-where-persist-fails free-replay hole — the message is
    // the decoder's own static enumerated text, safe to forward).
    let voucher = match crate::routes::channel::voucher_from_payload(voucher_payload) {
        Ok(v) => v,
        Err(e) => {
            warn!(task_id, error = %e, "A2A channel draw: malformed voucher payload");
            revert_working_and_release(state, task_id).await;
            return Err(payment_failed(&e.to_string()));
        }
    };

    // §5 step 7 — NAMED BYPASS (§9 item 3): `check_and_record_tx` is NOT
    // called for a voucher — no tx signature exists. Replay is subsumed by
    // the D3 digest binding + monotonic cumulative + draw lock + DB CAS
    // (invariant 2), mirroring the chat draw exactly.

    // §5 step 8 — RPC-free cached slot, fail-closed past the shared staleness
    // bound: a stale (lower) slot would inflate the expiry buffer and could
    // let a genuinely-expired voucher pass. `None` → refuse retryable, BEFORE
    // the draw lock (test A-13).
    let Some(current_slot) =
        crate::routes::escrow::fetch_cached_slot_bounded(state, CHANNEL_SLOT_MAX_STALENESS).await
    else {
        counter!("solvela_a2a_channel_draw_total", "outcome" => "slot_unavailable").increment(1);
        warn!(
            task_id,
            "A2A channel draw: no fresh cached slot — failing closed"
        );
        revert_working_and_release(state, task_id).await;
        return Err(internal_retryable(
            "could not reach the Solana cluster to verify the voucher; please retry shortly",
        ));
    };

    // §5 step 9 — per-channel draw lock (INNER; settle lock OUTER — D2-a, the
    // only acquisition order any request ever takes; both surfaces key on the
    // same base58 channel_id, so chat and A2A draws on one channel serialize
    // on one key). Busy / Err → refuse retryable (§4 table), money-free.
    let channel_id = voucher_payload.channel_id.as_str();
    let lock_token = match cache.acquire_channel_draw_lock(channel_id).await {
        Ok(Some(token)) => token,
        Ok(None) => {
            counter!("solvela_a2a_channel_draw_total", "outcome" => "lock_busy").increment(1);
            revert_working_and_release(state, task_id).await;
            return Err(internal_retryable(
                "a draw is already in progress on this channel; please retry shortly",
            ));
        }
        Err(e) => {
            warn!(task_id, error = %e, "A2A channel draw: lock acquisition failed (Redis)");
            revert_working_and_release(state, task_id).await;
            return Err(internal_retryable(
                "payment service is temporarily degraded; please retry shortly",
            ));
        }
    };

    // Steps 10–15 run under the RAII guard (D7, hoisted into
    // `crate::channels`); the guard releases FIRST on every arm — awaited on
    // the normal path, detached token-guarded release on cancellation/panic —
    // then failure arms revert Working and release the settle lock (outer).
    let guard = ChannelDrawLockGuard::new(Arc::clone(state), channel_id.to_string(), lock_token);
    let outcome = channel_locked(
        state,
        task_id,
        record,
        payload,
        &voucher,
        channel_id,
        guard.token(),
        current_slot,
        billed,
        pool,
        cache,
        resolved_model,
        model_info,
        messages,
        enforced_max_tokens,
    )
    .await;
    guard.release().await;

    match outcome {
        ChannelLegOutcome::Done(task) => {
            serde_json::to_value(&task).map_err(|e| JsonRpcErrorData {
                code: ERR_PROVIDER_ERROR,
                message: format!("Failed to serialize task: {e}"),
                data: None,
            })
        }
        ChannelLegOutcome::RevertAndFail(err) => {
            revert_working_and_release(state, task_id).await;
            Err(err)
        }
    }
}

/// The draw-locked body (§5 steps 10–15): load channel + verify voucher →
/// #499 re-check → bounded SERVE → ownership recheck → single-transaction
/// persist (THE debit) → wallet-attributed ledger → Completed + D6 refs.
// too_many_arguments: see `settle_channel_task`.
#[allow(clippy::too_many_arguments)]
async fn channel_locked(
    state: &Arc<AppState>,
    task_id: &str,
    record: &TaskRecord,
    payload: &solvela_x402::types::PaymentPayload,
    voucher: &solvela_x402::channel::Voucher,
    channel_id: &str,
    lock_token: &str,
    current_slot: u64,
    billed: u64,
    pool: &sqlx::PgPool,
    cache: &ResponseCache,
    resolved_model: &str,
    model_info: &ModelRegistration,
    messages: Vec<ChatMessage>,
    enforced_max_tokens: u32,
) -> ChannelLegOutcome {
    use ChannelLegOutcome::{Done, RevertAndFail};

    // ── §5 step 10 — load the open channel bound to the D3 digest, verify ──
    let expected_digest = a2a_channel_request_digest(task_id);
    let drawable =
        match crate::channels::load_open_channel_state(pool, channel_id, expected_digest).await {
            Ok(Some(d)) => d,
            Ok(None) => {
                counter!("solvela_a2a_channel_draw_total", "outcome" => "rejected").increment(1);
                return RevertAndFail(payment_failed("channel not found or not open"));
            }
            Err(e) => {
                warn!(task_id, error = %e, "A2A channel draw: failed to load channel state");
                return RevertAndFail(internal_retryable(
                    "could not load the channel; please retry shortly",
                ));
            }
        };
    let agent_wallet = drawable.agent_wallet;
    let state_view = drawable.state;

    // `verify_voucher` is THE enforcement (7 fail-closed rules: sig vs
    // session_key, digest binding, expiry buffer, cumulative − last == billed
    // EXACTLY, monotonicity, settled floor, deposit ceiling). Rejections map
    // through the shared `map_voucher_rejection` → the §9-item-7 adapter, so
    // post-auth arms carry the `last_cumulative` resync and pre-auth arms
    // leak nothing. No money has moved.
    if let Err(e) =
        solvela_x402::channel::verify_voucher(&state_view, voucher, billed, current_slot)
    {
        counter!("solvela_a2a_channel_draw_total", "outcome" => "rejected").increment(1);
        return RevertAndFail(voucher_rejection_to_jsonrpc(
            crate::routes::channel::map_voucher_rejection(e, state_view.last_cumulative_atomic),
        ));
    }

    // ── §5 step 10b — the #499 AUTHORITATIVE tenant re-check (change 1) ─────
    //
    // The intake gate keys on `extract_payer_wallet`, which is the "unknown"
    // sentinel for a voucher — a provable no-op. The authoritative check
    // sources the wallet from the DB `agent_wallet` under both locks,
    // mirroring chat/search. Money-free: nothing served, nothing persisted.
    if state.usage.require_tenant_for_wallet(&agent_wallet).await {
        counter!("solvela_a2a_channel_draw_total", "outcome" => "rejected").increment(1);
        warn!(
            task_id,
            "A2A channel draw rejected: agent wallet requires per-tenant budgeting (#499)"
        );
        return RevertAndFail(payment_failed(
            "this wallet requires per-tenant budgeting; use POST /v1/chat/completions",
        ));
    }

    // ── §5 step 11 — SERVE, PRE-persist (deliver-then-record, invariant 3) ──
    //
    // The SAME `chat_with_model_fallback` invocation the exact leg makes (no
    // client HeaderMap reaches it), wrapped in the D6 A2A serve bound. The
    // server-side health-driven fallback chain inside it is bounded by the
    // timeout and money-safe regardless of which provider serves: billed ==
    // the stored quote (#504), never provider- or usage-dependent.
    let chat_req = ChatRequest {
        model: resolved_model.to_string(),
        messages,
        max_tokens: Some(enforced_max_tokens),
        temperature: None,
        top_p: None,
        stream: false,
        tools: None,
        tool_choice: None,
    };
    let estimated_input_tokens = estimate_input_tokens(&chat_req);
    let serve_timeout = Duration::from_secs(state.config.channel.a2a_draw_serve_timeout_secs());
    let result = match tokio::time::timeout(
        serve_timeout,
        chat_with_model_fallback(
            &state.providers,
            &state.provider_health,
            &state.model_registry,
            &model_info.provider,
            &model_info.model_id,
            chat_req,
        ),
    )
    .await
    {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            // D4-a: provider failure PRE-persist = total non-charge. The
            // voucher is unconsumed (its delta still equals billed) — the
            // client may retry the SAME voucher. This deliberately differs
            // from exact's post-settle charged-Failed arm, for this kind
            // only. GHSA-cgqx-mg48-949v: log the error, never echo it.
            counter!("solvela_a2a_channel_draw_total", "outcome" => "provider_failed").increment(1);
            warn!(
                task_id,
                model = %resolved_model,
                error = %e,
                "A2A channel draw: provider call failed pre-persist — total non-charge \
                 (voucher unconsumed)"
            );
            return RevertAndFail(JsonRpcErrorData {
                code: ERR_PROVIDER_ERROR,
                message: "Provider unavailable; your channel was NOT drawn. Retry with the same \
                          voucher shortly."
                    .to_string(),
                data: None,
            });
        }
        Err(_elapsed) => {
            // D6: same total-non-charge contract as a provider failure.
            counter!("solvela_a2a_channel_draw_total", "outcome" => "serve_timeout").increment(1);
            warn!(
                task_id,
                timeout_secs = serve_timeout.as_secs(),
                "A2A channel draw: serve exceeded the A2A draw timeout — total non-charge \
                 (voucher unconsumed)"
            );
            return RevertAndFail(JsonRpcErrorData {
                code: ERR_PROVIDER_ERROR,
                message: "No provider could serve your request in time and your channel was NOT \
                          drawn. Retry with the same voucher shortly."
                    .to_string(),
                data: None,
            });
        }
    };

    // Extract response text (mirrors the exact leg's money-adjacent warns).
    let response_text = match result.data.choices.first() {
        Some(choice) => {
            let text = choice.message.content.as_text().into_owned();
            if text.is_empty() {
                warn!(
                    task_id,
                    "A2A channel draw: provider returned empty content for a paid request"
                );
            }
            text
        }
        None => {
            warn!(
                task_id,
                "A2A channel draw: provider returned no choices for a paid request"
            );
            String::new()
        }
    };

    // ── §5 step 12 — pre-persist ownership recheck ──────────────────────────
    match cache.channel_draw_lock_held(channel_id, lock_token).await {
        Ok(true) => {}
        Ok(false) => {
            // TTL expired mid-serve: a successor draw could have been
            // admitted on the SAME stale snapshot. The DB CAS already stops a
            // double DEBIT; abort the persist and deliver free — notice-tier
            // `lock_lost` (never page-tier), same as the donor.
            counter!(
                "solvela_channel_draw_persist_failed_total",
                "reason" => "lock_lost"
            )
            .increment(1);
            warn!(
                task_id,
                channel_id,
                cumulative_atomic = voucher.cumulative_atomic,
                "A2A channel draw: draw lock no longer ours before persist (TTL expired \
                 mid-serve) — aborting persist, deliver free (deposit intact)"
            );
            return deliver_free_completion(state, task_id, record, response_text).await;
        }
        Err(e) => {
            // Donor-exact (change 11): an inconclusive recheck PROCEEDS — the
            // DB CAS is the money backstop; failing closed here would turn a
            // transient Redis blip into a guaranteed free serve.
            warn!(
                task_id,
                error = %e,
                channel_id,
                "A2A channel draw: could not re-check draw-lock ownership before persist; \
                 proceeding (DB CAS is the money backstop)"
            );
        }
    }

    // ── §5 step 13 — THE debit ──────────────────────────────────────────────
    //
    // ONE transaction: voucher INSERT + `last += billed` (CAS: monotonic AND
    // `status = 'open'`) + `realized += billed`. A2A is quote==cap==settlement
    // (#504) — no actual-usage re-bill exists, so `realized_advance = billed`
    // EXACTLY (the chat §6b clamp is degenerate here), never usage-derived.
    // Advancing `last` IS the debit; the ledger below is contingent on it.
    if let Err(e) = crate::channels::persist_voucher_and_advance(
        pool,
        &crate::channels::VoucherRecord {
            channel_id,
            cumulative_atomic: voucher.cumulative_atomic,
            call_cost_atomic: billed,
            realized_advance_atomic: billed,
            expiry_slot: voucher.expiry_slot,
            nonce: voucher.nonce,
            request_digest: &voucher.request_digest,
            signature: &voucher.signature,
        },
    )
    .await
    {
        // Deliver-free (D5): the serve is already earned; `last` did NOT
        // advance → NO debit → NO ledger row (positive receipt ⟺ `last`
        // advanced ⟺ agent debited). PR-0's shared classifier emits the tier
        // split by NAME: page-tier `solvela_channel_draw_persist_page_total`
        // on `db_error`/`overflow`/`unexpected` (already wired into the #684
        // cron), notice-tier on the benign `race` arm.
        let reason = crate::channels::emit_persist_failure_counters(&e);
        if matches!(e, ChannelRepoError::AdvanceNotApplied) {
            warn!(
                task_id,
                error = %e,
                channel_id,
                reason,
                cumulative_atomic = voucher.cumulative_atomic,
                last_cumulative = state_view.last_cumulative_atomic,
                "A2A channel draw: persist lost the invariant-11 close race — bounded one-call \
                 gateway loss, NO charge recorded (deposit intact; agent resyncs on next draw)"
            );
        } else {
            error!(
                task_id,
                error = %e,
                channel_id,
                reason,
                cumulative_atomic = voucher.cumulative_atomic,
                last_cumulative = state_view.last_cumulative_atomic,
                "A2A channel draw: persist FAILED after a successful serve (not a close race) — \
                 every channel draw is free-serving; investigate immediately (deposit intact, \
                 NO charge recorded)"
            );
        }
        return deliver_free_completion(state, task_id, record, response_text).await;
    }

    // ── Debited: §5 steps 14–15 ─────────────────────────────────────────────
    counter!("solvela_a2a_channel_draw_total", "outcome" => "settled").increment(1);

    // Step 14 — ledger + durable receipt, INITIATED before the state write
    // (M1 #680 order; its writes are spawned, so durability is eventual).
    // `payer_wallet_override = Some(agent_wallet)` — the DB channel wallet,
    // never `extract_payer_wallet`'s "unknown" sentinel (invariant 10).
    // `tx_signature = None` — a voucher has no on-chain tx; the
    // `channel_vouchers` row `(channel_id, cumulative)` is the durable audit
    // artifact. The recorded amount is the stored quote's total — the SAME
    // figure `billed` was derived from, so spend row == voucher delta by
    // construction; provider-reported usage is attribution-only.
    let receipt_path = record_a2a_settlement(
        state,
        task_id,
        payload,
        &record.payment_required,
        resolved_model,
        &model_info.provider,
        result.data.usage.as_ref(),
        estimated_input_tokens,
        &None,
        Some(&agent_wallet),
    );
    if receipt_path.is_none() {
        error!(
            task_id,
            "A2A channel draw: ledger/receipt write was skipped (debited-but-unledgered; \
             see solvela_a2a_settlement_record_skipped_total)"
        );
    }

    // Step 15 — `Working → Completed` AFTER the ledger initiation, then the
    // D6 recovery refs (artifact text + receipt path; both best-effort — the
    // full response is returned in-band below).
    if let Err(e) = task_store::update_task_state(state, task_id, TaskState::Completed).await {
        metrics::counter!("solvela_a2a_task_stuck_working_total").increment(1);
        error!(
            task_id,
            error = %e,
            "A2A channel draw: failed to mark task Completed after the debit — task stays \
             fail-safe stuck in Working until task TTL"
        );
    }
    if let Err(e) = task_store::persist_terminal_refs(
        state,
        task_id,
        Some(response_text.clone()),
        None,
        receipt_path.clone(),
    )
    .await
    {
        warn!(
            task_id,
            error = %e,
            "A2A channel draw: failed to persist completed-task artifact/receipt refs (D6)"
        );
    }

    // Receipts metadata for the DEBITED arm: `x402.payment.status` +
    // `x402.payment.receipts` with the receipt path ONLY — no `tx_signature`
    // entry exists for a voucher (verified-nullable end-to-end, plan change
    // 16). Key NAMES verbatim (§9 STOP: omission is sanctioned on the
    // deliver-free arms; renames/additions never).
    let mut meta = serde_json::Map::new();
    meta.insert(
        x402_meta::STATUS_KEY.to_string(),
        json!(x402_meta::PAYMENT_COMPLETED),
    );
    if let Some(path) = &receipt_path {
        let mut receipts_obj = serde_json::Map::new();
        receipts_obj.insert("receipt".to_string(), json!(path));
        meta.insert(
            x402_meta::RECEIPTS_KEY.to_string(),
            Value::Object(receipts_obj),
        );
    }

    info!(
        task_id,
        model = %resolved_model,
        channel_id,
        "A2A channel voucher settled → completed"
    );
    Done(build_channel_task(
        task_id,
        record,
        response_text,
        Some(meta),
    ))
}

/// The deliver-free terminal arm (D5-a): artifacts returned, task `Completed`
/// WITH the `deliver_free` marker set by the SAME terminal save (so every
/// later `tasks/get` projection stays truthful — Rev-3 item 2), NEITHER
/// `x402.payment.status` NOR `x402.payment.receipts` on the in-band response,
/// NO ledger row. The settle lock stays HELD (the task is terminal); the draw
/// guard is released by the caller.
async fn deliver_free_completion(
    state: &Arc<AppState>,
    task_id: &str,
    record: &TaskRecord,
    response_text: String,
) -> ChannelLegOutcome {
    counter!("solvela_a2a_channel_draw_total", "outcome" => "deliver_free").increment(1);
    if let Err(e) =
        task_store::complete_deliver_free(state, task_id, Some(response_text.clone())).await
    {
        // The record stays `Working` — fail-safe stuck (further payments
        // reject on the marker until task TTL); counted like the exact leg's
        // failed Completed write.
        metrics::counter!("solvela_a2a_task_stuck_working_total").increment(1);
        error!(
            task_id,
            error = %e,
            "A2A channel draw: failed to persist the deliver-free Completed marker"
        );
    }
    ChannelLegOutcome::Done(build_channel_task(task_id, record, response_text, None))
}

/// Build the terminal `Completed` wire Task for the channel leg. `metadata`
/// carries status/receipts on the debited arm and is `None` on deliver-free
/// arms — key OMISSION, truthful by silence (D5).
fn build_channel_task(
    task_id: &str,
    record: &TaskRecord,
    response_text: String,
    metadata: Option<serde_json::Map<String, Value>>,
) -> Task {
    Task {
        id: task_id.to_string(),
        context_id: record.context_id.clone(),
        kind: TaskKind::Task,
        status: TaskStatus {
            state: TaskState::Completed,
            message: Some(Message {
                role: MessageRole::Agent,
                parts: vec![Part::Text {
                    text: response_text.clone(),
                }],
                message_id: new_wire_id(),
                kind: MessageKind::Message,
                task_id: Some(task_id.to_string()),
                context_id: Some(record.context_id.clone()),
                metadata,
            }),
            timestamp: Some(now_timestamp()),
        },
        artifacts: Some(vec![Artifact {
            artifact_id: new_wire_id(),
            parts: vec![Part::Text {
                text: response_text,
            }],
        }]),
    }
}
