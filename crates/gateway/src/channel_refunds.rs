//! v0 spend-down channel — refund reservations + the USDC close-disbursement
//! worker (PR-A of the channel disbursement plan; closes the #600-stranding
//! gate that keeps `channel.enabled = false`).
//!
//! A cooperative close freezes the refund obligation **inside the close
//! transaction** ([`close_channel_and_reserve_refund`]): flip `open → closing`,
//! read the post-flip ledger, and INSERT the `channel_refunds` reservation, all
//! in ONE transaction, so no crash window can strand an obligation and no
//! in-flight draw can invalidate a frozen amount (invariant 11: the draw's
//! persist carries `AND status = 'open'` — Postgres row-lock ordering
//! serializes the race to exactly two safe outcomes).
//!
//! The background worker ([`RefundWorker`]) then drains reservations with the
//! single-winner discipline (invariant 13):
//! - sign OFF any row lock / DB transaction (RPC never runs under a lock);
//! - ONE short claim transaction = `pg_advisory_xact_lock` (serializes the
//!   global daily-cap SUM across instances) + a payload-carrying CAS
//!   `UPDATE … WHERE status = 'reserved'`; `rows_affected == 1` is the ONLY
//!   broadcast gate — a loser discards its locally signed bytes (they never
//!   leave the process);
//! - retries rebroadcast the SAME persisted bytes (the tx signature is the
//!   ledger-level dedupe);
//! - re-signing happens ONLY after conclusive death (`getBlockHeight` past the
//!   stored `last_valid_block_height` AND a **history-searching**
//!   `getSignatureStatuses` finds nothing — the recent-cache default form
//!   would double-refund a landed transfer), via a CAS predicated on the
//!   superseded `tx_signature`;
//! - there is deliberately NO blind timer reclaim of stale `in_flight` rows
//!   (a timer alone re-signs a slow-but-landed tx = double refund) and NO
//!   terminal-abandon state: exhaustion/conclusive failures go to `held`
//!   (alert + operator re-arm), never `failed`.
//!
//! **The worker is gated on DB-pool presence ONLY — NEVER on
//! `channel.enabled`** (see `main.rs`): the flag gates NEW deposits/draws/
//! closes, never the honoring of already-frozen obligations. Owed money keeps
//! moving, and the stuck-refund alert keeps firing, through any incident
//! flag-flip or rollback window.
//!
//! Money-path rules: integer atomic USDC only (checked `u64 <-> i64`
//! crossings via [`crate::channels`]); the send tuple `(amount,
//! destination_wallet, mint)` is consumed EXCLUSIVELY from the frozen
//! reservation row — never config, never a second `channels` read (a config
//! mint read at send time would move the wrong token after a mint migration);
//! the signer must be the recipient wallet (the escrow claimer's FIX 6
//! payer==payee guard); key material lives in the zeroize-on-drop
//! [`solvela_x402::fee_payer::FeePayerWallet`].

use std::sync::Arc;

use sqlx::PgPool;
use tracing::{error, info, warn};

use solvela_x402::escrow::claim_queue::{backoff_duration, MAX_CLAIM_ATTEMPTS};
use solvela_x402::fee_payer::FeePayerPool;
use solvela_x402::solana_rpc::SignatureStatus;
use solvela_x402::traits::Error as X402Error;
use solvela_x402::usdc_transfer::SignedUsdcTransfer;

use crate::channels::{atomic_to_i64, i64_to_atomic, ChannelRepoError, ChannelStatus};

// ---------------------------------------------------------------------------
// Errors + pure money math
// ---------------------------------------------------------------------------

/// Errors from the refund repository / close transaction. Every variant is a
/// hard failure — no silent `$0` refund, no defaulted row.
#[derive(Debug, thiserror::Error)]
pub enum ChannelRefundError {
    /// Ledger/conversion failure (DB error, out-of-range atomic value, missing
    /// channel).
    #[error(transparent)]
    Repo(#[from] ChannelRepoError),
    /// `realized > deposited` — a corrupt ledger row (the 017 CHECK chain
    /// should make this unreachable). Refuse rather than wrap the checked
    /// subtraction into a huge refund.
    #[error("realized {realized} exceeds deposited {deposited} (corrupt ledger row)")]
    RealizedExceedsDeposited { deposited: u64, realized: u64 },
}

impl From<sqlx::Error> for ChannelRefundError {
    fn from(e: sqlx::Error) -> Self {
        Self::Repo(ChannelRepoError::Db(e))
    }
}

/// Cooperative-close refundable = `deposited - realized`, checked.
///
/// `realized` (the synchronous per-draw actual-cost counter, migration 017) is
/// the agent's true obligation, so `deposited - realized` is exactly what the
/// agent never consumed. The two WRONG formulas this replaces/refuses:
/// - `deposited - settled` — `settled` lags asynchronously (~0 in v0), so it
///   over-refunds value already drawn (the A1 money loss);
/// - `deposited - last` — `last` accrues QUOTES; on chat the quote exceeds the
///   actual, so it strands the quote-vs-actual gap forever (#600). It was only
///   correct on `/v1/search`, where quote == actual == realized.
///
/// Never returns more than `deposited - realized`; an underflow (`realized >
/// deposited`) is a corrupt-state hard error, never a wrapped amount.
pub fn compute_refundable(
    deposited_atomic: u64,
    realized_atomic: u64,
) -> Result<u64, ChannelRefundError> {
    deposited_atomic.checked_sub(realized_atomic).ok_or(
        ChannelRefundError::RealizedExceedsDeposited {
            deposited: deposited_atomic,
            realized: realized_atomic,
        },
    )
}

// ---------------------------------------------------------------------------
// Refund lifecycle state
// ---------------------------------------------------------------------------

/// Lifecycle state of a refund reservation. Mirrors the DB `CHECK (status IN
/// ('reserved','in_flight','confirmed','held'))`; a typed enum at every CAS
/// call site stops a stray string from reaching the constraint.
///
/// There is deliberately NO terminal-abandon (`failed`) state: a refund is
/// owed money. `Held` retains the reservation, fires an alert on entry, and is
/// re-armed to `Reserved` by operator runbook only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefundStatus {
    /// Frozen obligation awaiting a send (or a zero-amount completion).
    Reserved,
    /// Signed bytes persisted (claim CAS won); broadcast/confirmation pending.
    InFlight,
    /// Refund landed on-chain (or amount was 0). Terminal; the channel is
    /// `closed` in the same transaction.
    Confirmed,
    /// Conclusive failure or retry exhaustion — alert-and-hold, operator
    /// re-arm only.
    Held,
}

impl RefundStatus {
    /// The exact lowercase string stored in `channel_refunds.status`.
    pub fn as_str(self) -> &'static str {
        match self {
            RefundStatus::Reserved => "reserved",
            RefundStatus::InFlight => "in_flight",
            RefundStatus::Confirmed => "confirmed",
            RefundStatus::Held => "held",
        }
    }
}

/// A refund reservation row, as the worker consumes it. The send tuple
/// `(amount_atomic, destination_wallet, mint)` is the FROZEN close-time value —
/// the worker never recomputes any of it.
#[derive(Debug, Clone)]
pub struct RefundRow {
    pub channel_id: String,
    pub amount_atomic: u64,
    pub destination_wallet: String,
    pub mint: String,
    pub status: String,
    pub tx_signature: Option<String>,
    pub signed_tx: Option<Vec<u8>>,
    pub last_valid_block_height: Option<u64>,
    pub attempts: i32,
    /// Seconds since the row was last touched (`NOW() - updated_at`) — the
    /// worker's retry-backoff input.
    pub age_secs: i64,
}

// ---------------------------------------------------------------------------
// Close transaction: flip -> read -> reserve (ONE transaction)
// ---------------------------------------------------------------------------

/// Everything the close response needs, read from the same transaction that
/// froze the obligation.
#[derive(Debug, Clone)]
pub struct CloseOutcome {
    /// `closing` (this call flipped it, or it already was) or `closed`.
    pub channel_status: String,
    pub deposited_atomic: u64,
    pub settled_atomic: u64,
    /// `deposited - realized` — the frozen (or to-be-frozen-identical) amount.
    pub refundable_atomic: u64,
    /// The reservation's current status (`reserved`/`in_flight`/`confirmed`/
    /// `held`).
    pub refund_status: String,
    /// The refund's on-chain signature, once the claim CAS persisted one.
    pub tx_signature: Option<String>,
}

/// Cooperative close: flip `open → closing`, read the post-flip ledger, and
/// freeze the refund reservation — in **one transaction** (invariant 11b).
///
/// - The flip (`UPDATE … SET status = 'closing' WHERE status = 'open'
///   RETURNING …`) re-reads the row AFTER any concurrent draw's committed
///   persist (Postgres READ COMMITTED re-evaluation under the row lock), so
///   the frozen amount always reflects every draw that charged the agent.
/// - The reservation INSERT (`ON CONFLICT DO NOTHING`) freezes the FULL send
///   tuple: `amount = deposited - realized`, `destination_wallet =
///   channels.agent_wallet`, `mint = channels.mint` — never caller-supplied,
///   never config.
/// - Already-`closing`/`closed` channels are idempotent: the reservation is
///   ensured (covers legacy pre-017 rows and any historical gap — safe because
///   `realized` is frozen post-flip) and the current status + `tx_signature`
///   are returned, making the re-POSTed close THE refund-status poll surface.
pub async fn close_channel_and_reserve_refund(
    pool: &PgPool,
    channel_id: &str,
) -> Result<CloseOutcome, ChannelRefundError> {
    let mut tx = pool.begin().await?;

    // Flip open -> closing. RETURNING reads the post-flip (= post-any-draw)
    // ledger values the refund freezes against.
    let flipped: Option<(i64, i64, i64, String, String)> = sqlx::query_as(
        "UPDATE channels
            SET status = $1, updated_at = NOW()
          WHERE channel_id = $2 AND status = $3
      RETURNING deposited_atomic, settled_atomic, realized_atomic, agent_wallet, mint",
    )
    .bind(ChannelStatus::Closing.as_str())
    .bind(channel_id)
    .bind(ChannelStatus::Open.as_str())
    .fetch_optional(&mut *tx)
    .await?;

    let (deposited, settled, realized, wallet, mint, channel_status) = match flipped {
        Some((d, s, r, w, m)) => (d, s, r, w, m, ChannelStatus::Closing.as_str().to_string()),
        None => {
            // Not open: already closing/closed (idempotent re-close), or
            // missing. `realized` is frozen post-flip (the draw persist
            // requires status = 'open'), so reading it here is stable.
            let row: Option<(i64, i64, i64, String, String, String)> = sqlx::query_as(
                "SELECT deposited_atomic, settled_atomic, realized_atomic,
                        agent_wallet, mint, status
                   FROM channels
                  WHERE channel_id = $1",
            )
            .bind(channel_id)
            .fetch_optional(&mut *tx)
            .await?;
            let Some((d, s, r, w, m, status)) = row else {
                return Err(ChannelRepoError::ChannelNotFound.into());
            };
            (d, s, r, w, m, status)
        }
    };

    let deposited_atomic = i64_to_atomic(deposited).map_err(ChannelRefundError::Repo)?;
    let settled_atomic = i64_to_atomic(settled).map_err(ChannelRefundError::Repo)?;
    let realized_atomic = i64_to_atomic(realized).map_err(ChannelRefundError::Repo)?;
    let refundable_atomic = compute_refundable(deposited_atomic, realized_atomic)?;

    // Freeze/ensure the reservation. ON CONFLICT DO NOTHING: an existing
    // reservation's frozen tuple is NEVER overwritten.
    sqlx::query(
        "INSERT INTO channel_refunds
           (channel_id, amount_atomic, destination_wallet, mint, status)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (channel_id) DO NOTHING",
    )
    .bind(channel_id)
    .bind(atomic_to_i64(refundable_atomic).map_err(ChannelRefundError::Repo)?)
    .bind(&wallet)
    .bind(&mint)
    .bind(RefundStatus::Reserved.as_str())
    .execute(&mut *tx)
    .await?;

    let (refund_status, tx_signature): (String, Option<String>) =
        sqlx::query_as("SELECT status, tx_signature FROM channel_refunds WHERE channel_id = $1")
            .bind(channel_id)
            .fetch_one(&mut *tx)
            .await?;

    tx.commit().await?;

    Ok(CloseOutcome {
        channel_status,
        deposited_atomic,
        settled_atomic,
        refundable_atomic,
        refund_status,
        tx_signature,
    })
}

// ---------------------------------------------------------------------------
// Worker repository: every transition a status-predicated, rows_affected-gated
// CAS (invariant 13)
// ---------------------------------------------------------------------------

/// Fetch refund rows in `status` that are due per the claim-queue backoff
/// cadence (the caller filters `age_secs >= backoff(attempts)`).
async fn fetch_refunds(pool: &PgPool, status: RefundStatus) -> Result<Vec<RefundRow>, sqlx::Error> {
    #[allow(clippy::type_complexity)]
    let rows: Vec<(
        String,
        i64,
        String,
        String,
        String,
        Option<String>,
        Option<Vec<u8>>,
        Option<i64>,
        i32,
        i64,
    )> = sqlx::query_as(
        "SELECT channel_id, amount_atomic, destination_wallet, mint, status,
                tx_signature, signed_tx, last_valid_block_height, attempts,
                EXTRACT(EPOCH FROM (NOW() - updated_at))::BIGINT AS age_secs
           FROM channel_refunds
          WHERE status = $1
          ORDER BY created_at ASC
          LIMIT 32",
    )
    .bind(status.as_str())
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .filter_map(|r| {
            // A negative stored amount/height is a corrupt row; skip it loudly
            // rather than wrapping into a huge u64 send.
            let amount_atomic = match i64_to_atomic(r.1) {
                Ok(a) => a,
                Err(e) => {
                    metrics::counter!("solvela_channel_refund_corrupt_row_total").increment(1);
                    error!(channel_id = %r.0, error = %e, "corrupt refund amount — row skipped");
                    return None;
                }
            };
            let last_valid_block_height = match r.7.map(i64_to_atomic).transpose() {
                Ok(h) => h,
                Err(e) => {
                    metrics::counter!("solvela_channel_refund_corrupt_row_total").increment(1);
                    error!(channel_id = %r.0, error = %e, "corrupt refund block height — row skipped");
                    return None;
                }
            };
            Some(RefundRow {
                channel_id: r.0,
                amount_atomic,
                destination_wallet: r.2,
                mint: r.3,
                status: r.4,
                tx_signature: r.5,
                signed_tx: r.6,
                last_valid_block_height,
                attempts: r.8,
                age_secs: r.9,
            })
        })
        .collect())
}

/// Outcome of the single-winner claim transaction.
#[derive(Debug, PartialEq, Eq)]
pub enum ClaimOutcome {
    /// THIS caller's bytes are persisted — it (and only it) must broadcast
    /// exactly those bytes.
    Won,
    /// A peer instance won the CAS — discard the locally signed bytes.
    Lost,
    /// The global daily cap would be exceeded — no flip; the obligation stays
    /// `reserved` (alerted, retried once headroom returns). NOT the `held`
    /// status: nothing transitions.
    CapExceeded,
}

/// The full claim payload — named fields so a call site can never transpose
/// the adjacent `u64`s (`last_valid_block_height` vs `amount_atomic`: a
/// swapped positional call compiles and persists a wrong amount).
#[derive(Debug)]
pub struct RefundClaim<'a> {
    pub channel_id: &'a str,
    /// The exact wire bytes to persist (and later broadcast verbatim).
    pub signed_tx: &'a [u8],
    /// Base58 transaction signature of `signed_tx`.
    pub tx_signature: &'a str,
    pub last_valid_block_height: u64,
    pub amount_atomic: u64,
    /// Global trailing-24h disbursement ceiling; `None` = uncapped.
    pub daily_cap_atomic: Option<u64>,
}

/// The §5.C.2b single-winner claim: ONE short transaction —
/// `pg_advisory_xact_lock` (serializes the daily-cap SUM across concurrent
/// claimers; a SUM outside this transaction is a read-then-act TOCTOU) →
/// trailing-24h cap check → payload-carrying CAS `UPDATE … WHERE status =
/// 'reserved'`. `rows_affected == 1` is the broadcast gate; the broadcast
/// happens only AFTER this commits, and only ever with these persisted bytes.
pub async fn claim_refund_for_broadcast(
    pool: &PgPool,
    claim: &RefundClaim<'_>,
) -> Result<ClaimOutcome, ChannelRefundError> {
    let mut tx = pool.begin().await?;

    // ponytail: one global advisory lock; refund throughput is closes-per-day,
    // not requests-per-second — shard the lock key if that ever changes.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('channel_refund_claim'))")
        .execute(&mut *tx)
        .await?;

    if let Some(cap) = claim.daily_cap_atomic {
        // `held` rows are excluded: every held path (deterministic rejection,
        // landed-with-error, retry exhaustion) means no USDC left the wallet,
        // so counting them would only throttle legitimate refunds. Runbook
        // note: operator re-arm (held -> reserved) does NOT clear
        // first_broadcast_at — a re-armed row re-enters this window at its
        // ORIGINAL broadcast anchor once it leaves `held` again.
        let broadcast_24h: i64 = sqlx::query_scalar(
            "SELECT COALESCE(SUM(amount_atomic), 0)::BIGINT
               FROM channel_refunds
              WHERE first_broadcast_at > NOW() - INTERVAL '24 hours'
                AND status <> $1",
        )
        .bind(RefundStatus::Held.as_str())
        .fetch_one(&mut *tx)
        .await?;
        let already = i64_to_atomic(broadcast_24h).map_err(ChannelRefundError::Repo)?;
        if already.saturating_add(claim.amount_atomic) > cap {
            tx.rollback().await?;
            return Ok(ClaimOutcome::CapExceeded);
        }
    }

    let res = sqlx::query(
        "UPDATE channel_refunds
            SET status = $1,
                signed_tx = $2,
                tx_signature = $3,
                last_valid_block_height = $4,
                attempts = attempts + 1,
                first_broadcast_at = COALESCE(first_broadcast_at, NOW()),
                updated_at = NOW()
          WHERE channel_id = $5 AND status = $6",
    )
    .bind(RefundStatus::InFlight.as_str())
    .bind(claim.signed_tx)
    .bind(claim.tx_signature)
    .bind(atomic_to_i64(claim.last_valid_block_height).map_err(ChannelRefundError::Repo)?)
    .bind(claim.channel_id)
    .bind(RefundStatus::Reserved.as_str())
    .execute(&mut *tx)
    .await?;

    if res.rows_affected() == 1 {
        tx.commit().await?;
        Ok(ClaimOutcome::Won)
    } else {
        tx.rollback().await?;
        Ok(ClaimOutcome::Lost)
    }
}

/// Confirm a landed (or zero-amount) refund AND close the channel — **one
/// transaction** (a crash between the two writes would strand the channel in
/// `closing` with its refund already confirmed; addendum item 5).
///
/// Returns `false` (benign no-op) when a peer already confirmed.
pub async fn confirm_refund_and_close_channel(
    pool: &PgPool,
    channel_id: &str,
    from: RefundStatus,
) -> Result<bool, ChannelRefundError> {
    let mut tx = pool.begin().await?;
    let res = sqlx::query(
        "UPDATE channel_refunds
            SET status = $1, updated_at = NOW()
          WHERE channel_id = $2 AND status = $3",
    )
    .bind(RefundStatus::Confirmed.as_str())
    .bind(channel_id)
    .bind(from.as_str())
    .execute(&mut *tx)
    .await?;
    if res.rows_affected() != 1 {
        tx.rollback().await?;
        return Ok(false);
    }
    // Idempotent: a channel already `closed` (or a test row never flipped)
    // matches 0 rows, which is fine — the refund confirmation is the money
    // fact; the channel status is bookkeeping riding the same transaction.
    sqlx::query(
        "UPDATE channels SET status = $1, updated_at = NOW()
          WHERE channel_id = $2 AND status = $3",
    )
    .bind(ChannelStatus::Closed.as_str())
    .bind(channel_id)
    .bind(ChannelStatus::Closing.as_str())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(true)
}

/// Replace a conclusively-dead transaction's bytes — CAS predicated on the
/// SUPERSEDED `tx_signature`, which is what stops two instances that
/// concurrently concluded death from both broadcasting: the loser matches 0
/// rows and discards its bytes.
pub async fn resign_refund(
    pool: &PgPool,
    channel_id: &str,
    old_tx_signature: &str,
    new_signed_tx: &[u8],
    new_tx_signature: &str,
    new_last_valid_block_height: u64,
) -> Result<bool, ChannelRefundError> {
    let res = sqlx::query(
        "UPDATE channel_refunds
            SET signed_tx = $1,
                tx_signature = $2,
                last_valid_block_height = $3,
                attempts = attempts + 1,
                updated_at = NOW()
          WHERE channel_id = $4 AND status = $5 AND tx_signature = $6",
    )
    .bind(new_signed_tx)
    .bind(new_tx_signature)
    .bind(atomic_to_i64(new_last_valid_block_height).map_err(ChannelRefundError::Repo)?)
    .bind(channel_id)
    .bind(RefundStatus::InFlight.as_str())
    .bind(old_tx_signature)
    .execute(pool)
    .await?;
    Ok(res.rows_affected() == 1)
}

/// Move a reservation to `held` (alert-and-hold; operator re-arm only).
/// Status-predicated CAS; returns `false` when the row already moved on.
///
/// Runbook: re-arm is `UPDATE channel_refunds SET status = 'reserved' WHERE
/// channel_id = … AND status = 'held'` after fixing the cause. Re-arm keeps
/// `first_broadcast_at` (the daily-cap anchor) — see the cap note in
/// [`claim_refund_for_broadcast`].
pub async fn hold_refund(
    pool: &PgPool,
    channel_id: &str,
    from: RefundStatus,
) -> Result<bool, ChannelRefundError> {
    let res = sqlx::query(
        "UPDATE channel_refunds
            SET status = $1, updated_at = NOW()
          WHERE channel_id = $2 AND status = $3",
    )
    .bind(RefundStatus::Held.as_str())
    .bind(channel_id)
    .bind(from.as_str())
    .execute(pool)
    .await?;
    Ok(res.rows_affected() == 1)
}

/// Age (seconds) of the oldest not-yet-confirmed refund, for the stuck-refund
/// gauge/alert. `None` when nothing is pending.
async fn oldest_pending_age_secs(pool: &PgPool) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT EXTRACT(EPOCH FROM (NOW() - MIN(created_at)))::BIGINT
           FROM channel_refunds
          WHERE status IN ($1, $2)",
    )
    .bind(RefundStatus::Reserved.as_str())
    .bind(RefundStatus::InFlight.as_str())
    .fetch_one(pool)
    .await
}

/// `held` visibility: (count, age-of-oldest-held-entry seconds). A held row is
/// operator-owed money — it must keep signaling every sweep until re-armed,
/// not just at the one-shot entry alert (age keyed on `updated_at`, which the
/// hold CAS set on entry).
async fn held_stats(pool: &PgPool) -> Result<(i64, Option<i64>), sqlx::Error> {
    sqlx::query_as(
        "SELECT COUNT(*),
                EXTRACT(EPOCH FROM (NOW() - MIN(updated_at)))::BIGINT
           FROM channel_refunds
          WHERE status = $1",
    )
    .bind(RefundStatus::Held.as_str())
    .fetch_one(pool)
    .await
}

// ---------------------------------------------------------------------------
// RPC seam (mocked in tests, real over solana_rpc)
// ---------------------------------------------------------------------------

/// The on-chain operations the refund worker needs. A seam so the exactly-once
/// state machine is provable in tests without a live RPC (the faucet's
/// `GasSource` pattern).
#[async_trait::async_trait]
pub trait RefundRpc: Send + Sync {
    /// `owner`'s balance of `mint` in atomic units (via the owner's ATA —
    /// derived from the FROZEN reservation mint, never config).
    async fn usdc_balance(&self, owner_b58: &str, mint_b58: &str) -> Result<u64, X402Error>;
    /// A fresh blockhash + its `lastValidBlockHeight`.
    async fn latest_blockhash_and_height(&self) -> Result<([u8; 32], u64), X402Error>;
    /// Current block height (`confirmed`) — the death-check reader.
    async fn block_height(&self) -> Result<u64, X402Error>;
    /// One-shot signature status. Callers on the death/recovery path MUST pass
    /// `search_history = true` (recent-cache reads double-refund).
    async fn signature_status(
        &self,
        signature_b58: &str,
        search_history: bool,
    ) -> Result<Option<SignatureStatus>, X402Error>;
    /// Broadcast base64 wire bytes; returns the signature string.
    async fn send_transaction(&self, base64_tx: &str) -> Result<String, X402Error>;
}

/// Production [`RefundRpc`] over the shared `solana_rpc` helpers.
pub struct SolanaRefundRpc {
    client: reqwest::Client,
    rpc_url: String,
}

impl SolanaRefundRpc {
    pub fn new(client: reqwest::Client, rpc_url: String) -> Self {
        Self { client, rpc_url }
    }
}

#[async_trait::async_trait]
impl RefundRpc for SolanaRefundRpc {
    async fn usdc_balance(&self, owner_b58: &str, mint_b58: &str) -> Result<u64, X402Error> {
        solvela_x402::solana_rpc::get_usdc_balance(&self.client, &self.rpc_url, owner_b58, mint_b58)
            .await
    }
    async fn latest_blockhash_and_height(&self) -> Result<([u8; 32], u64), X402Error> {
        solvela_x402::solana_rpc::get_latest_blockhash_and_height(
            &self.client,
            &self.rpc_url,
            "confirmed",
        )
        .await
    }
    async fn block_height(&self) -> Result<u64, X402Error> {
        solvela_x402::solana_rpc::get_block_height(&self.client, &self.rpc_url).await
    }
    async fn signature_status(
        &self,
        signature_b58: &str,
        search_history: bool,
    ) -> Result<Option<SignatureStatus>, X402Error> {
        solvela_x402::solana_rpc::get_signature_status(
            &self.client,
            &self.rpc_url,
            signature_b58,
            search_history,
        )
        .await
    }
    async fn send_transaction(&self, base64_tx: &str) -> Result<String, X402Error> {
        solvela_x402::solana_rpc::send_transaction(&self.client, &self.rpc_url, base64_tx).await
    }
}

// ---------------------------------------------------------------------------
// The worker
// ---------------------------------------------------------------------------

/// Alert threshold for the oldest pending refund (seconds). A refund is
/// user-visible money not arriving — detect in minutes, not from complaints.
const STUCK_REFUND_ALERT_SECS: i64 = 3_600;

/// The wired refund worker. Note what is deliberately ABSENT: any
/// `channel.enabled` input — the worker cannot consult the flag by
/// construction (pool-gated only; round-2 finding 7).
pub struct RefundWorker {
    pool: PgPool,
    rpc: Arc<dyn RefundRpc>,
    /// The signing capability — `None` (no fee-payer keys configured) leaves
    /// obligations `reserved` with the age alert firing; it never abandons
    /// them.
    fee_payer_pool: Option<Arc<FeePayerPool>>,
    /// Base58 recipient wallet — the refund SOURCE. The signer's pubkey must
    /// equal it (the claimer FIX 6 payer==payee guard).
    recipient_wallet: String,
    /// Global trailing-24h disbursement ceiling (atomic). `None` = uncapped.
    daily_cap_atomic: Option<u64>,
}

impl RefundWorker {
    pub fn new(
        pool: PgPool,
        rpc: Arc<dyn RefundRpc>,
        fee_payer_pool: Option<Arc<FeePayerPool>>,
        recipient_wallet: String,
        daily_cap_atomic: Option<u64>,
    ) -> Self {
        Self {
            pool,
            rpc,
            fee_payer_pool,
            recipient_wallet,
            daily_cap_atomic,
        }
    }

    /// One sweep: gauge/alert, then drain `reserved` and `in_flight` rows that
    /// are due per the claim-queue backoff cadence.
    pub async fn sweep(&self) {
        // Every Err arm below ALSO bumps the sweep-error counter: a Prometheus
        // gauge holds its last value, so a silently-failing sweep would read
        // as all-clear forever without a rising error series next to it.
        match oldest_pending_age_secs(&self.pool).await {
            Ok(age) => {
                let secs = age.unwrap_or(0).max(0);
                metrics::gauge!("solvela_channel_refund_oldest_pending_seconds").set(secs as f64);
                if secs > STUCK_REFUND_ALERT_SECS {
                    error!(
                        oldest_pending_secs = secs,
                        "channel refund pending past the alert threshold — user-visible money \
                         is not arriving"
                    );
                }
            }
            Err(e) => {
                metrics::counter!("solvela_channel_refund_sweep_error_total").increment(1);
                warn!(error = %e, "refund worker: pending-age read failed");
            }
        }

        // Continuous `held` visibility (never one-shot): a held row is owed
        // money awaiting operator action — it keeps signaling every sweep,
        // through gauges + the same age-threshold error alert, until re-armed.
        match held_stats(&self.pool).await {
            Ok((count, oldest)) => {
                metrics::gauge!("solvela_channel_refund_held_count").set(count as f64);
                let secs = oldest.unwrap_or(0).max(0);
                metrics::gauge!("solvela_channel_refund_held_oldest_seconds").set(secs as f64);
                if secs > STUCK_REFUND_ALERT_SECS {
                    error!(
                        held_count = count,
                        held_oldest_secs = secs,
                        "channel refund HELD past the alert threshold — operator re-arm \
                         required (owed money is not moving)"
                    );
                }
            }
            Err(e) => {
                metrics::counter!("solvela_channel_refund_sweep_error_total").increment(1);
                warn!(error = %e, "refund worker: held stats read failed");
            }
        }

        match fetch_refunds(&self.pool, RefundStatus::Reserved).await {
            Ok(rows) => {
                for row in rows {
                    if row.age_secs >= backoff_duration(row.attempts).as_secs() as i64 {
                        self.handle_reserved(&row).await;
                    }
                }
            }
            Err(e) => {
                metrics::counter!("solvela_channel_refund_sweep_error_total").increment(1);
                warn!(error = %e, "refund worker: reserved scan failed");
            }
        }

        match fetch_refunds(&self.pool, RefundStatus::InFlight).await {
            Ok(rows) => {
                for row in rows {
                    if row.age_secs >= backoff_duration(row.attempts).as_secs() as i64 {
                        self.handle_in_flight(&row).await;
                    }
                }
            }
            Err(e) => {
                metrics::counter!("solvela_channel_refund_sweep_error_total").increment(1);
                warn!(error = %e, "refund worker: in_flight scan failed");
            }
        }
    }

    /// Decode the frozen tuple + build + sign a fresh transaction — all OFF
    /// any row lock / DB transaction. Returns the signed transfer and its
    /// `last_valid_block_height`, or `None` (logged) when signing is not
    /// currently possible (row retained for retry; a CORRUPT frozen tuple —
    /// unfixable by retry — is held from `from` with the entry alert).
    async fn sign_fresh(
        &self,
        row: &RefundRow,
        from: RefundStatus,
    ) -> Option<(SignedUsdcTransfer, u64)> {
        let Some(pool) = self.fee_payer_pool.as_ref() else {
            warn!(
                channel_id = %row.channel_id,
                "refund worker: no fee-payer key configured — reservation retained"
            );
            return None;
        };
        let wallet = match pool.next() {
            Ok(w) => w,
            Err(e) => {
                warn!(channel_id = %row.channel_id, error = %e, "refund worker: no healthy fee payer");
                return None;
            }
        };

        // FIX 6 payer==payee: the refund source is the recipient wallet; never
        // sign with a key that is not that wallet. Both sides are base58
        // strings (the wallet derives its pubkey from the validated keypair).
        if wallet.pubkey_b58 != self.recipient_wallet {
            error!(
                channel_id = %row.channel_id,
                "refund worker: fee_payer_key does not belong to recipient_wallet — refusing to \
                 sign (payer==payee only; see escrow claimer FIX 6)"
            );
            return None;
        }

        // The FROZEN tuple. A corrupt destination/mint is conclusive (no retry
        // can fix a bad frozen value) → held + alert, never a silent loop.
        let destination = match solvela_x402::escrow::pda::decode_bs58_pubkey(
            &row.destination_wallet,
        ) {
            Ok(d) => d,
            Err(_) => {
                error!(channel_id = %row.channel_id, "refund worker: corrupt frozen destination_wallet");
                self.hold_with_alert(&row.channel_id, from).await;
                return None;
            }
        };
        let mint = match solvela_x402::escrow::pda::decode_bs58_pubkey(&row.mint) {
            Ok(m) => m,
            Err(_) => {
                error!(channel_id = %row.channel_id, "refund worker: corrupt frozen mint");
                self.hold_with_alert(&row.channel_id, from).await;
                return None;
            }
        };

        let (blockhash, last_valid_block_height) = match self
            .rpc
            .latest_blockhash_and_height()
            .await
        {
            Ok(v) => v,
            Err(e) => {
                warn!(channel_id = %row.channel_id, error = %e, "refund worker: blockhash fetch failed");
                return None;
            }
        };

        // Typed signing surface: the raw keypair never crosses the crate
        // boundary; the owner is always the wallet's own pubkey.
        match wallet.sign_usdc_transfer_checked(&destination, &mint, row.amount_atomic, &blockhash)
        {
            Ok(signed) => Some((signed, last_valid_block_height)),
            Err(e) => {
                error!(channel_id = %row.channel_id, error = %e, "refund worker: signing failed");
                None
            }
        }
    }

    /// Move a row to `held` with the entry alert (log + counter).
    async fn hold_with_alert(&self, channel_id: &str, from: RefundStatus) {
        match hold_refund(&self.pool, channel_id, from).await {
            Ok(true) => {
                metrics::counter!("solvela_channel_refund_held_total").increment(1);
                error!(
                    channel_id,
                    "channel refund HELD — operator action required (runbook: fix the cause, \
                     re-arm the row to 'reserved')"
                );
            }
            Ok(false) => {}
            Err(e) => warn!(channel_id, error = %e, "refund worker: hold CAS failed"),
        }
    }

    /// Drain one `reserved` row: zero-amount completion, or pre-checks + sign
    /// (off-lock) + single-winner claim + broadcast.
    async fn handle_reserved(&self, row: &RefundRow) {
        // A fully-drawn channel owes nothing: confirm + close, no tx.
        if row.amount_atomic == 0 {
            match confirm_refund_and_close_channel(
                &self.pool,
                &row.channel_id,
                RefundStatus::Reserved,
            )
            .await
            {
                Ok(true) => {
                    metrics::counter!("solvela_channel_refund_confirmed_total").increment(1);
                    info!(channel_id = %row.channel_id, "zero-amount refund completed; channel closed");
                }
                Ok(false) => {}
                Err(e) => {
                    warn!(channel_id = %row.channel_id, error = %e, "refund worker: zero-amount confirm failed")
                }
            }
            return;
        }

        // Balance pre-check against the operational wallet's ATA for the
        // FROZEN mint (addendum item 3) — insufficient funds retain the
        // reservation (the obligation is durable, the send is deferred; NEVER
        // a partial send). ponytail: no attempts increment while `reserved` —
        // the reserved→held-on-exhaustion arm was dropped as dead (addendum
        // item 1); a permanently blocked reservation is guarded by the
        // oldest-pending age alert instead.
        match self
            .rpc
            .usdc_balance(&self.recipient_wallet, &row.mint)
            .await
        {
            Ok(balance) if balance < row.amount_atomic => {
                metrics::counter!("solvela_channel_refund_balance_insufficient_total").increment(1);
                error!(
                    channel_id = %row.channel_id,
                    balance,
                    amount = row.amount_atomic,
                    "refund worker: operational wallet balance below refund amount — \
                     reservation retained, top up the wallet"
                );
                return;
            }
            Ok(_) => {}
            Err(e) => {
                warn!(channel_id = %row.channel_id, error = %e, "refund worker: balance check failed");
                return;
            }
        }

        let Some((signed, last_valid_block_height)) =
            self.sign_fresh(row, RefundStatus::Reserved).await
        else {
            return;
        };

        // Single-winner claim (the ONLY broadcast gate).
        match claim_refund_for_broadcast(
            &self.pool,
            &RefundClaim {
                channel_id: &row.channel_id,
                signed_tx: &signed.wire_bytes,
                tx_signature: &signed.signature_b58,
                last_valid_block_height,
                amount_atomic: row.amount_atomic,
                daily_cap_atomic: self.daily_cap_atomic,
            },
        )
        .await
        {
            Ok(ClaimOutcome::Won) => {}
            Ok(ClaimOutcome::Lost) => {
                // A peer persisted ITS bytes; ours never leave the process.
                metrics::counter!("solvela_channel_refund_claim_lost_total").increment(1);
                return;
            }
            Ok(ClaimOutcome::CapExceeded) => {
                metrics::counter!("solvela_channel_refund_daily_cap_exceeded_total").increment(1);
                error!(
                    channel_id = %row.channel_id,
                    amount = row.amount_atomic,
                    "refund worker: global daily disbursement cap reached — reservation \
                     retained (drains when the trailing-24h window frees headroom)"
                );
                return;
            }
            Err(e) => {
                warn!(channel_id = %row.channel_id, error = %e, "refund worker: claim transaction failed");
                return;
            }
        }

        self.broadcast(&row.channel_id, &signed.base64_tx).await;
    }

    /// Broadcast persisted bytes; classify deterministic rejections to `held`.
    async fn broadcast(&self, channel_id: &str, base64_tx: &str) {
        match self.rpc.send_transaction(base64_tx).await {
            Ok(_sig) => {
                metrics::counter!("solvela_channel_refund_broadcast_total").increment(1);
            }
            Err(e) => {
                if solvela_x402::solana_rpc::is_already_processed_error(&e) {
                    // Landed already (a prior crashed broadcast) — the next
                    // sweep's history-searching check confirms it.
                    return;
                }
                let raw = e.to_string();
                match solvela_x402::solana_rpc::classify_settlement_error(&raw) {
                    solvela_x402::types::SettlementFailureKind::Rejected { program_error_code } => {
                        // Deterministic program rejection: these bytes can
                        // never land — conclusive execution failure.
                        error!(
                            channel_id,
                            ?program_error_code,
                            "refund broadcast rejected by the program — holding"
                        );
                        self.hold_with_alert(channel_id, RefundStatus::InFlight)
                            .await;
                    }
                    _ => {
                        // Transient (transport / blockhash) — the in_flight
                        // recovery path rebroadcasts or re-signs safely.
                        warn!(channel_id, error = %raw, "refund broadcast failed transiently");
                    }
                }
            }
        }
    }

    /// Recover one `in_flight` row. FIRST action is always the
    /// history-searching signature check (never the recent-cache default,
    /// never a timer): landed → confirm; alive → rebroadcast the SAME bytes;
    /// conclusively dead → signature-predicated re-sign.
    async fn handle_in_flight(&self, row: &RefundRow) {
        let (Some(tx_signature), Some(signed_tx), Some(last_valid_block_height)) = (
            row.tx_signature.as_deref(),
            row.signed_tx.as_deref(),
            row.last_valid_block_height,
        ) else {
            // The claim CAS persists all three in one statement; a partial row
            // is corruption, not a retry case.
            error!(channel_id = %row.channel_id, "refund worker: in_flight row missing claim payload");
            self.hold_with_alert(&row.channel_id, RefundStatus::InFlight)
                .await;
            return;
        };

        match self.rpc.signature_status(tx_signature, true).await {
            Ok(Some(status)) => {
                if let Some(err) = &status.err {
                    // Landed AND failed on-chain — conclusive execution
                    // failure (the classify path exists for the code, the
                    // verdict is the same either way).
                    let kind = solvela_x402::solana_rpc::classify_settlement_error(err);
                    error!(
                        channel_id = %row.channel_id,
                        error = %err,
                        ?kind,
                        "refund landed with an on-chain error — holding"
                    );
                    self.hold_with_alert(&row.channel_id, RefundStatus::InFlight)
                        .await;
                    return;
                }
                match status.confirmation_status.as_deref() {
                    Some("confirmed") | Some("finalized") => {
                        match confirm_refund_and_close_channel(
                            &self.pool,
                            &row.channel_id,
                            RefundStatus::InFlight,
                        )
                        .await
                        {
                            Ok(true) => {
                                metrics::counter!("solvela_channel_refund_confirmed_total")
                                    .increment(1);
                                info!(
                                    channel_id = %row.channel_id,
                                    tx_signature,
                                    amount = row.amount_atomic,
                                    "channel refund confirmed on-chain; channel closed"
                                );
                            }
                            Ok(false) => {}
                            Err(e) => {
                                warn!(channel_id = %row.channel_id, error = %e, "refund worker: confirm CAS failed")
                            }
                        }
                    }
                    // `processed` (or unknown): observed but not quorum-voted —
                    // wait for the next sweep.
                    _ => {}
                }
            }
            Ok(None) => {
                // No trace in FULL history. Dead only once the blockhash can
                // no longer land.
                let current_height = match self.rpc.block_height().await {
                    Ok(h) => h,
                    Err(e) => {
                        warn!(channel_id = %row.channel_id, error = %e, "refund worker: block height fetch failed");
                        return;
                    }
                };
                if current_height <= last_valid_block_height {
                    // Still alive — rebroadcast the SAME persisted bytes
                    // (byte-identical ⇒ same signature ⇒ ledger dedupe holds).
                    use base64::Engine;
                    let b64 = base64::engine::general_purpose::STANDARD.encode(signed_tx);
                    self.broadcast(&row.channel_id, &b64).await;
                    return;
                }
                // Conclusively dead: history-negative AND blockhash expired.
                if row.attempts >= MAX_CLAIM_ATTEMPTS {
                    error!(
                        channel_id = %row.channel_id,
                        attempts = row.attempts,
                        "refund worker: retry attempts exhausted — holding"
                    );
                    self.hold_with_alert(&row.channel_id, RefundStatus::InFlight)
                        .await;
                    return;
                }
                let Some((signed, new_height)) = self.sign_fresh(row, RefundStatus::InFlight).await
                else {
                    return;
                };
                match resign_refund(
                    &self.pool,
                    &row.channel_id,
                    tx_signature,
                    &signed.wire_bytes,
                    &signed.signature_b58,
                    new_height,
                )
                .await
                {
                    Ok(true) => self.broadcast(&row.channel_id, &signed.base64_tx).await,
                    Ok(false) => {
                        // A peer concluded death first and re-signed — its
                        // bytes are authoritative; ours are discarded.
                        metrics::counter!("solvela_channel_refund_claim_lost_total").increment(1);
                    }
                    Err(e) => {
                        warn!(channel_id = %row.channel_id, error = %e, "refund worker: re-sign CAS failed")
                    }
                }
            }
            Err(e) => {
                warn!(channel_id = %row.channel_id, error = %e, "refund worker: signature status check failed");
            }
        }
    }
}

/// Spawn the refund worker loop (the claim-processor shape: interval tick +
/// graceful shutdown). Spawned in `main.rs` gated on DB-pool presence ONLY.
pub fn start_refund_worker(
    worker: RefundWorker,
    poll_interval: std::time::Duration,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(poll_interval);
        info!(
            poll_interval_secs = poll_interval.as_secs(),
            "channel refund worker started (pool-gated; independent of channel.enabled)"
        );
        loop {
            tokio::select! {
                _ = interval.tick() => worker.sweep().await,
                _ = shutdown_rx.changed() => {
                    info!("channel refund worker shutting down gracefully");
                    break;
                }
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- pure money math (no DB) ---------------------------------------------

    /// THE regression the disbursement formula is pinned to (plan §3):
    /// deposited = 50_000, the agent signed quotes summing to last = 12_600,
    /// but the ACTUAL realized obligation is 3_780 (chat: actual < quote) and
    /// nothing is settled. The refund MUST be `deposited - realized` = 46_220 —
    /// NOT 50_000 (`deposited - settled`, the A1 over-refund: free service plus
    /// the money back) and NOT 37_400 (`deposited - last`, the #600 strand:
    /// the 8_820 quote-vs-actual gap lost forever).
    #[test]
    fn refundable_regression_deposited_minus_realized() {
        let deposited = 50_000u64;
        let last = 12_600u64;
        let realized = 3_780u64;
        let settled = 0u64;

        let refund = compute_refundable(deposited, realized).unwrap();
        assert_eq!(refund, 46_220);
        assert_ne!(
            refund,
            deposited - settled,
            "must never refund deposited - settled (A1 over-refund)"
        );
        assert_ne!(
            refund,
            deposited - last,
            "must never disburse deposited - last (#600 strand)"
        );
    }

    #[test]
    fn refundable_is_deposited_minus_realized() {
        // Nothing realized → full deposit back; fully realized → zero, never
        // negative.
        assert_eq!(compute_refundable(50_000, 0).unwrap(), 50_000);
        assert_eq!(compute_refundable(50_000, 50_000).unwrap(), 0);
    }

    #[test]
    fn refundable_fails_closed_when_realized_exceeds_deposited() {
        // A corrupt row must NOT wrap into a huge refund.
        assert!(matches!(
            compute_refundable(100, 101),
            Err(ChannelRefundError::RealizedExceedsDeposited {
                deposited: 100,
                realized: 101
            })
        ));
        assert!(compute_refundable(0, u64::MAX).is_err());
    }

    #[test]
    fn refundable_never_exceeds_deposited_minus_realized() {
        // Exhaustive small sweep: always exactly deposited - realized, never
        // more (no path can over-refund).
        for deposited in 0u64..200 {
            for realized in 0u64..=deposited {
                assert_eq!(
                    compute_refundable(deposited, realized).unwrap(),
                    deposited - realized
                );
            }
        }
    }

    #[test]
    fn refund_status_strings_match_db_check() {
        assert_eq!(RefundStatus::Reserved.as_str(), "reserved");
        assert_eq!(RefundStatus::InFlight.as_str(), "in_flight");
        assert_eq!(RefundStatus::Confirmed.as_str(), "confirmed");
        assert_eq!(RefundStatus::Held.as_str(), "held");
    }

    // -- DB-backed tests (skip when no DATABASE_URL — channels.rs precedent) --

    use crate::channels::{
        self, create_channel, persist_voucher_and_advance, ChannelStatus, NewChannel, VoucherRecord,
    };
    use base64::Engine as _;
    use sqlx::PgPool;

    async fn db() -> Option<PgPool> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let pool = PgPool::connect(&url).await.ok()?;
        sqlx::migrate!("../../migrations")
            .run(&pool)
            .await
            .expect("migrations must apply");
        Some(pool)
    }

    /// A per-test ISOLATED database (dropped + recreated on every run). The
    /// worker's sweep and the daily-cap SUM are table-global, so tests that
    /// exercise them cannot share the common test database with concurrently
    /// running channel tests (a parallel test's reservation would be swept by
    /// THIS test's worker and poison its broadcast-count assertions).
    /// Assumes a query-string-free DATABASE_URL (the dev-harness form).
    async fn isolated_db(name: &str) -> Option<PgPool> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let admin = PgPool::connect(&url).await.ok()?;
        let db = format!("solvela_isolated_{name}");
        // Test-authored fixed identifiers, not user input — safe to assert.
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "DROP DATABASE IF EXISTS {db} WITH (FORCE)"
        )))
        .execute(&admin)
        .await
        .expect("drop isolated test db");
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {db}")))
            .execute(&admin)
            .await
            .expect("create isolated test db");
        let (base, _) = url.rsplit_once('/')?;
        let pool = PgPool::connect(&format!("{base}/{db}")).await.ok()?;
        sqlx::migrate!("../../migrations")
            .run(&pool)
            .await
            .expect("migrations must apply");
        Some(pool)
    }

    fn fresh_channel_id() -> String {
        let mut bytes = [0u8; 32];
        bytes[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        bytes[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        bs58::encode(bytes).into_string()
    }

    fn agent_wallet() -> String {
        "9noXzpXnkyEcKF3AeXqUHTdR59V5uvrRBUo9bwsHaByz".to_string()
    }

    const TEST_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

    fn new_channel(channel_id: &str, deposited: u64) -> NewChannel {
        NewChannel {
            channel_id: channel_id.to_string(),
            agent_wallet: agent_wallet(),
            session_key: agent_wallet(),
            provider: "RecipientWallet111111111111111111111111111111".to_string(),
            mint: TEST_MINT.to_string(),
            deposited_atomic: deposited,
            expiry_slot: Some(1_000_750),
            funding_tx_sig: Some(format!("sig-refund-{channel_id}")),
        }
    }

    /// A test claim with no cap.
    fn test_claim<'a>(
        channel_id: &'a str,
        signed_tx: &'a [u8],
        tx_signature: &'a str,
    ) -> RefundClaim<'a> {
        RefundClaim {
            channel_id,
            signed_tx,
            tx_signature,
            last_valid_block_height: 1_000,
            amount_atomic: 46_220,
            daily_cap_atomic: None,
        }
    }

    /// Draw the plan's pinned shape: last = 12_600, realized = 3_780.
    async fn draw_pinned_shape(pool: &PgPool, cid: &str) {
        persist_voucher_and_advance(
            pool,
            &VoucherRecord {
                channel_id: cid,
                cumulative_atomic: 12_600,
                call_cost_atomic: 12_600,
                realized_advance_atomic: 3_780,
                expiry_slot: 1_000_750,
                nonce: 1,
                request_digest: &[0x22; 32],
                signature: &[0x33; 64],
            },
        )
        .await
        .expect("draw persists");
    }

    /// Direct single-row SELECT — deliberately NOT the production
    /// `fetch_refunds` sweep query: that one is LIMIT-32 paged, and on the
    /// persistent dev Postgres the accumulated rows of prior runs push a fresh
    /// reservation past the page, flaking the `.expect()`s here.
    async fn load_refund_row(pool: &PgPool, cid: &str) -> Option<RefundRow> {
        #[allow(clippy::type_complexity)]
        let row: Option<(
            String,
            i64,
            String,
            String,
            String,
            Option<String>,
            Option<Vec<u8>>,
            Option<i64>,
            i32,
            i64,
        )> = sqlx::query_as(
            "SELECT channel_id, amount_atomic, destination_wallet, mint, status,
                    tx_signature, signed_tx, last_valid_block_height, attempts,
                    EXTRACT(EPOCH FROM (NOW() - updated_at))::BIGINT AS age_secs
               FROM channel_refunds
              WHERE channel_id = $1",
        )
        .bind(cid)
        .fetch_optional(pool)
        .await
        .unwrap();
        row.map(|r| RefundRow {
            channel_id: r.0,
            amount_atomic: u64::try_from(r.1).unwrap(),
            destination_wallet: r.2,
            mint: r.3,
            status: r.4,
            tx_signature: r.5,
            signed_tx: r.6,
            last_valid_block_height: r.7.map(|h| u64::try_from(h).unwrap()),
            attempts: r.8,
            age_secs: r.9,
        })
    }

    #[tokio::test]
    async fn close_freezes_reservation_in_one_transaction() {
        let Some(pool) = db().await else { return };
        let cid = fresh_channel_id();
        create_channel(&pool, &new_channel(&cid, 50_000))
            .await
            .unwrap();
        draw_pinned_shape(&pool, &cid).await;

        let outcome = close_channel_and_reserve_refund(&pool, &cid)
            .await
            .expect("close succeeds");
        assert_eq!(outcome.channel_status, "closing");
        assert_eq!(outcome.refundable_atomic, 46_220, "deposited - realized");
        assert_eq!(outcome.refund_status, "reserved");
        assert_eq!(outcome.tx_signature, None);

        // The frozen tuple: exact amount, DB agent_wallet, the CHANNEL's mint.
        let row = load_refund_row(&pool, &cid).await.expect("reservation row");
        assert_eq!(row.amount_atomic, 46_220);
        assert_eq!(row.destination_wallet, agent_wallet());
        assert_eq!(row.mint, TEST_MINT);
        assert_eq!(row.status, "reserved");

        // The channel flipped in the same transaction.
        let ch = channels::load_channel(&pool, &cid).await.unwrap().unwrap();
        assert_eq!(ch.status, "closing");
    }

    #[tokio::test]
    async fn close_is_idempotent_and_serves_as_the_status_poll() {
        let Some(pool) = db().await else { return };
        let cid = fresh_channel_id();
        create_channel(&pool, &new_channel(&cid, 50_000))
            .await
            .unwrap();
        draw_pinned_shape(&pool, &cid).await;

        let first = close_channel_and_reserve_refund(&pool, &cid).await.unwrap();
        let second = close_channel_and_reserve_refund(&pool, &cid).await.unwrap();
        // The reservation is frozen once; the re-close reports, never re-freezes.
        assert_eq!(second.refundable_atomic, first.refundable_atomic);
        assert_eq!(second.channel_status, "closing");
        assert_eq!(second.refund_status, "reserved");
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM channel_refunds WHERE channel_id = $1")
                .bind(&cid)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 1, "exactly one reservation row, ever");
    }

    #[tokio::test]
    async fn close_missing_channel_is_channel_not_found() {
        let Some(pool) = db().await else { return };
        let err = close_channel_and_reserve_refund(&pool, &fresh_channel_id())
            .await
            .expect_err("unknown channel must fail");
        assert!(matches!(
            err,
            ChannelRefundError::Repo(ChannelRepoError::ChannelNotFound)
        ));
    }

    #[tokio::test]
    async fn legacy_closing_channel_gets_reservation_on_reclose() {
        // A v0 channel already `closing` (pre-017: close recorded the
        // obligation with no reservation table). The idempotent re-close
        // ensures the reservation — frozen from the same tuple.
        let Some(pool) = db().await else { return };
        let cid = fresh_channel_id();
        create_channel(&pool, &new_channel(&cid, 50_000))
            .await
            .unwrap();
        draw_pinned_shape(&pool, &cid).await;
        channels::set_status(&pool, &cid, ChannelStatus::Closing)
            .await
            .unwrap();
        // Simulate the pre-017 state: no reservation row exists.
        sqlx::query("DELETE FROM channel_refunds WHERE channel_id = $1")
            .bind(&cid)
            .execute(&pool)
            .await
            .unwrap();

        let outcome = close_channel_and_reserve_refund(&pool, &cid).await.unwrap();
        assert_eq!(outcome.channel_status, "closing");
        assert_eq!(outcome.refundable_atomic, 46_220);
        let row = load_refund_row(&pool, &cid)
            .await
            .expect("reservation ensured");
        assert_eq!(row.amount_atomic, 46_220);
    }

    /// Invariant-11 concurrency regression (plan §6c): a draw in flight and a
    /// concurrent close. EITHER the frozen amount reflects the committed draw
    /// OR the draw recorded nothing — in both interleavings
    /// `channel_refunds.amount_atomic == deposited - realized_final`.
    #[tokio::test]
    async fn close_vs_inflight_draw_race_never_over_refunds() {
        let Some(pool) = db().await else { return };
        let cid = fresh_channel_id();
        let deposited = 50_000u64;
        create_channel(&pool, &new_channel(&cid, deposited))
            .await
            .unwrap();

        let draw_pool = pool.clone();
        let close_pool = pool.clone();
        let draw_cid = cid.clone();
        let close_cid = cid.clone();
        let draw = tokio::spawn(async move {
            persist_voucher_and_advance(
                &draw_pool,
                &VoucherRecord {
                    channel_id: &draw_cid,
                    cumulative_atomic: 12_600,
                    call_cost_atomic: 12_600,
                    realized_advance_atomic: 3_780,
                    expiry_slot: 1_000_750,
                    nonce: 1,
                    request_digest: &[0x22; 32],
                    signature: &[0x33; 64],
                },
            )
            .await
        });
        let close =
            tokio::spawn(
                async move { close_channel_and_reserve_refund(&close_pool, &close_cid).await },
            );

        let draw_result = draw.await.unwrap();
        let close_outcome = close.await.unwrap().expect("close succeeds");

        let ch = channels::load_channel(&pool, &cid).await.unwrap().unwrap();
        let row = load_refund_row(&pool, &cid).await.expect("reservation row");

        // The one invariant that must hold in EVERY interleaving.
        assert_eq!(
            row.amount_atomic,
            deposited - ch.realized_atomic,
            "frozen refund must equal deposited - realized_final"
        );
        match draw_result {
            Ok(()) => {
                // Draw won the row lock: the flip read the post-draw realized.
                assert_eq!(ch.realized_atomic, 3_780);
                assert_eq!(row.amount_atomic, 46_220);
            }
            Err(ChannelRepoError::AdvanceNotApplied) => {
                // Close won: the draw recorded NOTHING (bounded non-charge).
                assert_eq!(ch.realized_atomic, 0);
                assert_eq!(ch.last_voucher_cumulative_atomic, 0);
                assert_eq!(row.amount_atomic, deposited);
            }
            Err(other) => panic!("unexpected draw error: {other}"),
        }
        assert_eq!(close_outcome.refundable_atomic, row.amount_atomic);
    }

    /// Invariant-13 two-worker regression (escrow_queue double-claim shape):
    /// two concurrent claimers on ONE reservation — exactly one wins the CAS,
    /// and only the winner's bytes/signature are persisted.
    #[tokio::test]
    async fn two_concurrent_claims_have_exactly_one_winner() {
        let Some(pool) = db().await else { return };
        let cid = fresh_channel_id();
        create_channel(&pool, &new_channel(&cid, 50_000))
            .await
            .unwrap();
        draw_pinned_shape(&pool, &cid).await;
        close_channel_and_reserve_refund(&pool, &cid).await.unwrap();

        let (pool_a, pool_b) = (pool.clone(), pool.clone());
        let (cid_a, cid_b) = (cid.clone(), cid.clone());
        let a = tokio::spawn(async move {
            claim_refund_for_broadcast(
                &pool_a,
                &test_claim(&cid_a, b"worker-a-bytes", "sig-worker-a"),
            )
            .await
        });
        let b = tokio::spawn(async move {
            claim_refund_for_broadcast(
                &pool_b,
                &test_claim(&cid_b, b"worker-b-bytes", "sig-worker-b"),
            )
            .await
        });
        let ra = a.await.unwrap().unwrap();
        let rb = b.await.unwrap().unwrap();

        let winners = [&ra, &rb]
            .iter()
            .filter(|o| ***o == ClaimOutcome::Won)
            .count();
        assert_eq!(winners, 1, "exactly one signer per reservation");
        assert_eq!(
            [&ra, &rb]
                .iter()
                .filter(|o| ***o == ClaimOutcome::Lost)
                .count(),
            1,
            "the loser must know it lost (discards its bytes)"
        );

        // Only the winner's payload is persisted.
        let row = load_refund_row(&pool, &cid).await.unwrap();
        assert_eq!(row.status, "in_flight");
        assert_eq!(row.attempts, 1, "one claim, not two");
        let expected_sig = if ra == ClaimOutcome::Won {
            "sig-worker-a"
        } else {
            "sig-worker-b"
        };
        let expected_bytes: &[u8] = if ra == ClaimOutcome::Won {
            b"worker-a-bytes"
        } else {
            b"worker-b-bytes"
        };
        assert_eq!(row.tx_signature.as_deref(), Some(expected_sig));
        assert_eq!(row.signed_tx.as_deref(), Some(expected_bytes));
    }

    #[tokio::test]
    async fn claim_holds_when_daily_cap_would_be_exceeded() {
        let Some(pool) = isolated_db("daily_cap").await else {
            return;
        };
        let cid = fresh_channel_id();
        create_channel(&pool, &new_channel(&cid, 50_000))
            .await
            .unwrap();
        draw_pinned_shape(&pool, &cid).await;
        close_channel_and_reserve_refund(&pool, &cid).await.unwrap();

        let claim_with_cap = |cap: Option<u64>| RefundClaim {
            daily_cap_atomic: cap,
            ..test_claim(&cid, b"bytes", "sig-cap")
        };

        // One atomic unit of headroom short → CapExceeded (no flip, no payload).
        let outcome = claim_refund_for_broadcast(&pool, &claim_with_cap(Some(46_219)))
            .await
            .unwrap();
        assert_eq!(outcome, ClaimOutcome::CapExceeded);
        let row = load_refund_row(&pool, &cid).await.unwrap();
        assert_eq!(row.status, "reserved", "obligation retained, never dropped");
        assert_eq!(row.tx_signature, None);
        assert_eq!(row.attempts, 0);

        // Exactly at the cap → allowed (the check holds only when sum + amount
        // EXCEEDS the cap).
        let outcome = claim_refund_for_broadcast(&pool, &claim_with_cap(Some(46_220)))
            .await
            .unwrap();
        assert_eq!(outcome, ClaimOutcome::Won);
    }

    #[tokio::test]
    async fn resign_is_predicated_on_the_superseded_signature() {
        let Some(pool) = db().await else { return };
        let cid = fresh_channel_id();
        create_channel(&pool, &new_channel(&cid, 50_000))
            .await
            .unwrap();
        draw_pinned_shape(&pool, &cid).await;
        close_channel_and_reserve_refund(&pool, &cid).await.unwrap();
        assert_eq!(
            claim_refund_for_broadcast(&pool, &test_claim(&cid, b"v1", "sig-v1"))
                .await
                .unwrap(),
            ClaimOutcome::Won
        );

        // Winner path: predicate matches the current signature.
        assert!(resign_refund(&pool, &cid, "sig-v1", b"v2", "sig-v2", 2_000)
            .await
            .unwrap());
        // Loser path: a peer that concurrently concluded death still holds the
        // OLD signature in its predicate — 0 rows, bytes discarded.
        assert!(
            !resign_refund(&pool, &cid, "sig-v1", b"v2-loser", "sig-v2-loser", 2_000)
                .await
                .unwrap()
        );

        let row = load_refund_row(&pool, &cid).await.unwrap();
        assert_eq!(row.tx_signature.as_deref(), Some("sig-v2"));
        assert_eq!(row.signed_tx.as_deref(), Some(b"v2".as_slice()));
        assert_eq!(row.attempts, 2, "claim + one re-sign");
    }

    #[tokio::test]
    async fn confirm_closes_channel_in_the_same_transaction() {
        let Some(pool) = db().await else { return };
        let cid = fresh_channel_id();
        create_channel(&pool, &new_channel(&cid, 50_000))
            .await
            .unwrap();
        draw_pinned_shape(&pool, &cid).await;
        close_channel_and_reserve_refund(&pool, &cid).await.unwrap();
        assert_eq!(
            claim_refund_for_broadcast(&pool, &test_claim(&cid, b"v1", "sig-c"))
                .await
                .unwrap(),
            ClaimOutcome::Won
        );

        assert!(
            confirm_refund_and_close_channel(&pool, &cid, RefundStatus::InFlight)
                .await
                .unwrap()
        );
        let row = load_refund_row(&pool, &cid).await.unwrap();
        assert_eq!(row.status, "confirmed");
        let ch = channels::load_channel(&pool, &cid).await.unwrap().unwrap();
        assert_eq!(ch.status, "closed", "channel closes with the confirm CAS");

        // A peer's duplicate confirm is a benign no-op.
        assert!(
            !confirm_refund_and_close_channel(&pool, &cid, RefundStatus::InFlight)
                .await
                .unwrap()
        );
    }

    // -- worker lifecycle with a mock RPC (DB-gated) --------------------------

    struct MockRpc {
        balance: u64,
        /// Mutable so a test can advance the chain past `last_valid_block_height`
        /// (the conclusive-death trigger). Defaults to 500 (< the 1_000 the
        /// blockhash mock hands out — blockhash alive).
        block_height: std::sync::Mutex<u64>,
        /// What `signature_status(_, true)` reports. `None` inner = not found.
        status: std::sync::Mutex<Option<SignatureStatus>>,
        /// When `Some(text)`, `send_transaction` fails with that error text
        /// (drives the broadcast-classification arms).
        send_error: std::sync::Mutex<Option<String>>,
        sends: std::sync::Mutex<Vec<String>>,
    }

    impl MockRpc {
        fn new(balance: u64) -> Self {
            Self {
                balance,
                block_height: std::sync::Mutex::new(500),
                status: std::sync::Mutex::new(None),
                send_error: std::sync::Mutex::new(None),
                sends: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait::async_trait]
    impl RefundRpc for MockRpc {
        async fn usdc_balance(&self, _owner: &str, _mint: &str) -> Result<u64, X402Error> {
            Ok(self.balance)
        }
        async fn latest_blockhash_and_height(&self) -> Result<([u8; 32], u64), X402Error> {
            Ok(([7u8; 32], 1_000))
        }
        async fn block_height(&self) -> Result<u64, X402Error> {
            Ok(*self.block_height.lock().unwrap())
        }
        async fn signature_status(
            &self,
            _signature_b58: &str,
            search_history: bool,
        ) -> Result<Option<SignatureStatus>, X402Error> {
            assert!(
                search_history,
                "recovery/death checks MUST search transaction history (HALT 4)"
            );
            Ok(self.status.lock().unwrap().clone())
        }
        async fn send_transaction(&self, base64_tx: &str) -> Result<String, X402Error> {
            if let Some(err) = self.send_error.lock().unwrap().clone() {
                return Err(X402Error::Rpc(err));
            }
            self.sends.lock().unwrap().push(base64_tx.to_string());
            Ok("mock-broadcast-sig".to_string())
        }
    }

    /// A worker whose signer key IS the recipient wallet (the FIX 6 shape).
    fn test_worker(pool: PgPool, rpc: Arc<MockRpc>) -> RefundWorker {
        use ed25519_dalek::SigningKey;
        let signing_key = SigningKey::from_bytes(&[1u8; 32]);
        let mut keypair = [0u8; 64];
        keypair[..32].copy_from_slice(&signing_key.to_bytes());
        keypair[32..].copy_from_slice(signing_key.verifying_key().as_bytes());
        let key_b58 = bs58::encode(keypair).into_string();
        let recipient = bs58::encode(signing_key.verifying_key().to_bytes()).into_string();
        let fee_pool = solvela_x402::fee_payer::FeePayerPool::from_keys(&[key_b58])
            .expect("test fee payer pool");
        RefundWorker::new(pool, rpc, Some(Arc::new(fee_pool)), recipient, None)
    }

    /// The flag-off-drain regression (plan §5.C.2 / round-2 finding 7): the
    /// worker drains a frozen reservation end-to-end with `channel.enabled`
    /// NEVER consulted — structurally, [`RefundWorker`] has no flag input at
    /// all (this test constructs it from pool + RPC + key only), so an
    /// incident flag-flip or rollback window cannot freeze owed money.
    #[tokio::test]
    async fn worker_drains_reservation_independent_of_channel_flag() {
        let Some(pool) = isolated_db("worker_drain").await else {
            return;
        };
        let cid = fresh_channel_id();
        create_channel(&pool, &new_channel(&cid, 50_000))
            .await
            .unwrap();
        draw_pinned_shape(&pool, &cid).await;
        close_channel_and_reserve_refund(&pool, &cid).await.unwrap();

        let rpc = Arc::new(MockRpc::new(1_000_000));
        let worker = test_worker(pool.clone(), rpc.clone());

        // Sweep 1 (after the attempts=0 backoff of 1s): claim + broadcast.
        tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
        worker.sweep().await;
        let row = load_refund_row(&pool, &cid).await.unwrap();
        assert_eq!(row.status, "in_flight");
        assert_eq!(row.attempts, 1);
        let persisted = row
            .signed_tx
            .clone()
            .expect("claim CAS persisted the bytes");
        let sends = rpc.sends.lock().unwrap().clone();
        assert_eq!(sends.len(), 1, "exactly one broadcast");
        assert_eq!(
            sends[0],
            base64::engine::general_purpose::STANDARD.encode(&persisted),
            "the broadcast bytes ARE the persisted bytes"
        );

        // The chain confirms; sweep 2 (after the attempts=1 backoff of 2s)
        // lands the confirm CAS and closes the channel.
        *rpc.status.lock().unwrap() = Some(SignatureStatus {
            err: None,
            confirmation_status: Some("confirmed".to_string()),
        });
        tokio::time::sleep(std::time::Duration::from_millis(2_100)).await;
        worker.sweep().await;

        let row = load_refund_row(&pool, &cid).await.unwrap();
        assert_eq!(row.status, "confirmed");
        let ch = channels::load_channel(&pool, &cid).await.unwrap().unwrap();
        assert_eq!(ch.status, "closed");
        assert_eq!(
            rpc.sends.lock().unwrap().len(),
            1,
            "a confirmed refund is never rebroadcast"
        );
    }

    #[tokio::test]
    async fn worker_completes_zero_amount_refund_without_a_transaction() {
        let Some(pool) = isolated_db("worker_zero").await else {
            return;
        };
        let cid = fresh_channel_id();
        create_channel(&pool, &new_channel(&cid, 12_600))
            .await
            .unwrap();
        // Fully drawn AND fully realized: deposited == realized ⇒ refund 0.
        persist_voucher_and_advance(
            &pool,
            &VoucherRecord {
                channel_id: &cid,
                cumulative_atomic: 12_600,
                call_cost_atomic: 12_600,
                realized_advance_atomic: 12_600,
                expiry_slot: 1_000_750,
                nonce: 1,
                request_digest: &[0x22; 32],
                signature: &[0x33; 64],
            },
        )
        .await
        .unwrap();
        let outcome = close_channel_and_reserve_refund(&pool, &cid).await.unwrap();
        assert_eq!(outcome.refundable_atomic, 0);

        let rpc = Arc::new(MockRpc::new(1_000_000));
        let worker = test_worker(pool.clone(), rpc.clone());
        tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
        worker.sweep().await;

        let row = load_refund_row(&pool, &cid).await.unwrap();
        assert_eq!(row.status, "confirmed");
        assert_eq!(row.tx_signature, None, "no transaction for a zero refund");
        assert!(rpc.sends.lock().unwrap().is_empty(), "nothing broadcast");
        let ch = channels::load_channel(&pool, &cid).await.unwrap().unwrap();
        assert_eq!(ch.status, "closed");
    }

    #[tokio::test]
    async fn worker_retains_reservation_on_insufficient_balance() {
        let Some(pool) = isolated_db("worker_balance").await else {
            return;
        };
        let cid = fresh_channel_id();
        create_channel(&pool, &new_channel(&cid, 50_000))
            .await
            .unwrap();
        draw_pinned_shape(&pool, &cid).await;
        close_channel_and_reserve_refund(&pool, &cid).await.unwrap();

        // Operational wallet holds less than the refund → NEVER a partial
        // send; the obligation is retained (and alerted), not abandoned.
        let rpc = Arc::new(MockRpc::new(5));
        let worker = test_worker(pool.clone(), rpc.clone());
        tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
        worker.sweep().await;

        let row = load_refund_row(&pool, &cid).await.unwrap();
        assert_eq!(row.status, "reserved");
        assert_eq!(row.attempts, 0);
        assert!(rpc.sends.lock().unwrap().is_empty(), "nothing broadcast");
    }

    // -- worker decision paths: death / rebroadcast / confirm (review item 11) --

    /// Seed an in_flight row via the real claim CAS (bytes "old-bytes",
    /// signature "sig-old", last_valid_block_height 1_000) so the in_flight
    /// handler's decision inputs are exactly production-shaped.
    async fn seed_in_flight(pool: &PgPool, cid: &str) {
        assert_eq!(
            claim_refund_for_broadcast(
                pool,
                &RefundClaim {
                    channel_id: cid,
                    signed_tx: b"old-bytes",
                    tx_signature: "sig-old",
                    last_valid_block_height: 1_000,
                    amount_atomic: 46_220,
                    daily_cap_atomic: None,
                },
            )
            .await
            .unwrap(),
            ClaimOutcome::Won
        );
    }

    /// Conclusive death (history-negative AND block height past
    /// last_valid_block_height) → the worker re-signs through the
    /// signature-predicated CAS and broadcasts the NEW bytes.
    #[tokio::test]
    async fn worker_resigns_after_conclusive_death() {
        let Some(pool) = isolated_db("death_resign").await else {
            return;
        };
        let cid = fresh_channel_id();
        create_channel(&pool, &new_channel(&cid, 50_000))
            .await
            .unwrap();
        draw_pinned_shape(&pool, &cid).await;
        close_channel_and_reserve_refund(&pool, &cid).await.unwrap();
        seed_in_flight(&pool, &cid).await;

        let rpc = Arc::new(MockRpc::new(1_000_000));
        *rpc.block_height.lock().unwrap() = 2_000; // past last_valid (1_000)
        let worker = test_worker(pool.clone(), rpc.clone());

        // attempts = 1 after the claim → backoff 2s before the row is due.
        tokio::time::sleep(std::time::Duration::from_millis(2_100)).await;
        worker.sweep().await;

        let row = load_refund_row(&pool, &cid).await.unwrap();
        assert_eq!(row.status, "in_flight");
        assert_eq!(row.attempts, 2, "claim + one re-sign");
        let new_sig = row.tx_signature.clone().expect("re-signed signature");
        assert_ne!(new_sig, "sig-old", "the dead signature must be superseded");
        let new_bytes = row.signed_tx.clone().expect("re-signed bytes");
        assert_ne!(new_bytes.as_slice(), b"old-bytes".as_slice());
        let sends = rpc.sends.lock().unwrap().clone();
        assert_eq!(sends.len(), 1, "exactly one broadcast of the NEW bytes");
        assert_eq!(
            sends[0],
            base64::engine::general_purpose::STANDARD.encode(&new_bytes),
            "the broadcast bytes ARE the re-signed persisted bytes"
        );
    }

    /// History-negative but the blockhash is still alive → the worker
    /// rebroadcasts the SAME persisted bytes (never re-signs).
    #[tokio::test]
    async fn worker_rebroadcasts_same_bytes_while_blockhash_alive() {
        let Some(pool) = isolated_db("rebroadcast").await else {
            return;
        };
        let cid = fresh_channel_id();
        create_channel(&pool, &new_channel(&cid, 50_000))
            .await
            .unwrap();
        draw_pinned_shape(&pool, &cid).await;
        close_channel_and_reserve_refund(&pool, &cid).await.unwrap();
        seed_in_flight(&pool, &cid).await;

        let rpc = Arc::new(MockRpc::new(1_000_000)); // block_height 500 < 1_000
        let worker = test_worker(pool.clone(), rpc.clone());
        tokio::time::sleep(std::time::Duration::from_millis(2_100)).await;
        worker.sweep().await;

        let row = load_refund_row(&pool, &cid).await.unwrap();
        assert_eq!(row.status, "in_flight");
        assert_eq!(row.attempts, 1, "no re-sign while the blockhash lives");
        assert_eq!(row.tx_signature.as_deref(), Some("sig-old"));
        let sends = rpc.sends.lock().unwrap().clone();
        assert_eq!(sends.len(), 1);
        assert_eq!(
            sends[0],
            base64::engine::general_purpose::STANDARD.encode(b"old-bytes"),
            "rebroadcast must reuse the persisted bytes verbatim"
        );
    }

    /// The history search finds the transaction confirmed → confirm CAS +
    /// channel closed, no further broadcast.
    #[tokio::test]
    async fn worker_confirms_in_flight_from_history_search() {
        let Some(pool) = isolated_db("confirm_path").await else {
            return;
        };
        let cid = fresh_channel_id();
        create_channel(&pool, &new_channel(&cid, 50_000))
            .await
            .unwrap();
        draw_pinned_shape(&pool, &cid).await;
        close_channel_and_reserve_refund(&pool, &cid).await.unwrap();
        seed_in_flight(&pool, &cid).await;

        let rpc = Arc::new(MockRpc::new(1_000_000));
        *rpc.status.lock().unwrap() = Some(SignatureStatus {
            err: None,
            confirmation_status: Some("confirmed".to_string()),
        });
        let worker = test_worker(pool.clone(), rpc.clone());
        tokio::time::sleep(std::time::Duration::from_millis(2_100)).await;
        worker.sweep().await;

        let row = load_refund_row(&pool, &cid).await.unwrap();
        assert_eq!(row.status, "confirmed");
        let ch = channels::load_channel(&pool, &cid).await.unwrap().unwrap();
        assert_eq!(ch.status, "closed");
        assert!(rpc.sends.lock().unwrap().is_empty(), "no broadcast needed");
    }

    /// A corrupt FROZEN tuple (unfixable by retry) → held with the entry alert,
    /// never a silent every-sweep retry loop (review item 12).
    #[tokio::test]
    async fn worker_holds_reservation_with_corrupt_frozen_tuple() {
        let Some(pool) = isolated_db("corrupt_tuple").await else {
            return;
        };
        let cid = fresh_channel_id();
        create_channel(&pool, &new_channel(&cid, 50_000))
            .await
            .unwrap();
        draw_pinned_shape(&pool, &cid).await;
        close_channel_and_reserve_refund(&pool, &cid).await.unwrap();
        sqlx::query("UPDATE channel_refunds SET destination_wallet = $1 WHERE channel_id = $2")
            .bind("not base58 0OIl")
            .bind(&cid)
            .execute(&pool)
            .await
            .unwrap();

        let rpc = Arc::new(MockRpc::new(1_000_000));
        let worker = test_worker(pool.clone(), rpc.clone());
        tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
        worker.sweep().await;

        let row = load_refund_row(&pool, &cid).await.unwrap();
        assert_eq!(row.status, "held", "corrupt frozen tuple is conclusive");
        assert!(rpc.sends.lock().unwrap().is_empty(), "nothing broadcast");
    }

    // -- broadcast classification (review item 13) ---------------------------

    /// A deterministic program rejection on broadcast → these bytes can never
    /// land → held (conclusive execution failure).
    #[tokio::test]
    async fn worker_holds_on_deterministic_broadcast_rejection() {
        let Some(pool) = isolated_db("broadcast_reject").await else {
            return;
        };
        let cid = fresh_channel_id();
        create_channel(&pool, &new_channel(&cid, 50_000))
            .await
            .unwrap();
        draw_pinned_shape(&pool, &cid).await;
        close_channel_and_reserve_refund(&pool, &cid).await.unwrap();

        let rpc = Arc::new(MockRpc::new(1_000_000));
        *rpc.send_error.lock().unwrap() = Some(
            r#"{"code":-32002,"message":"Transaction simulation failed: Error processing Instruction 1: custom program error: 0x1"}"#
                .to_string(),
        );
        let worker = test_worker(pool.clone(), rpc.clone());
        tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
        worker.sweep().await;

        let row = load_refund_row(&pool, &cid).await.unwrap();
        assert_eq!(row.status, "held");
    }

    /// A transient broadcast failure (transport / expired blockhash) → the row
    /// stays in_flight; the recovery path retries safely next sweep.
    #[tokio::test]
    async fn worker_retains_in_flight_on_transient_broadcast_failure() {
        let Some(pool) = isolated_db("broadcast_transient").await else {
            return;
        };
        let cid = fresh_channel_id();
        create_channel(&pool, &new_channel(&cid, 50_000))
            .await
            .unwrap();
        draw_pinned_shape(&pool, &cid).await;
        close_channel_and_reserve_refund(&pool, &cid).await.unwrap();

        let rpc = Arc::new(MockRpc::new(1_000_000));
        *rpc.send_error.lock().unwrap() = Some("blockhash not found".to_string());
        let worker = test_worker(pool.clone(), rpc.clone());
        tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
        worker.sweep().await;

        let row = load_refund_row(&pool, &cid).await.unwrap();
        assert_eq!(row.status, "in_flight", "transient failure never abandons");
        assert_eq!(row.attempts, 1);
    }

    /// An "already processed" broadcast response means the transfer LANDED (a
    /// prior crashed broadcast) — treated as success (no hold); the next
    /// sweep's history search confirms it.
    #[tokio::test]
    async fn worker_treats_already_processed_broadcast_as_landed() {
        let Some(pool) = isolated_db("broadcast_already").await else {
            return;
        };
        let cid = fresh_channel_id();
        create_channel(&pool, &new_channel(&cid, 50_000))
            .await
            .unwrap();
        draw_pinned_shape(&pool, &cid).await;
        close_channel_and_reserve_refund(&pool, &cid).await.unwrap();

        let rpc = Arc::new(MockRpc::new(1_000_000));
        *rpc.send_error.lock().unwrap() =
            Some("Transaction has already been processed".to_string());
        let worker = test_worker(pool.clone(), rpc.clone());
        tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
        worker.sweep().await;

        let row = load_refund_row(&pool, &cid).await.unwrap();
        assert_eq!(
            row.status, "in_flight",
            "landed, awaiting the confirm sweep"
        );

        // The confirm sweep lands it.
        *rpc.send_error.lock().unwrap() = None;
        *rpc.status.lock().unwrap() = Some(SignatureStatus {
            err: None,
            confirmation_status: Some("finalized".to_string()),
        });
        tokio::time::sleep(std::time::Duration::from_millis(2_100)).await;
        worker.sweep().await;
        let row = load_refund_row(&pool, &cid).await.unwrap();
        assert_eq!(row.status, "confirmed");
    }

    /// Deterministic close-overtakes-draw interleaving (review item 15): the
    /// close's flip commits while a draw's persist is BLOCKED on the row lock —
    /// the draw must lose (AdvanceNotApplied), record NOTHING, and the frozen
    /// refund must be the full pre-draw amount. Complements the stochastic
    /// race test above.
    #[tokio::test]
    async fn close_overtakes_blocked_draw_which_records_nothing() {
        let Some(pool) = db().await else { return };
        let cid = fresh_channel_id();
        let deposited = 50_000u64;
        create_channel(&pool, &new_channel(&cid, deposited))
            .await
            .unwrap();

        // Take the channels row lock exactly as close's flip does, and HOLD it.
        let mut close_tx = pool.begin().await.unwrap();
        let flipped = sqlx::query(
            "UPDATE channels SET status = 'closing', updated_at = NOW()
              WHERE channel_id = $1 AND status = 'open'",
        )
        .bind(&cid)
        .execute(&mut *close_tx)
        .await
        .unwrap();
        assert_eq!(flipped.rows_affected(), 1);

        // The draw's persist now BLOCKS on the row lock.
        let draw_pool = pool.clone();
        let draw_cid = cid.clone();
        let draw = tokio::spawn(async move {
            persist_voucher_and_advance(
                &draw_pool,
                &VoucherRecord {
                    channel_id: &draw_cid,
                    cumulative_atomic: 12_600,
                    call_cost_atomic: 12_600,
                    realized_advance_atomic: 3_780,
                    expiry_slot: 1_000_750,
                    nonce: 1,
                    request_digest: &[0x22; 32],
                    signature: &[0x33; 64],
                },
            )
            .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(
            !draw.is_finished(),
            "the draw must be blocked on the close's row lock"
        );

        // The flip commits first — the draw re-evaluates and MUST lose.
        close_tx.commit().await.unwrap();
        let draw_result = draw.await.unwrap();
        assert!(matches!(
            draw_result,
            Err(ChannelRepoError::AdvanceNotApplied)
        ));

        // Nothing recorded; the (idempotent) reservation freezes the FULL deposit.
        let ch = channels::load_channel(&pool, &cid).await.unwrap().unwrap();
        assert_eq!(ch.realized_atomic, 0);
        assert_eq!(ch.last_voucher_cumulative_atomic, 0);
        let outcome = close_channel_and_reserve_refund(&pool, &cid).await.unwrap();
        assert_eq!(outcome.refundable_atomic, deposited);
    }

    /// Review item 18: a raw disaster-recovery RE-APPLY of migration 017 must
    /// not corrupt a legitimate post-017 `realized = 0, last > 0` row (the
    /// fully-discounted chat-draw shape) — the backfill is guarded on
    /// column-CREATION, not on a value predicate.
    #[tokio::test]
    async fn migration_017_reapply_does_not_corrupt_realized_zero_rows() {
        let Some(pool) = isolated_db("migration_reapply").await else {
            return;
        };
        let cid = fresh_channel_id();
        create_channel(&pool, &new_channel(&cid, 50_000))
            .await
            .unwrap();
        // A legitimate post-017 shape: last advanced, realized still 0
        // (CHECK chain holds: 0 <= 0 <= 5_000 <= 50_000).
        sqlx::query(
            "UPDATE channels SET last_voucher_cumulative_atomic = 5000 WHERE channel_id = $1",
        )
        .bind(&cid)
        .execute(&pool)
        .await
        .unwrap();

        sqlx::raw_sql(sqlx::AssertSqlSafe(
            include_str!("../../../migrations/017_channel_realized_and_refunds.sql").to_string(),
        ))
        .execute(&pool)
        .await
        .expect("raw re-apply must succeed");

        let ch = channels::load_channel(&pool, &cid).await.unwrap().unwrap();
        assert_eq!(
            ch.realized_atomic, 0,
            "re-applied backfill must NOT inflate a legitimate realized=0 row"
        );
        assert_eq!(ch.last_voucher_cumulative_atomic, 5_000);
    }

    // -- round-2 mutation-proven gaps -----------------------------------------

    /// FIX-6 payer==payee guard (review item 1 — deleting the guard left every
    /// prior test green): a worker whose recipient_wallet does NOT match the
    /// fee-payer key's pubkey must refuse to sign — the row stays reserved and
    /// nothing is ever broadcast.
    #[tokio::test]
    async fn worker_refuses_to_sign_when_payer_is_not_recipient() {
        let Some(pool) = isolated_db("fix6_mismatch").await else {
            return;
        };
        let cid = fresh_channel_id();
        create_channel(&pool, &new_channel(&cid, 50_000))
            .await
            .unwrap();
        draw_pinned_shape(&pool, &cid).await;
        close_channel_and_reserve_refund(&pool, &cid).await.unwrap();

        // Key from seed [1u8;32], recipient = a DIFFERENT key's pubkey.
        use ed25519_dalek::SigningKey;
        let signing_key = SigningKey::from_bytes(&[1u8; 32]);
        let mut keypair = [0u8; 64];
        keypair[..32].copy_from_slice(&signing_key.to_bytes());
        keypair[32..].copy_from_slice(signing_key.verifying_key().as_bytes());
        let fee_pool =
            solvela_x402::fee_payer::FeePayerPool::from_keys(
                &[bs58::encode(keypair).into_string()],
            )
            .unwrap();
        let other = SigningKey::from_bytes(&[2u8; 32]);
        let wrong_recipient = bs58::encode(other.verifying_key().to_bytes()).into_string();

        let rpc = Arc::new(MockRpc::new(1_000_000));
        let worker = RefundWorker::new(
            pool.clone(),
            rpc.clone(),
            Some(Arc::new(fee_pool)),
            wrong_recipient,
            None,
        );
        tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
        worker.sweep().await;

        let row = load_refund_row(&pool, &cid).await.unwrap();
        assert_eq!(row.status, "reserved", "guard must refuse, never abandon");
        assert_eq!(row.tx_signature, None, "no bytes may be persisted");
        assert!(rpc.sends.lock().unwrap().is_empty(), "nothing broadcast");
    }

    /// Landed-but-FAILED execution (review item 3): err present even alongside
    /// confirmationStatus "confirmed" → held, NEVER confirmed — a bug here
    /// closes the channel while no USDC moved.
    #[tokio::test]
    async fn worker_holds_landed_with_error_never_confirms() {
        let Some(pool) = isolated_db("landed_error").await else {
            return;
        };
        let cid = fresh_channel_id();
        create_channel(&pool, &new_channel(&cid, 50_000))
            .await
            .unwrap();
        draw_pinned_shape(&pool, &cid).await;
        close_channel_and_reserve_refund(&pool, &cid).await.unwrap();
        seed_in_flight(&pool, &cid).await;

        let rpc = Arc::new(MockRpc::new(1_000_000));
        *rpc.status.lock().unwrap() = Some(SignatureStatus {
            err: Some(r#"{"InstructionError":[1,{"Custom":1}]}"#.to_string()),
            confirmation_status: Some("confirmed".to_string()),
        });
        let worker = test_worker(pool.clone(), rpc.clone());
        tokio::time::sleep(std::time::Duration::from_millis(2_100)).await;
        worker.sweep().await;

        let row = load_refund_row(&pool, &cid).await.unwrap();
        assert_eq!(
            row.status, "held",
            "landed-with-error is conclusive failure"
        );
        let ch = channels::load_channel(&pool, &cid).await.unwrap().unwrap();
        assert_ne!(
            ch.status, "closed",
            "the channel must NOT close on a failed refund"
        );
    }

    /// Retry exhaustion (review item 4): at MAX_CLAIM_ATTEMPTS a conclusively
    /// dead in_flight row goes to held instead of re-signing forever.
    #[tokio::test]
    async fn worker_holds_in_flight_at_max_attempts() {
        let Some(pool) = isolated_db("exhaustion").await else {
            return;
        };
        let cid = fresh_channel_id();
        create_channel(&pool, &new_channel(&cid, 50_000))
            .await
            .unwrap();
        draw_pinned_shape(&pool, &cid).await;
        close_channel_and_reserve_refund(&pool, &cid).await.unwrap();
        seed_in_flight(&pool, &cid).await;
        sqlx::query("UPDATE channel_refunds SET attempts = $1 WHERE channel_id = $2")
            .bind(MAX_CLAIM_ATTEMPTS)
            .bind(&cid)
            .execute(&pool)
            .await
            .unwrap();

        let rpc = Arc::new(MockRpc::new(1_000_000));
        *rpc.block_height.lock().unwrap() = 2_000; // conclusively dead
        let worker = test_worker(pool.clone(), rpc.clone());
        // attempts = MAX → backoff is capped at 300s; make the row due by
        // backdating its last touch instead of sleeping.
        sqlx::query(
            "UPDATE channel_refunds SET updated_at = NOW() - INTERVAL '10 minutes'
              WHERE channel_id = $1",
        )
        .bind(&cid)
        .execute(&pool)
        .await
        .unwrap();
        worker.sweep().await;

        let row = load_refund_row(&pool, &cid).await.unwrap();
        assert_eq!(row.status, "held");
        assert_eq!(row.attempts, MAX_CLAIM_ATTEMPTS, "no further re-sign");
        assert!(rpc.sends.lock().unwrap().is_empty(), "nothing broadcast");
    }

    /// Daily-cap held-exclusion (review item 5 — the `AND status <> 'held'`
    /// predicate survived mutation): a held row with a huge frozen amount and
    /// a recent broadcast anchor must NOT throttle a fresh legitimate claim.
    #[tokio::test]
    async fn daily_cap_excludes_held_rows_from_the_window() {
        let Some(pool) = isolated_db("cap_held_exclusion").await else {
            return;
        };
        // Channel A: a held reservation with a large amount inside the 24h
        // window (no USDC ever left the wallet on any held path).
        let cid_held = fresh_channel_id();
        create_channel(&pool, &new_channel(&cid_held, 50_000))
            .await
            .unwrap();
        close_channel_and_reserve_refund(&pool, &cid_held)
            .await
            .unwrap();
        sqlx::query(
            "UPDATE channel_refunds
                SET status = 'held', amount_atomic = 1000000, first_broadcast_at = NOW()
              WHERE channel_id = $1",
        )
        .bind(&cid_held)
        .execute(&pool)
        .await
        .unwrap();

        // Channel B: a fresh reservation whose claim would be blocked if the
        // held row counted (1_000_000 + 46_220 > 50_000).
        let cid = fresh_channel_id();
        create_channel(&pool, &new_channel(&cid, 50_000))
            .await
            .unwrap();
        draw_pinned_shape(&pool, &cid).await;
        close_channel_and_reserve_refund(&pool, &cid).await.unwrap();

        let outcome = claim_refund_for_broadcast(
            &pool,
            &RefundClaim {
                daily_cap_atomic: Some(50_000),
                ..test_claim(&cid, b"bytes", "sig-held-excl")
            },
        )
        .await
        .unwrap();
        assert_eq!(
            outcome,
            ClaimOutcome::Won,
            "a held row's phantom amount must not consume cap headroom"
        );
    }

    /// No signing key configured (review item 6): the worker retains the
    /// obligation (never abandons, never panics) — the age alert is the signal.
    #[tokio::test]
    async fn worker_without_fee_payer_pool_retains_reservation() {
        let Some(pool) = isolated_db("no_signer").await else {
            return;
        };
        let cid = fresh_channel_id();
        create_channel(&pool, &new_channel(&cid, 50_000))
            .await
            .unwrap();
        draw_pinned_shape(&pool, &cid).await;
        close_channel_and_reserve_refund(&pool, &cid).await.unwrap();

        let rpc = Arc::new(MockRpc::new(1_000_000));
        let worker = RefundWorker::new(
            pool.clone(),
            rpc.clone(),
            None, // no signing capability
            agent_wallet(),
            None,
        );
        tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
        worker.sweep().await;

        let row = load_refund_row(&pool, &cid).await.unwrap();
        assert_eq!(row.status, "reserved");
        assert!(rpc.sends.lock().unwrap().is_empty(), "nothing broadcast");
    }
}
