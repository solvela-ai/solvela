//! POST /v1/escrow/settle — F4 escrow settle endpoint.
//!
//! Called by client-side F4 wrappers (e.g. `@solvela/openclaw-provider`'s
//! `wrapStreamForF4`) after a stream completes, to trigger the escrow
//! claim against the actual token usage rather than waiting for the
//! gateway's timeout-based auto-claim. This brings reconciliation latency
//! from minutes to milliseconds when both halves of F4 are deployed.
//!
//! Wire contract:
//!   {
//!     "service_id":           "<gateway-defined encoding, 32 bytes>",
//!     "agent_pubkey":         "<base58 Solana pubkey>",
//!     "model":                "<model id, e.g. gpt-4o>",
//!     "status":               "completed" | "error",
//!     "actual_prompt_tokens":     <u32, required when status=completed>,
//!     "actual_completion_tokens": <u32, required when status=completed>
//!   }
//!
//! Behavior:
//!   - status=completed + tokens present → compute actual cost from the
//!     model registry, fire escrow claim through the durable queue
//!     (or fire-and-forget if no DB).
//!   - status=error → no claim fired. The deposit is reconciled by the
//!     escrow program's on-chain auto-timeout refund path (same as the
//!     pre-F4 flow).
//!   - status=completed without tokens → 400.
//!   - unknown model → 400.
//!   - cost overflow / non-finite pricing → 400.
//!
//! Auth model (v1):
//!   The on-chain escrow program enforces that claims only succeed when
//!   the PDA seeds `[b"escrow", agent, service_id]` match the deposit.
//!   A misrouted settle (wrong agent_pubkey or service_id) fails at the
//!   chain layer, logged but not client-visible. This is intentional for
//!   v1 — settle is fire-and-forget and the chain is the source of
//!   truth. Future hardening (see TODO comments): HMAC over the request
//!   body, or an agent signature over (service_id, model, status,
//!   tokens). Rate limiting is handled by the existing gateway middleware.
//!
//! Idempotency:
//!   `fire_escrow_claim` enqueues the claim via the durable
//!   `escrow_claim_queue` (when DB is available), which dedupes by
//!   service_id + agent. Without a DB, duplicate settle requests can
//!   produce duplicate on-chain claim attempts — the second is rejected
//!   by the escrow program (deposit account already drained). Wasteful
//!   but correct.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::routes::chat::cost::compute_actual_atomic_cost;
use crate::routes::chat::payment::fire_escrow_claim;
use crate::AppState;

/// Request body for `POST /v1/escrow/settle`.
#[derive(Debug, Clone, Deserialize)]
pub struct SettleRequest {
    pub service_id: String,
    pub agent_pubkey: String,
    pub model: String,
    pub status: SettleStatus,
    #[serde(default)]
    pub actual_prompt_tokens: Option<u32>,
    #[serde(default)]
    pub actual_completion_tokens: Option<u32>,
}

/// Stream-completion status reported by the client F4 wrapper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SettleStatus {
    Completed,
    Error,
}

/// Response body for `POST /v1/escrow/settle`.
#[derive(Debug, Clone, Serialize)]
pub struct SettleResponse {
    pub ok: bool,
    /// Atomic micro-USDC claim amount fired. Present when status=completed
    /// and the cost was computable; absent on error-status or skip.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claim_amount: Option<u64>,
}

/// POST /v1/escrow/settle
///
/// See module docs for wire contract, behavior, auth model, and idempotency.
pub async fn handle_settle(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SettleRequest>,
) -> Result<Json<SettleResponse>, (StatusCode, Json<serde_json::Value>)> {
    // status=error: stream failed mid-response. Don't fire a claim — the
    // on-chain refund path will reconcile the deposit via the auto-timeout.
    if req.status == SettleStatus::Error {
        tracing::info!(
            service_id = %req.service_id,
            agent = %req.agent_pubkey,
            model = %req.model,
            "settle: stream finished with error — no claim fired (auto-timeout will refund)"
        );
        return Ok(Json(SettleResponse {
            ok: true,
            claim_amount: None,
        }));
    }

    // status=completed: tokens are required for cost computation.
    let (prompt_tokens, completion_tokens) = match (
        req.actual_prompt_tokens,
        req.actual_completion_tokens,
    ) {
        (Some(p), Some(c)) => (p, c),
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "actual_prompt_tokens and actual_completion_tokens are required when status=completed"
                })),
            ));
        }
    };

    // Look up model pricing.
    let model_info = match state.model_registry.get(&req.model) {
        Some(info) => info,
        None => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("unknown model: {}", req.model) })),
            ));
        }
    };

    // Compute the actual cost. Returns None on non-finite/negative pricing
    // or u64 overflow — both indicate a misconfigured model entry.
    let claim_atomic =
        match compute_actual_atomic_cost(prompt_tokens, completion_tokens, model_info) {
            Some(c) => c,
            None => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "error": "could not compute cost (overflow or non-finite pricing)"
                    })),
                ));
            }
        };

    // claim_atomic is the actual computed cost. We don't have a separate
    // deposit-amount upper bound here — the original 402's amount isn't
    // threaded into settle (the wire contract stays minimal). Pass
    // claim_atomic as both `claim_atomic` and `client_amount` so
    // cap_claim_amount becomes a no-op. The escrow program enforces the
    // "don't claim more than deposited" invariant on-chain.
    //
    // TODO(hardening): accept an optional `deposit_amount` field in the
    // request body so cap_claim_amount can enforce a tighter pre-chain
    // bound. Today's flow leans on the chain layer to reject over-claims.
    fire_escrow_claim(
        &state,
        "escrow",
        &Some(req.service_id.clone()),
        &Some(req.agent_pubkey.clone()),
        None, // deposit amount unknown at this layer
        claim_atomic,
        claim_atomic, // no-op cap (see comment above)
    );

    tracing::info!(
        service_id = %req.service_id,
        agent = %req.agent_pubkey,
        model = %req.model,
        prompt_tokens,
        completion_tokens,
        claim_atomic,
        "settle: fired escrow claim with actual usage"
    );

    Ok(Json(SettleResponse {
        ok: true,
        claim_amount: Some(claim_atomic),
    }))
}
