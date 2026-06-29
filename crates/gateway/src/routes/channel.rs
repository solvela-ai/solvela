//! v0 off-chain spend-down channel — management plane (`POST /v1/channel/{open,close}`).
//!
//! "Fund once, draw down." This module is the channel **management** surface;
//! the per-call draw (voucher verify + spend) is Pass B and is deliberately not
//! here. Two endpoints:
//!
//! - [`open`] — credit a channel from an **on-chain-verified** funding deposit.
//!   The client presents a signed USDC `TransferChecked` to the gateway
//!   recipient (the exact-scheme artifact); the gateway runs it through the
//!   existing facilitator/verifier (recipient-ATA + USDC-mint + amount checks),
//!   broadcasts+confirms it, and credits `deposited_atomic` = the
//!   **verifier-extracted** transfer amount — never a client-asserted value
//!   (mirrors the `cap_claim_amount` on-chain-verified ceiling rule,
//!   `routes/chat/payment.rs:101`). Re-presenting the same funding tx is
//!   rejected by Solana's tx dedup (the broadcast fails) and, belt-and-suspenders,
//!   by the unique `funding_tx_sig` ledger index (migration 016).
//!
//! - [`close`] — cooperative close. Computes `refundable = deposited - settled`
//!   (checked; never more than `deposited - settled`) and marks the channel
//!   `closing`. v0 is custodial: the on-chain disbursement of the refundable
//!   balance to the agent is performed out of band (the gateway holds the
//!   prepaid funds and has no in-tree custodial USDC-send primitive).
//!
//! **DB is REQUIRED** (CLAUDE.md #12): when no `db_pool` is configured the
//! channel scheme is simply unavailable — both endpoints return 404, never a
//! fake in-memory ledger.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;

use solvela_x402::types::{
    PayloadData, PaymentAccept, PaymentPayload, Resource, SolanaPayload, SOLANA_NETWORK,
};

use crate::channels::{self, ChannelStatus, NewChannel};
use crate::error::GatewayError;
use crate::middleware::rate_limit::{connect_info_client_id, rate_limited_response, PeerAddr};
use crate::AppState;

// ---------------------------------------------------------------------------
// Pure money-path helpers (no DB, no HTTP) — unit-tested below
// ---------------------------------------------------------------------------

/// Why a channel could not be opened.
#[derive(Debug, thiserror::Error)]
pub enum ChannelOpenError {
    /// The verifier did not extract a deposit amount (parse failure). Fail
    /// closed — never credit a client-asserted amount in its place (the exact
    /// trap `solvela-x402 §5a` warns about for the claim path).
    #[error("funding amount could not be verified on-chain")]
    UnverifiedAmount,
    /// The on-chain-verified transfer was zero — nothing to credit.
    #[error("verified funding amount is zero")]
    ZeroVerifiedAmount,
}

/// The credited deposit MUST be the on-chain-verified transfer amount. Returns
/// the amount only when the verifier produced a positive value; otherwise fails
/// closed. There is deliberately no `client_amount` parameter — a client cannot
/// assert its own deposit.
pub fn credit_deposit(verified_amount: Option<u64>) -> Result<u64, ChannelOpenError> {
    match verified_amount {
        Some(a) if a > 0 => Ok(a),
        Some(_) => Err(ChannelOpenError::ZeroVerifiedAmount),
        None => Err(ChannelOpenError::UnverifiedAmount),
    }
}

/// Why a channel could not be closed.
#[derive(Debug, thiserror::Error)]
pub enum ChannelCloseError {
    /// `settled > deposited` — a corrupt ledger row. Refuse rather than wrap the
    /// checked subtraction into a huge refund.
    #[error("settled {settled} exceeds deposited {deposited} (corrupt ledger row)")]
    SettledExceedsDeposited { deposited: u64, settled: u64 },
}

/// Cooperative-close refundable = `deposited - settled`, checked. Never returns
/// more than `deposited - settled`; an underflow (settled > deposited) is a
/// corrupt-state hard error, never a wrapped amount.
pub fn compute_refundable(
    deposited_atomic: u64,
    settled_atomic: u64,
) -> Result<u64, ChannelCloseError> {
    deposited_atomic
        .checked_sub(settled_atomic)
        .ok_or(ChannelCloseError::SettledExceedsDeposited {
            deposited: deposited_atomic,
            settled: settled_atomic,
        })
}

// ---------------------------------------------------------------------------
// POST /v1/channel/open
// ---------------------------------------------------------------------------

/// Request body for `POST /v1/channel/open`.
///
/// `#[serde(deny_unknown_fields)]`: a money-path request must reject unknown
/// fields. In particular there is NO `amount`/`deposit` field — the credited
/// deposit comes only from the on-chain-verified transfer, so a client cannot
/// assert how much it funded.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenChannelRequest {
    /// base58 depositor wallet.
    pub agent_wallet: String,
    /// base58 ed25519 voucher-signing pubkey. Absent → defaults to
    /// `agent_wallet` (the agent signs its own vouchers).
    #[serde(default)]
    pub session_key: Option<String>,
    /// Base64 signed USDC `TransferChecked` transaction paying the gateway
    /// recipient (the exact-scheme funding artifact). Verified + broadcast by
    /// the facilitator before the deposit is credited.
    pub funding_tx: String,
    /// Optional channel/voucher validity horizon (Solana slot).
    #[serde(default)]
    pub expiry_slot: Option<u64>,
}

/// Response body for `POST /v1/channel/open`.
#[derive(Debug, Clone, Serialize)]
pub struct OpenChannelResponse {
    /// base58 32-byte channel id — also the 32-byte id the agent's vouchers sign.
    pub channel_id: String,
    pub agent_wallet: String,
    pub session_key: String,
    /// On-chain-verified credited principal (atomic micro-USDC).
    pub deposited_atomic: u64,
    pub settled_atomic: u64,
    pub last_cumulative_atomic: u64,
    pub status: String,
    /// The settled funding-transfer signature.
    pub funding_tx_sig: Option<String>,
    pub network: String,
}

/// POST /v1/channel/open
///
/// Status codes:
/// - 200 with the opened channel state.
/// - 400 on a malformed `agent_wallet` / `session_key` / `funding_tx`.
/// - 402 when the funding transfer fails on-chain verification (wrong
///   recipient/mint, insufficient, or it would not confirm).
/// - 404 `{"error":"channel not available"}` when no DB is configured.
/// - 409 when the funding tx already opened a channel (replay).
/// - 429 when the per-IP rate limit is exceeded.
/// - 503 (fail-closed) when the Solana cluster is unreachable.
pub async fn open(
    State(state): State<Arc<AppState>>,
    peer_addr: PeerAddr,
    Json(req): Json<OpenChannelRequest>,
) -> axum::response::Response {
    // Per-IP cap BEFORE any RPC work (this route fans out to Solana RPC and is
    // unauthenticated). Reuses the deposit-tx limiter — same purpose: bound RPC
    // amplification on an unauthenticated money endpoint. Keyed on the TCP peer
    // IP, never a forgeable header (GHSA-6ggq-cvwx-4f67).
    let client_id = connect_info_client_id(peer_addr.0);
    if state
        .deposit_tx_rate_limiter
        .check(&client_id)
        .await
        .is_err()
    {
        metrics::counter!("solvela_channel_open_rate_limited_total").increment(1);
        tracing::warn!(client_id = %client_id, "channel open rate limit exceeded");
        return rate_limited_response(state.deposit_tx_rate_limiter.config());
    }

    // Channels are DB-backed; with no pool the scheme is unavailable. 404 BEFORE
    // any RPC — never fabricate an in-memory ledger (CLAUDE.md #12).
    let Some(pool) = state.db_pool.clone() else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "channel not available" })),
        )
            .into_response();
    };

    // --- Validate client inputs (no network I/O on bad input) ---
    if solvela_x402::escrow::pda::decode_bs58_pubkey(&req.agent_wallet).is_err() {
        return GatewayError::BadRequest(
            "agent_wallet must be a base58-encoded 32-byte pubkey".to_string(),
        )
        .into_response();
    }
    let session_key = req
        .session_key
        .clone()
        .unwrap_or_else(|| req.agent_wallet.clone());
    if solvela_x402::escrow::pda::decode_bs58_pubkey(&session_key).is_err() {
        return GatewayError::BadRequest(
            "session_key must be a base58-encoded 32-byte ed25519 key".to_string(),
        )
        .into_response();
    }
    if req.funding_tx.is_empty() {
        return GatewayError::BadRequest("funding_tx must be present".to_string()).into_response();
    }

    let usdc_mint = state.config.solana.usdc_mint.clone();
    let recipient = state.config.solana.recipient_wallet.clone();

    // Build the exact-scheme payload around the client's signed funding transfer.
    // The verifier enforces destination == recipient ATA, mint == USDC, and
    // amount >= the floor; `amount = "1"` is a non-zero floor only — the CREDITED
    // deposit is the verifier-extracted `verified_amount`, not this field.
    let payload = PaymentPayload {
        x402_version: 2,
        resource: Resource {
            url: "/v1/channel/open".to_string(),
            method: "POST".to_string(),
        },
        accepted: PaymentAccept {
            scheme: "exact".to_string(),
            network: SOLANA_NETWORK.to_string(),
            amount: "1".to_string(),
            asset: usdc_mint.clone(),
            pay_to: recipient.clone(),
            max_timeout_seconds: 300,
            escrow_program_id: None,
        },
        payload: PayloadData::Direct(SolanaPayload {
            transaction: req.funding_tx.clone(),
        }),
    };

    // 1. Verify on-chain (non-mutating: signature, recipient ATA, USDC mint,
    //    amount, simulation). This is the on-chain-verified amount source.
    let verification = match state.facilitator.verify(&payload).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "channel open: funding verification errored");
            return service_unavailable("could not verify the funding transaction on-chain");
        }
    };
    if !verification.valid {
        return payment_required("funding transaction failed verification");
    }
    // Credit ONLY the on-chain-verified amount; fail closed if absent/zero.
    let deposited_atomic = match credit_deposit(verification.verified_amount) {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(error = %e, "channel open: no usable on-chain funding amount");
            return payment_required("funding transaction has no verifiable amount");
        }
    };

    // 2. Settle (broadcast + confirm) the funding transfer so the money lands
    //    before we credit. A re-presented funding tx fails here (Solana dedup) →
    //    no channel created. Don't create a channel for funds that didn't land.
    let settlement = match state.facilitator.settle(&payload).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "channel open: funding settlement errored");
            return service_unavailable("could not settle the funding transaction on-chain");
        }
    };
    if !settlement.success {
        tracing::warn!(
            error = ?settlement.error,
            "channel open: funding settlement did not confirm"
        );
        return payment_required("funding transaction did not confirm on-chain");
    }

    // 3. Create the channel ledger row. AWAITED (not fire-and-forget): this write
    //    IS the operation — the funds have landed, so losing the row would strand
    //    them. The unique funding_tx_sig index rejects a double-credit replay.
    let channel_id = fresh_channel_id();
    let new_channel = NewChannel {
        channel_id: channel_id.clone(),
        agent_wallet: req.agent_wallet.clone(),
        session_key: session_key.clone(),
        provider: recipient,
        mint: usdc_mint,
        deposited_atomic,
        expiry_slot: req.expiry_slot,
        funding_tx_sig: settlement.tx_signature.clone(),
    };
    match channels::create_channel(&pool, &new_channel).await {
        Ok(()) => {}
        Err(channels::ChannelRepoError::FundingAlreadyUsed) => {
            return (
                StatusCode::CONFLICT,
                Json(json!({ "error": "funding transaction already used to open a channel" })),
            )
                .into_response();
        }
        Err(e) => {
            // The funding landed but the ledger write failed. Surface the funding
            // signature so the deposit can be reconciled (idempotent re-open keys
            // on the same funding tx). Never silently drop it.
            tracing::error!(
                error = %e,
                funding_tx_sig = ?settlement.tx_signature,
                deposited_atomic,
                "channel open: funding settled but ledger write failed"
            );
            return internal_error("funding confirmed but the channel could not be recorded");
        }
    }

    let response = OpenChannelResponse {
        channel_id,
        agent_wallet: req.agent_wallet,
        session_key,
        deposited_atomic,
        settled_atomic: 0,
        last_cumulative_atomic: 0,
        status: ChannelStatus::Open.as_str().to_string(),
        funding_tx_sig: settlement.tx_signature,
        network: SOLANA_NETWORK.to_string(),
    };
    (StatusCode::OK, Json(json!(response))).into_response()
}

// ---------------------------------------------------------------------------
// POST /v1/channel/close
// ---------------------------------------------------------------------------

/// Request body for `POST /v1/channel/close`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CloseChannelRequest {
    /// The unguessable base58 32-byte channel id (the v0 close capability).
    pub channel_id: String,
}

/// Response body for `POST /v1/channel/close`.
#[derive(Debug, Clone, Serialize)]
pub struct CloseChannelResponse {
    pub channel_id: String,
    pub deposited_atomic: u64,
    pub settled_atomic: u64,
    /// `deposited - settled`, the amount owed back to the agent. v0: disbursed
    /// out of band (custodial).
    pub refundable_atomic: u64,
    pub status: String,
}

/// POST /v1/channel/close
///
/// Cooperative close. Computes `refundable = deposited - settled` (never more)
/// and transitions an open channel to `closing` (refund pending). Idempotent on
/// an already-closing/closed channel.
///
/// Status codes:
/// - 200 with the refundable amount + status.
/// - 404 `{"error":"channel not available"}` when no DB is configured, or
///   `{"error":"channel not found"}` for an unknown channel id.
/// - 429 when the per-IP rate limit is exceeded.
/// - 500 (fail-closed) on a corrupt ledger row (`settled > deposited`).
pub async fn close(
    State(state): State<Arc<AppState>>,
    peer_addr: PeerAddr,
    Json(req): Json<CloseChannelRequest>,
) -> axum::response::Response {
    let client_id = connect_info_client_id(peer_addr.0);
    if state
        .deposit_tx_rate_limiter
        .check(&client_id)
        .await
        .is_err()
    {
        metrics::counter!("solvela_channel_close_rate_limited_total").increment(1);
        tracing::warn!(client_id = %client_id, "channel close rate limit exceeded");
        return rate_limited_response(state.deposit_tx_rate_limiter.config());
    }

    let Some(pool) = state.db_pool.clone() else {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "channel not available" })),
        )
            .into_response();
    };

    let row = match channels::load_channel(&pool, &req.channel_id).await {
        Ok(Some(r)) => r,
        Ok(None) => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "channel not found" })),
            )
                .into_response();
        }
        Err(e) => {
            tracing::error!(error = %e, "channel close: failed to load channel");
            return internal_error("could not load the channel");
        }
    };

    // refundable = deposited - settled, checked. A corrupt row (settled >
    // deposited) fails closed rather than wrapping into a huge refund.
    let refundable_atomic = match compute_refundable(row.deposited_atomic, row.settled_atomic) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, channel_id = %req.channel_id, "channel close: corrupt ledger row");
            return internal_error("channel ledger state is inconsistent");
        }
    };

    // Mark an open channel `closing` (refund pending). Already-closing/closed
    // channels are returned as-is (idempotent). v0: the on-chain custodial
    // disbursement of `refundable_atomic` to the agent happens out of band.
    let status = if row.status == ChannelStatus::Open.as_str() {
        if let Err(e) = channels::set_status(&pool, &req.channel_id, ChannelStatus::Closing).await {
            tracing::error!(error = %e, "channel close: failed to set closing status");
            return internal_error("could not mark the channel closing");
        }
        ChannelStatus::Closing.as_str().to_string()
    } else {
        row.status
    };

    let response = CloseChannelResponse {
        channel_id: req.channel_id,
        deposited_atomic: row.deposited_atomic,
        settled_atomic: row.settled_atomic,
        refundable_atomic,
        status,
    };
    (StatusCode::OK, Json(json!(response))).into_response()
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A fresh base58 32-byte channel id. v0 generates 32 random bytes (two v4
/// UUIDs) so the id (a) unifies with the end-state Channel-PDA base58 and (b)
/// gives the voucher a clean 32-byte `channel_id` — a 16-byte UUID could not
/// fill the voucher's 32-byte field. Unguessable (256-bit), so it doubles as the
/// v0 close capability.
fn fresh_channel_id() -> String {
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    bs58::encode(bytes).into_string()
}

/// 402 Payment Required with a fixed, safe message (never reflect RPC internals).
fn payment_required(message: &str) -> axum::response::Response {
    (
        StatusCode::PAYMENT_REQUIRED,
        Json(json!({ "error": message })),
    )
        .into_response()
}

/// 503 Service Unavailable (RPC unreachable), static message free of internals.
fn service_unavailable(message: &str) -> axum::response::Response {
    GatewayError::ServiceUnavailable(message.to_string()).into_response()
}

/// 500 for a gateway-side fault; the inner message is discarded client-side.
fn internal_error(message: &str) -> axum::response::Response {
    GatewayError::Internal(message.to_string()).into_response()
}

// ---------------------------------------------------------------------------
// Tests — pure money-path helpers (no DB, no HTTP)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credit_deposit_uses_verified_amount() {
        assert_eq!(credit_deposit(Some(2_625)).unwrap(), 2_625);
        assert_eq!(credit_deposit(Some(1)).unwrap(), 1);
    }

    #[test]
    fn credit_deposit_fails_closed_when_unverified() {
        // The verifier could not extract an amount → refuse, never credit a
        // fallback. This is the trap solvela-x402 §5a warns about.
        assert!(matches!(
            credit_deposit(None),
            Err(ChannelOpenError::UnverifiedAmount)
        ));
    }

    #[test]
    fn credit_deposit_rejects_zero() {
        assert!(matches!(
            credit_deposit(Some(0)),
            Err(ChannelOpenError::ZeroVerifiedAmount)
        ));
    }

    #[test]
    fn refundable_is_deposited_minus_settled() {
        assert_eq!(compute_refundable(50_000, 12_600).unwrap(), 37_400);
        // Nothing settled → full deposit refundable.
        assert_eq!(compute_refundable(50_000, 0).unwrap(), 50_000);
        // Fully settled → zero refundable, never negative.
        assert_eq!(compute_refundable(50_000, 50_000).unwrap(), 0);
    }

    #[test]
    fn refundable_never_exceeds_deposited_minus_settled() {
        // Exhaustive small sweep: the result is always exactly deposited-settled
        // and never more (no path can over-refund).
        for deposited in 0u64..200 {
            for settled in 0u64..=deposited {
                assert_eq!(
                    compute_refundable(deposited, settled).unwrap(),
                    deposited - settled
                );
            }
        }
    }

    #[test]
    fn refundable_fails_closed_when_settled_exceeds_deposited() {
        // A corrupt row must NOT wrap into a huge refund.
        assert!(matches!(
            compute_refundable(100, 101),
            Err(ChannelCloseError::SettledExceedsDeposited {
                deposited: 100,
                settled: 101
            })
        ));
        assert!(compute_refundable(0, u64::MAX).is_err());
    }

    #[test]
    fn open_request_rejects_client_asserted_amount() {
        // deny_unknown_fields means a client cannot smuggle an `amount`/`deposit`
        // field — the credited deposit can only come from the on-chain verifier.
        let with_amount = r#"{"agent_wallet":"a","funding_tx":"tx","amount":"999999"}"#;
        assert!(serde_json::from_str::<OpenChannelRequest>(with_amount).is_err());

        let ok = r#"{"agent_wallet":"a","funding_tx":"tx"}"#;
        let parsed: OpenChannelRequest = serde_json::from_str(ok).unwrap();
        assert!(parsed.session_key.is_none());
    }
}
