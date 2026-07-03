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
//! - [`close`] — cooperative close. Requires an ed25519 `session_key` signature
//!   over the canonical close message (a leaked `channel_id` alone must not
//!   force a close). Acquires the per-channel draw lock, then in ONE
//!   transaction flips the channel `closing`, computes `refundable =
//!   deposited - realized` (checked; the synchronous per-draw actual-cost
//!   counter — NOT `settled`, which lags and would over-refund, and NOT
//!   `last`, which would strand the quote-vs-actual gap), and FREEZES the
//!   refund reservation. The background worker in
//!   [`crate::channel_refunds`] disburses USDC from the frozen tuple;
//!   re-POSTing the idempotent close polls the refund status.
//!
//! **DB is REQUIRED** (CLAUDE.md #12) and the scheme ships **DISABLED by
//! default** (`[channel] enabled = false`): when channels are disabled OR no
//! `db_pool` is configured, both endpoints return 404 (`channel not
//! available`), never a fake in-memory ledger.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use base64::Engine;
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

// NOTE: `compute_refundable` moved to `crate::channel_refunds` (PR-A) and now
// computes `deposited - realized` against the synchronous `realized` counter —
// the disbursement-grade formula (never `- settled` = A1 over-refund, never
// `- last` = #600 strand). The close handler below delegates the whole
// flip→read→reserve to [`crate::channel_refunds::close_channel_and_reserve_refund`].

/// Per-channel deposit cap (config `channel.max_deposit_atomic`): `true` when
/// the on-chain-verified deposit is acceptable. `None` = uncapped. Checked
/// AFTER verification but BEFORE the funding broadcast, so a rejected deposit
/// moves no money.
pub fn deposit_within_cap(verified_amount: u64, cap: Option<u64>) -> bool {
    match cap {
        Some(cap) => verified_amount <= cap,
        None => true,
    }
}

/// Why a close request could not be authorized. Distinguishes a malformed client
/// credential (400), a corrupt stored key (500), and a credential that simply does
/// not match the channel's `session_key` (401) — status separation per scope §3.
#[derive(Debug, thiserror::Error)]
pub enum CloseAuthError {
    /// The presented `signature` is not base64 or not exactly 64 bytes. The client
    /// sent a malformed credential → 400.
    #[error("signature must be base64-encoded 64 bytes")]
    MalformedSignature,
    /// The stored `channel_id`/`session_key` is not a base58 32-byte value, or the
    /// `session_key` bytes are not a valid ed25519 point — corrupt ledger row → 500
    /// (a gateway-side fault, never the caller's). `which` names the field.
    #[error("stored {which} is corrupt (not a usable key)")]
    CorruptStoredKey { which: &'static str },
    /// The signature did not verify against the channel's `session_key` over the
    /// canonical close message — a bad credential (or a force-close attempt with a
    /// leaked `channel_id`) → 401.
    #[error("close signature does not match the channel session key")]
    BadSignature,
}

/// Decode the base64 close `signature` to exactly 64 bytes, fail-closed.
fn decode_close_signature(signature_b64: &str) -> Result<[u8; 64], CloseAuthError> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(signature_b64)
        .map_err(|_| CloseAuthError::MalformedSignature)?;
    bytes
        .try_into()
        .map_err(|_| CloseAuthError::MalformedSignature)
}

/// Authorize a cooperative close: the client's `signature_b64` must be a valid
/// ed25519 signature by the channel's `session_key` over the canonical close
/// message ([`solvela_x402::channel::build_close_message`]). This upgrades the v0
/// close from "present the 256-bit `channel_id` capability" (a leaked id is a
/// force-close DoS) to "prove control of the channel's `session_key`".
///
/// There is deliberately NO refund-destination parameter — the refund goes to the
/// DB-stored `agent_wallet`, never anything the caller supplies, so replaying a
/// valid close signature is a harmless idempotent no-op (no nonce needed).
///
/// `channel_id_b58`/`session_key_b58` are the **stored** (already-validated)
/// values; a decode failure here is corrupt gateway state, not client error.
pub fn authorize_close(
    session_key_b58: &str,
    channel_id_b58: &str,
    signature_b64: &str,
) -> Result<(), CloseAuthError> {
    let signature = decode_close_signature(signature_b64)?;
    let channel_id =
        solvela_x402::escrow::pda::decode_bs58_pubkey(channel_id_b58).map_err(|_| {
            CloseAuthError::CorruptStoredKey {
                which: "channel_id",
            }
        })?;
    let session_key =
        solvela_x402::escrow::pda::decode_bs58_pubkey(session_key_b58).map_err(|_| {
            CloseAuthError::CorruptStoredKey {
                which: "session_key",
            }
        })?;
    match solvela_x402::channel::verify_close(&session_key, &channel_id, &signature) {
        Ok(()) => Ok(()),
        Err(solvela_x402::channel::ChannelCloseSigError::InvalidSessionKey) => {
            Err(CloseAuthError::CorruptStoredKey {
                which: "session_key",
            })
        }
        Err(solvela_x402::channel::ChannelCloseSigError::InvalidSignature) => {
            Err(CloseAuthError::BadSignature)
        }
    }
}

// ---------------------------------------------------------------------------
// v0 spend-down channel DRAW helpers (Pass B) — pure, no DB/HTTP
// ---------------------------------------------------------------------------

/// The canonical `request_digest` bound into a channel voucher:
/// **`SHA-256(raw request body bytes, exactly as received)`**.
///
/// The SDK signs what it *sends* — the raw bytes — so the raw bytes are the one
/// preimage both parties reproduce. This deliberately does NOT reuse the
/// wallet-agnostic response-cache normalization (`cache/exact.rs`), which merges
/// byte-different-but-semantically-equal requests: for a voucher that would be
/// wrong (the voucher must bind to THE specific request, not a class of them).
/// Golden-vector-pinned below so a one-byte SDK↔gateway disagreement is caught
/// in test, not by fail-closing every draw. See channel scope §4.3.
pub fn request_digest(body: &[u8]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(body);
    hasher.finalize().into()
}

/// Why a channel voucher payload could not be decoded into a verifiable
/// [`solvela_x402::channel::Voucher`]. Every variant is a hard, fail-closed
/// client error (400) — a malformed credential is never silently coerced.
#[derive(Debug, thiserror::Error)]
pub enum ChannelDrawError {
    /// `channel_id` is not a base58-encoded 32-byte value.
    #[error("voucher channel_id must be a base58-encoded 32-byte value")]
    MalformedChannelId,
    /// `request_digest` is not base64 of exactly 32 bytes.
    #[error("voucher request_digest must be base64 of exactly 32 bytes")]
    MalformedRequestDigest,
    /// `signature` is not base64 of exactly 64 bytes.
    #[error("voucher signature must be base64 of exactly 64 bytes")]
    MalformedSignature,
    /// `nonce` exceeds `i64::MAX` — unstorable in the BIGINT ledger.
    #[error("voucher nonce exceeds the storable range")]
    MalformedNonce,
    /// `expiry_slot` exceeds `i64::MAX` — unstorable in the BIGINT ledger.
    #[error("voucher expiry_slot exceeds the storable range")]
    MalformedExpirySlot,
}

/// Decode a wire [`solvela_x402::types::ChannelVoucherPayload`] into the frozen
/// [`solvela_x402::channel::Voucher`] the verifier consumes, fail-closed on any
/// malformed field.
///
/// The base58 `channel_id` and base64 `request_digest`/`signature` are decoded
/// to their fixed-width byte arrays (wrong length ⇒ hard error, never truncated
/// or zero-padded). The bare `u64` scalars `nonce`/`expiry_slot` are
/// **range-guarded to `i64::MAX`** here — the CRITICAL guard: `verify_voucher`
/// does NOT range-check `nonce`, and `expiry_slot = u64::MAX` passes its expiry
/// rule, so an unchecked value would VERIFY and SERVE, then fail
/// `persist_voucher_and_advance`'s `i64` (BIGINT) conversion → `last` never
/// advances → the SAME voucher (`cumulative = 0 + billed`) replays free forever.
/// Rejecting here (before the lock and before any serve) closes that
/// unbounded-free-draw hole. (`cumulative_atomic` needs no guard — `verify_voucher`
/// rule 7 caps it at `deposited`, itself a stored `i64`.)
///
/// This performs NO signature verification — the caller runs
/// [`solvela_x402::channel::verify_voucher`], which authenticates the signature
/// before any signed field is trusted.
pub fn voucher_from_payload(
    v: &solvela_x402::types::ChannelVoucherPayload,
) -> Result<solvela_x402::channel::Voucher, ChannelDrawError> {
    let channel_id = solvela_x402::escrow::pda::decode_bs58_pubkey(&v.channel_id)
        .map_err(|_| ChannelDrawError::MalformedChannelId)?;

    // BIGINT-storability guards (see the fn doc): reject before any serve so a
    // voucher that could never persist can never be served-then-replayed.
    if i64::try_from(v.nonce).is_err() {
        return Err(ChannelDrawError::MalformedNonce);
    }
    if i64::try_from(v.expiry_slot).is_err() {
        return Err(ChannelDrawError::MalformedExpirySlot);
    }

    let digest_bytes = base64::engine::general_purpose::STANDARD
        .decode(&v.request_digest)
        .map_err(|_| ChannelDrawError::MalformedRequestDigest)?;
    let request_digest: [u8; 32] = digest_bytes
        .try_into()
        .map_err(|_| ChannelDrawError::MalformedRequestDigest)?;

    let sig_bytes = base64::engine::general_purpose::STANDARD
        .decode(&v.signature)
        .map_err(|_| ChannelDrawError::MalformedSignature)?;
    let signature: [u8; 64] = sig_bytes
        .try_into()
        .map_err(|_| ChannelDrawError::MalformedSignature)?;

    Ok(solvela_x402::channel::Voucher {
        channel_id,
        cumulative_atomic: v.cumulative_atomic,
        expiry_slot: v.expiry_slot,
        nonce: v.nonce,
        request_digest,
        signature,
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
    // Gate 1 (cheapest-first, mirrors the faucet's enabled-first check): channels
    // ship DISABLED by default. When the flag is off the whole scheme is
    // unavailable — 404 before the rate limiter or any RPC, no abuse surface to
    // protect. Prod must accept no deposit it cannot yet programmatically refund.
    if !state.config.channel.enabled {
        return channel_not_available();
    }

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
        return channel_not_available();
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

    // Per-channel deposit cap (config, PR-A) — checked BEFORE the funding
    // broadcast so a rejected deposit moves no money. The cap value is public
    // policy, safe to state.
    if !deposit_within_cap(deposited_atomic, state.config.channel.max_deposit_atomic) {
        tracing::warn!(
            deposited_atomic,
            cap = ?state.config.channel.max_deposit_atomic,
            "channel open: deposit exceeds the per-channel cap"
        );
        return GatewayError::BadRequest(format!(
            "deposit exceeds the per-channel maximum of {} atomic USDC",
            state.config.channel.max_deposit_atomic.unwrap_or(0)
        ))
        .into_response();
    }

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
    /// The base58 32-byte channel id (identifies the channel to close).
    pub channel_id: String,
    /// Base64 ed25519 signature (64 bytes) over the canonical close message
    /// (`solvela-channel-close-v1 || channel_id`), proving control of the
    /// channel's `session_key`. Required — the `channel_id` alone is no longer a
    /// sufficient close capability (a leaked id must not force a close). There is
    /// deliberately NO refund-destination field: the refund goes to the stored
    /// `agent_wallet`, never a caller-supplied address.
    pub signature: String,
}

/// Response body for `POST /v1/channel/close`.
#[derive(Debug, Clone, Serialize)]
pub struct CloseChannelResponse {
    pub channel_id: String,
    pub deposited_atomic: u64,
    pub settled_atomic: u64,
    /// `deposited - realized` — what the agent never consumed, owed back. The
    /// amount is FROZEN into the `channel_refunds` reservation inside the same
    /// transaction that flips the channel `closing`.
    pub refundable_atomic: u64,
    /// Lifecycle status: `closing` (refund pending/in flight) or `closed`
    /// (refund confirmed).
    pub status: String,
    /// The refund reservation's status: `reserved` | `in_flight` |
    /// `confirmed` | `held`. Re-POSTing the (idempotent) signed close is the
    /// documented refund-status polling surface.
    pub refund_status: String,
    /// The refund's on-chain transaction signature, once broadcast — so the
    /// agent can verify the refund on-chain. `None` until the worker's claim
    /// persisted one.
    pub tx_signature: Option<String>,
}

/// POST /v1/channel/close
///
/// Cooperative close. Verifies a `session_key` signature over the canonical
/// close message, acquires the per-channel DRAW lock (close serializes against
/// in-flight draws — the anti-grief half of invariant 11; the one-transaction
/// flip→read→reserve below remains the correctness guarantee even across a
/// lock-expiry window), then in ONE transaction flips the channel `closing`,
/// computes `refundable = deposited - realized`, and FREEZES the refund
/// reservation `(amount, destination_wallet, mint)` — see
/// [`crate::channel_refunds::close_channel_and_reserve_refund`]. The
/// background refund worker disburses from the frozen tuple; re-POSTing the
/// idempotent close is the refund-status polling surface.
///
/// Status codes:
/// - 200 with the frozen refundable amount + the reservation's status (and its
///   `tx_signature` once broadcast).
/// - 400 when the `signature` is not base64-encoded 64 bytes.
/// - 401 when the signature does not match the channel's `session_key`.
/// - 404 `{"error":"channel not available"}` when channels are disabled or no
///   DB is configured, or `{"error":"channel not found"}` for an unknown id.
/// - 429 when the per-IP rate limit is exceeded.
/// - 500 (fail-closed) on a corrupt ledger row (`realized > deposited`) or
///   corrupt stored key material.
/// - 503 when Redis is absent/unreachable (the draw lock cannot serialize —
///   fail closed, never proceed lock-free) or a draw is in flight on this
///   channel (`Retry-After` set).
pub async fn close(
    State(state): State<Arc<AppState>>,
    peer_addr: PeerAddr,
    Json(req): Json<CloseChannelRequest>,
) -> axum::response::Response {
    // Gate 1 (cheapest-first): channels ship DISABLED by default → 404 before the
    // rate limiter. See [`open`].
    if !state.config.channel.enabled {
        return channel_not_available();
    }

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
        return channel_not_available();
    };

    // Close now serializes against draws via the per-channel Redis lock, so
    // Redis is REQUIRED here: absent/unreachable → 503 fail-closed, never a
    // lock-free close (a mid-serve draw could otherwise race the freeze on a
    // multi-instance deployment even though the transaction stays correct —
    // the lock is the liveness/anti-grief half).
    let Some(cache) = state.cache.as_ref() else {
        return service_unavailable("channel close requires the payment lock service");
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

    // Authenticate the close: the signature must prove control of the channel's
    // `session_key` (a leaked `channel_id` alone must not force a close). The
    // refund destination is the stored `agent_wallet`, never caller-supplied, so
    // a replayed close signature is a harmless idempotent no-op.
    if let Err(e) = authorize_close(&row.session_key, &row.channel_id, &req.signature) {
        return match e {
            CloseAuthError::MalformedSignature => {
                GatewayError::BadRequest("signature must be base64-encoded 64 bytes".to_string())
                    .into_response()
            }
            CloseAuthError::BadSignature => {
                tracing::warn!(channel_id = %req.channel_id, "channel close: signature did not verify");
                unauthorized("close signature does not match the channel session key")
            }
            CloseAuthError::CorruptStoredKey { which } => {
                tracing::error!(channel_id = %req.channel_id, which, "channel close: corrupt stored key");
                internal_error("channel credential material is inconsistent")
            }
        };
    }

    // Acquire the per-channel DRAW lock: a draw in flight blocks the close
    // (503 + Retry-After) — the deliberately-triggerable "draw an expensive
    // call, close mid-serve, collect the serve AND the full refund" grief loop.
    match cache.acquire_channel_draw_lock(&req.channel_id).await {
        Ok(true) => {}
        Ok(false) => {
            let mut resp = GatewayError::ServiceUnavailable(
                "a draw is in flight on this channel; retry the close shortly".to_string(),
            )
            .into_response();
            resp.headers_mut().insert(
                axum::http::header::RETRY_AFTER,
                axum::http::HeaderValue::from_static("5"),
            );
            return resp;
        }
        Err(e) => {
            tracing::warn!(error = %e, "channel close: lock acquisition failed (Redis)");
            return service_unavailable(
                "payment service is temporarily degraded; please retry shortly",
            );
        }
    }

    // Lock HELD: flip → read → reserve in ONE transaction, then release on
    // every path (bounded, like the draw — the TTL is only the crash backstop).
    let outcome =
        crate::channel_refunds::close_channel_and_reserve_refund(&pool, &req.channel_id).await;
    if tokio::time::timeout(
        std::time::Duration::from_secs(5),
        cache.release_channel_draw_lock(&req.channel_id),
    )
    .await
    .is_err()
    {
        tracing::warn!(
            channel_id = %req.channel_id,
            "channel close lock release timed out — lock will expire via TTL"
        );
    }

    match outcome {
        Ok(o) => {
            let response = CloseChannelResponse {
                channel_id: req.channel_id,
                deposited_atomic: o.deposited_atomic,
                settled_atomic: o.settled_atomic,
                refundable_atomic: o.refundable_atomic,
                status: o.channel_status,
                refund_status: o.refund_status,
                tx_signature: o.tx_signature,
            };
            (StatusCode::OK, Json(json!(response))).into_response()
        }
        Err(crate::channel_refunds::ChannelRefundError::Repo(
            channels::ChannelRepoError::ChannelNotFound,
        )) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "channel not found" })),
        )
            .into_response(),
        Err(e) => {
            tracing::error!(error = %e, channel_id = %req.channel_id, "channel close failed");
            internal_error("channel ledger state is inconsistent")
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A fresh base58 32-byte channel id. v0 generates 32 random bytes (two v4
/// UUIDs) so the id (a) unifies with the end-state Channel-PDA base58 and (b)
/// gives the voucher a clean 32-byte `channel_id` — a 16-byte UUID could not
/// fill the voucher's 32-byte field. Unguessable (256-bit). Note: the id is NOT
/// itself a close capability — closing additionally requires a `session_key`
/// signature ([`authorize_close`]).
fn fresh_channel_id() -> String {
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    bs58::encode(bytes).into_string()
}

/// 404 Not Found: the channel scheme is unavailable (disabled by config, no DB
/// pool, or — for the draw fork — no Redis). Same body on every unavailable
/// reason so the two are indistinguishable to a caller: the scheme is simply
/// not offered here. `pub(crate)` so the search-route draw fork returns the
/// identical 404, uniform with `open`/`close`.
pub(crate) fn channel_not_available() -> axum::response::Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({ "error": "channel not available" })),
    )
        .into_response()
}

/// 402 Payment Required with a fixed, safe message (never reflect RPC internals).
fn payment_required(message: &str) -> axum::response::Response {
    (
        StatusCode::PAYMENT_REQUIRED,
        Json(json!({ "error": message })),
    )
        .into_response()
}

/// 401 Unauthorized: a bad close credential (signature does not match the
/// channel's `session_key`). Distinct from 402 (payment) and 403 (policy) per
/// scope §3. Static message free of internals.
fn unauthorized(message: &str) -> axum::response::Response {
    (StatusCode::UNAUTHORIZED, Json(json!({ "error": message }))).into_response()
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

    // NOTE: the `compute_refundable` money-math tests (incl. the A1/#600
    // regression, now pinned on `deposited - realized`) moved with the
    // function to `crate::channel_refunds`.

    #[test]
    fn deposit_cap_accepts_at_or_below_and_rejects_above() {
        // Uncapped: everything passes.
        assert!(deposit_within_cap(u64::MAX, None));
        // Capped: boundary is inclusive; above fails closed.
        assert!(deposit_within_cap(50_000, Some(50_000)));
        assert!(!deposit_within_cap(50_001, Some(50_000)));
        assert!(deposit_within_cap(0, Some(0)));
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

    // -- A3: signed-close authorization (pure, no DB) -----------------------

    use base64::engine::general_purpose::STANDARD as B64;
    use ed25519_dalek::{Signer, SigningKey};

    /// (session_key_b58, channel_id_b58, valid signature_b64) for a channel id.
    fn signed_close_fixture(channel_id: [u8; 32]) -> (String, String, String) {
        let key = SigningKey::from_bytes(&[7u8; 32]);
        let session_key_b58 = bs58::encode(key.verifying_key().to_bytes()).into_string();
        let channel_id_b58 = bs58::encode(channel_id).into_string();
        let msg = solvela_x402::channel::build_close_message(&channel_id);
        let sig_b64 = B64.encode(key.sign(&msg).to_bytes());
        (session_key_b58, channel_id_b58, sig_b64)
    }

    #[test]
    fn authorize_close_accepts_valid_signature() {
        let (sk, cid, sig) = signed_close_fixture([9u8; 32]);
        assert!(authorize_close(&sk, &cid, &sig).is_ok());
    }

    #[test]
    fn authorize_close_rejects_flipped_byte_with_bad_signature() {
        let (sk, cid, sig) = signed_close_fixture([9u8; 32]);
        let mut raw = B64.decode(&sig).unwrap();
        raw[0] ^= 0xFF;
        let bad = B64.encode(&raw);
        assert!(matches!(
            authorize_close(&sk, &cid, &bad),
            Err(CloseAuthError::BadSignature)
        ));
    }

    #[test]
    fn authorize_close_rejects_wrong_session_key() {
        let (_sk, cid, sig) = signed_close_fixture([9u8; 32]);
        // A DIFFERENT key's pubkey as the channel session key → no match.
        let other = SigningKey::from_bytes(&[8u8; 32]);
        let other_sk = bs58::encode(other.verifying_key().to_bytes()).into_string();
        assert!(matches!(
            authorize_close(&other_sk, &cid, &sig),
            Err(CloseAuthError::BadSignature)
        ));
    }

    #[test]
    fn authorize_close_rejects_signature_for_a_different_channel() {
        // Signature was made over channel A; present it against channel B.
        let (sk, _cid_a, sig) = signed_close_fixture([1u8; 32]);
        let cid_b = bs58::encode([2u8; 32]).into_string();
        assert!(matches!(
            authorize_close(&sk, &cid_b, &sig),
            Err(CloseAuthError::BadSignature)
        ));
    }

    #[test]
    fn authorize_close_rejects_malformed_signature() {
        let (sk, cid, _sig) = signed_close_fixture([9u8; 32]);
        // Not base64.
        assert!(matches!(
            authorize_close(&sk, &cid, "not base64 !!!"),
            Err(CloseAuthError::MalformedSignature)
        ));
        // Valid base64 but wrong length (not 64 bytes).
        let short = B64.encode([0u8; 32]);
        assert!(matches!(
            authorize_close(&sk, &cid, &short),
            Err(CloseAuthError::MalformedSignature)
        ));
    }

    #[test]
    fn close_request_requires_signature_and_rejects_unknown_fields() {
        // signature is required — a body without it must not parse.
        assert!(serde_json::from_str::<CloseChannelRequest>(r#"{"channel_id":"c"}"#).is_err());
        // deny_unknown_fields: no refund-destination field can be smuggled in.
        let with_dest = r#"{"channel_id":"c","signature":"AA==","refund_to":"attacker"}"#;
        assert!(serde_json::from_str::<CloseChannelRequest>(with_dest).is_err());
        // Well-formed body parses.
        let ok = r#"{"channel_id":"c","signature":"AA=="}"#;
        assert!(serde_json::from_str::<CloseChannelRequest>(ok).is_ok());
    }

    // -- A4: honest close response (no false "done") ------------------------

    #[test]
    fn close_response_carries_reservation_status_and_signature() {
        // Pre-broadcast: the response must not imply the money moved.
        let resp = CloseChannelResponse {
            channel_id: "c".to_string(),
            deposited_atomic: 50_000,
            settled_atomic: 0,
            refundable_atomic: 46_220,
            status: ChannelStatus::Closing.as_str().to_string(),
            refund_status: "reserved".to_string(),
            tx_signature: None,
        };
        let v = serde_json::to_value(&resp).unwrap();
        assert_eq!(
            v["refund_status"], "reserved",
            "must not claim refunded before the worker confirms"
        );
        assert_eq!(v["status"], "closing");
        assert_eq!(v["refundable_atomic"], 46_220);
        assert_eq!(v["tx_signature"], serde_json::Value::Null);

        // Post-confirmation poll: the signature is surfaced for on-chain
        // verification by the agent.
        let done = CloseChannelResponse {
            channel_id: "c".to_string(),
            deposited_atomic: 50_000,
            settled_atomic: 0,
            refundable_atomic: 46_220,
            status: ChannelStatus::Closed.as_str().to_string(),
            refund_status: "confirmed".to_string(),
            tx_signature: Some("5sig".to_string()),
        };
        let v = serde_json::to_value(&done).unwrap();
        assert_eq!(v["refund_status"], "confirmed");
        assert_eq!(v["status"], "closed");
        assert_eq!(v["tx_signature"], "5sig");
    }

    // -- request_digest: byte-exact golden vector --------------------------

    /// `request_digest` = `SHA-256(raw request body)`, pinned to a fixed body so
    /// a one-byte SDK↔gateway disagreement is caught here, not by fail-closing
    /// every draw. The expected value was computed INDEPENDENTLY of this
    /// function (`printf '%s' '{"query":"solana x402"}' | sha256sum`), so it
    /// re-derives the layout, not this code's output. See channel scope §4.3.
    #[test]
    fn request_digest_matches_golden_vector() {
        let body = br#"{"query":"solana x402"}"#;
        // 9efc2adccccf39e642e803716d2e387685d3b8a31d289ec3326818cbe0df5b3f
        let expected: [u8; 32] = [
            0x9e, 0xfc, 0x2a, 0xdc, 0xcc, 0xcf, 0x39, 0xe6, 0x42, 0xe8, 0x03, 0x71, 0x6d, 0x2e,
            0x38, 0x76, 0x85, 0xd3, 0xb8, 0xa3, 0x1d, 0x28, 0x9e, 0xc3, 0x32, 0x68, 0x18, 0xcb,
            0xe0, 0xdf, 0x5b, 0x3f,
        ];
        assert_eq!(
            request_digest(body),
            expected,
            "request_digest bytes drifted from the pinned golden vector"
        );
        // A one-byte change in the body yields a different digest (binds to THE
        // specific request, not a normalized class of them).
        assert_ne!(request_digest(br#"{"query":"solana x403"}"#), expected);
    }

    // -- voucher_from_payload: decode + fail-closed ------------------------

    fn channel_voucher_payload(
        channel_id: [u8; 32],
        request_digest: [u8; 32],
        signature: [u8; 64],
    ) -> solvela_x402::types::ChannelVoucherPayload {
        solvela_x402::types::ChannelVoucherPayload {
            channel_id: bs58::encode(channel_id).into_string(),
            cumulative_atomic: 12_600,
            expiry_slot: 1_000_750,
            nonce: 42,
            request_digest: B64.encode(request_digest),
            signature: B64.encode(signature),
        }
    }

    #[test]
    fn voucher_from_payload_decodes_valid() {
        let payload = channel_voucher_payload([0x11; 32], [0x22; 32], [0x33; 64]);
        let voucher = voucher_from_payload(&payload).expect("valid payload decodes");
        assert_eq!(voucher.channel_id, [0x11; 32]);
        assert_eq!(voucher.request_digest, [0x22; 32]);
        assert_eq!(voucher.signature, [0x33; 64]);
        assert_eq!(voucher.cumulative_atomic, 12_600);
        assert_eq!(voucher.expiry_slot, 1_000_750);
        assert_eq!(voucher.nonce, 42);
    }

    #[test]
    fn voucher_from_payload_fails_closed_on_malformed_fields() {
        // Bad base58 channel_id.
        let mut p = channel_voucher_payload([0x11; 32], [0x22; 32], [0x33; 64]);
        p.channel_id = "not base58 0OIl".to_string();
        assert!(matches!(
            voucher_from_payload(&p),
            Err(ChannelDrawError::MalformedChannelId)
        ));

        // request_digest that is valid base64 but the WRONG length (16 bytes).
        let mut p = channel_voucher_payload([0x11; 32], [0x22; 32], [0x33; 64]);
        p.request_digest = B64.encode([0u8; 16]);
        assert!(matches!(
            voucher_from_payload(&p),
            Err(ChannelDrawError::MalformedRequestDigest)
        ));

        // signature that is valid base64 but the WRONG length (32 bytes).
        let mut p = channel_voucher_payload([0x11; 32], [0x22; 32], [0x33; 64]);
        p.signature = B64.encode([0u8; 32]);
        assert!(matches!(
            voucher_from_payload(&p),
            Err(ChannelDrawError::MalformedSignature)
        ));

        // Non-base64 signature.
        let mut p = channel_voucher_payload([0x11; 32], [0x22; 32], [0x33; 64]);
        p.signature = "!!! not base64 !!!".to_string();
        assert!(matches!(
            voucher_from_payload(&p),
            Err(ChannelDrawError::MalformedSignature)
        ));

        // BLOCKER guard: nonce > i64::MAX would verify + serve, then fail the
        // BIGINT persist → last never advances → free replay forever. Reject.
        let mut p = channel_voucher_payload([0x11; 32], [0x22; 32], [0x33; 64]);
        p.nonce = u64::MAX;
        assert!(matches!(
            voucher_from_payload(&p),
            Err(ChannelDrawError::MalformedNonce)
        ));
        // Exactly i64::MAX is fine (storable); i64::MAX + 1 is not.
        let mut p = channel_voucher_payload([0x11; 32], [0x22; 32], [0x33; 64]);
        p.nonce = i64::MAX as u64;
        assert!(voucher_from_payload(&p).is_ok());
        p.nonce = i64::MAX as u64 + 1;
        assert!(matches!(
            voucher_from_payload(&p),
            Err(ChannelDrawError::MalformedNonce)
        ));

        // Same guard on expiry_slot, symmetric boundary cases with nonce.
        let mut p = channel_voucher_payload([0x11; 32], [0x22; 32], [0x33; 64]);
        p.expiry_slot = u64::MAX;
        assert!(matches!(
            voucher_from_payload(&p),
            Err(ChannelDrawError::MalformedExpirySlot)
        ));
        p.expiry_slot = i64::MAX as u64;
        assert!(voucher_from_payload(&p).is_ok());
        p.expiry_slot = i64::MAX as u64 + 1;
        assert!(matches!(
            voucher_from_payload(&p),
            Err(ChannelDrawError::MalformedExpirySlot)
        ));
    }
}
