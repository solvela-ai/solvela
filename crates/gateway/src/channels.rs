//! v0 off-chain spend-down channel — gateway-side Postgres ledger (repository).
//!
//! "Fund once, draw down." A channel is one USDC-SPL deposit, credited only
//! after the funding tx is verified on-chain, then drawn down off-chain by a
//! sequence of monotonic cumulative vouchers (the pure verification core lives
//! in [`solvela_x402::channel`]). This module is the **ledger**: the durable
//! record of every channel's `deposited / settled / last_voucher_cumulative`
//! state and its append-only voucher audit trail.
//!
//! Money-path rules honored here (solvela-fintech §1, §7):
//! - **Integer-atomic throughout.** Amounts are `u64` atomic micro-USDC in Rust
//!   and `BIGINT` (`i64`) in Postgres; every `u64 <-> i64` crossing is checked
//!   ([`atomic_to_i64`] / [`i64_to_atomic`]) and **fails closed** — an
//!   out-of-range or negative value is a hard error, never a wrapped or
//!   saturated amount.
//! - **The voucher-persist and the channel advance commit as ONE transaction**
//!   ([`persist_voucher_and_advance`]): a crash can never accept a draw it
//!   cannot later prove (RFC §5 T6 / channel scope §2). The advance is guarded
//!   monotonic at the DB layer (`WHERE last < new`) so the ledger can never
//!   regress, and a non-advancing voucher rolls the insert back too.
//! - **DB is REQUIRED for channels** (CLAUDE.md #12): there is no in-memory
//!   fallback ledger. When no pool is configured the channel scheme is simply
//!   unavailable — the routes return 404, they never fake a ledger.
//!
//! Channel-ledger writes here are intentionally **awaited** (the create and the
//! voucher-advance ARE the operation — losing them loses funds), unlike the
//! fire-and-forget spend-log/audit side effects on the chat hot path.

use solvela_x402::channel::ChannelState;
use sqlx::PgPool;

/// Lifecycle state of a channel. The DB `CHECK (status IN ('open','closing',
/// 'closed'))` mirrors these; a typed enum at the `set_status` call site stops a
/// stray string from ever reaching the constraint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelStatus {
    /// Funded and drawable.
    Open,
    /// Cooperative close requested; refund of `deposited -
    /// last_voucher_cumulative` (the unspent principal the agent never drew) is
    /// pending (v0: the on-chain custodial disbursement is a later slice).
    Closing,
    /// Fully closed — refund completed / no balance owed.
    Closed,
}

impl ChannelStatus {
    /// The exact lowercase string stored in `channels.status`.
    pub fn as_str(self) -> &'static str {
        match self {
            ChannelStatus::Open => "open",
            ChannelStatus::Closing => "closing",
            ChannelStatus::Closed => "closed",
        }
    }
}

/// Errors from the channel repository and its integer-atomic conversions. Every
/// variant is a hard failure — there is no silent `$0` / default row on the
/// money path.
#[derive(Debug, thiserror::Error)]
pub enum ChannelRepoError {
    /// A database / transaction error (connect, query, or a CHECK-constraint
    /// violation such as `settled > last`, which the DB rejects for us).
    #[error("channel database error: {0}")]
    Db(#[from] sqlx::Error),
    /// A `u64` atomic value (amount, slot, or nonce) exceeds `i64::MAX` and so
    /// cannot be stored in a `BIGINT` column. Fail closed — never truncate.
    #[error("value {0} does not fit a BIGINT (i64) column")]
    ValueTooLargeForBigint(u64),
    /// A stored `BIGINT` is negative — a corrupt ledger row (the CHECK
    /// constraints should make this unreachable). Refuse to read it as `u64`.
    #[error("stored atomic value {0} is negative (corrupt ledger row)")]
    NegativeStoredAmount(i64),
    /// The stored `channel_id` is not a base58 32-byte value (cannot build the
    /// voucher `channel_id` from it).
    #[error("stored channel_id is not a valid base58 32-byte value")]
    BadChannelId,
    /// The stored `session_key` is not a base58 32-byte ed25519 key.
    #[error("stored session_key is not a valid base58 32-byte ed25519 key")]
    BadSessionKey,
    /// The funding tx already opened a channel (replay) — `ON CONFLICT` matched
    /// the unique `funding_tx_sig` index and inserted nothing.
    #[error("funding transaction has already been used to open a channel")]
    FundingAlreadyUsed,
    /// The targeted channel row does not exist.
    #[error("channel not found")]
    ChannelNotFound,
    /// The voucher advance did not apply: the channel is missing or the
    /// presented cumulative is not strictly greater than the stored one. The
    /// voucher insert in the same transaction was rolled back.
    #[error("voucher advance not applied (channel missing or cumulative did not increase)")]
    AdvanceNotApplied,
}

/// Convert a `u64` atomic value to the `i64` BIGINT stored form. Fail-closed on
/// overflow (`> i64::MAX`) — never truncate a money amount.
fn atomic_to_i64(v: u64) -> Result<i64, ChannelRepoError> {
    i64::try_from(v).map_err(|_| ChannelRepoError::ValueTooLargeForBigint(v))
}

/// Convert a stored `i64` BIGINT atomic value to `u64`. Fail-closed on a
/// negative (corrupt) value rather than wrapping it into a huge `u64`.
fn i64_to_atomic(v: i64) -> Result<u64, ChannelRepoError> {
    u64::try_from(v).map_err(|_| ChannelRepoError::NegativeStoredAmount(v))
}

/// A new channel to insert, built from an **on-chain-verified** funding deposit.
/// `deposited_atomic` MUST be the verifier-extracted transfer amount, never a
/// client-asserted value.
#[derive(Debug, Clone)]
pub struct NewChannel {
    /// base58 of 32 bytes (v0: random; end-state: the Channel PDA) — also the
    /// 32-byte `channel_id` the voucher signs over.
    pub channel_id: String,
    /// base58 depositor wallet.
    pub agent_wallet: String,
    /// base58 ed25519 voucher-signing pubkey (defaults to `agent_wallet`).
    pub session_key: String,
    /// = the gateway recipient wallet (single on-chain payee).
    pub provider: String,
    /// USDC mint (base58).
    pub mint: String,
    /// On-chain-verified deposited principal (atomic micro-USDC).
    pub deposited_atomic: u64,
    /// Optional channel/voucher validity horizon (Solana slot).
    pub expiry_slot: Option<u64>,
    /// On-chain funding reference (the settled transfer signature). Uniquely
    /// indexed (migration 016) — the replay guard.
    pub funding_tx_sig: Option<String>,
}

/// A full channel row, for the close path and inspection.
#[derive(Debug, Clone)]
pub struct ChannelRow {
    pub channel_id: String,
    pub agent_wallet: String,
    pub session_key: String,
    pub provider: String,
    pub mint: String,
    pub deposited_atomic: u64,
    pub settled_atomic: u64,
    pub last_voucher_cumulative_atomic: u64,
    pub expiry_slot: Option<u64>,
    pub status: String,
    pub funding_tx_sig: Option<String>,
}

/// A voucher to persist together with the channel advance, in one transaction.
/// All amounts are atomic micro-USDC; `request_digest`/`signature` are raw bytes.
#[derive(Debug, Clone)]
pub struct VoucherRecord<'a> {
    pub channel_id: &'a str,
    /// Signed running total at this draw (atomic, fee-inclusive).
    pub cumulative_atomic: u64,
    /// This call's cost = `cumulative - prev_cumulative`.
    pub call_cost_atomic: u64,
    pub expiry_slot: u64,
    pub nonce: u64,
    /// SHA-256 of the canonical request body.
    pub request_digest: &'a [u8; 32],
    /// ed25519 signature over the canonical voucher message.
    pub signature: &'a [u8; 64],
}

/// Insert a new channel row from an on-chain-verified funding deposit.
///
/// Uses `ON CONFLICT (funding_tx_sig) DO NOTHING` against the unique index
/// (migration 016): if the same funding tx already opened a channel, 0 rows are
/// inserted and we return [`ChannelRepoError::FundingAlreadyUsed`] rather than
/// crediting the deposit twice. The DB CHECK constraints additionally reject any
/// row that violates `settled <= last <= deposited`.
pub async fn create_channel(pool: &PgPool, ch: &NewChannel) -> Result<(), ChannelRepoError> {
    let deposited = atomic_to_i64(ch.deposited_atomic)?;
    let expiry = ch.expiry_slot.map(atomic_to_i64).transpose()?;

    let result = sqlx::query(
        "INSERT INTO channels
           (channel_id, agent_wallet, session_key, provider, mint,
            deposited_atomic, settled_atomic, last_voucher_cumulative_atomic,
            expiry_slot, status, funding_tx_sig)
         VALUES ($1, $2, $3, $4, $5, $6, 0, 0, $7, 'open', $8)
         ON CONFLICT (funding_tx_sig) DO NOTHING",
    )
    .bind(&ch.channel_id)
    .bind(&ch.agent_wallet)
    .bind(&ch.session_key)
    .bind(&ch.provider)
    .bind(&ch.mint)
    .bind(deposited)
    .bind(expiry)
    .bind(&ch.funding_tx_sig)
    .execute(pool)
    .await?;

    if result.rows_affected() != 1 {
        return Err(ChannelRepoError::FundingAlreadyUsed);
    }
    Ok(())
}

/// Raw DB projection of a channel row (BIGINT → `i64`), mapped to the
/// integer-atomic [`ChannelRow`] after a fail-closed `i64 -> u64` conversion.
#[derive(sqlx::FromRow)]
struct ChannelDbRow {
    channel_id: String,
    agent_wallet: String,
    session_key: String,
    provider: String,
    mint: String,
    deposited_atomic: i64,
    settled_atomic: i64,
    last_voucher_cumulative_atomic: i64,
    expiry_slot: Option<i64>,
    status: String,
    funding_tx_sig: Option<String>,
}

/// Load a full channel row by id, or `None` if it does not exist.
pub async fn load_channel(
    pool: &PgPool,
    channel_id: &str,
) -> Result<Option<ChannelRow>, ChannelRepoError> {
    let row: Option<ChannelDbRow> = sqlx::query_as(
        "SELECT channel_id, agent_wallet, session_key, provider, mint,
                deposited_atomic, settled_atomic, last_voucher_cumulative_atomic,
                expiry_slot, status, funding_tx_sig
           FROM channels
          WHERE channel_id = $1",
    )
    .bind(channel_id)
    .fetch_optional(pool)
    .await?;

    let Some(r) = row else {
        return Ok(None);
    };

    Ok(Some(ChannelRow {
        channel_id: r.channel_id,
        agent_wallet: r.agent_wallet,
        session_key: r.session_key,
        provider: r.provider,
        mint: r.mint,
        deposited_atomic: i64_to_atomic(r.deposited_atomic)?,
        settled_atomic: i64_to_atomic(r.settled_atomic)?,
        last_voucher_cumulative_atomic: i64_to_atomic(r.last_voucher_cumulative_atomic)?,
        expiry_slot: r.expiry_slot.map(i64_to_atomic).transpose()?,
        status: r.status,
        funding_tx_sig: r.funding_tx_sig,
    }))
}

/// An open channel ready to be drawn: the verifier's [`ChannelState`] plus the
/// DB-sourced `agent_wallet`.
///
/// `agent_wallet` is carried alongside because a channel voucher itself has NO
/// `agent_pubkey`/tx (unlike an escrow payload), so the draw path MUST source
/// the wallet — for the #499 `require_tenant` gate and the spend/receipt
/// attribution — from the persistent ledger row, never from the voucher or
/// `extract_payer_wallet` (which returns `"unknown"` for a voucher). See channel
/// scope §3.13 / HALT 7.
#[derive(Debug, Clone)]
pub struct DrawableChannel {
    pub state: ChannelState,
    pub agent_wallet: String,
}

/// Load a [`DrawableChannel`] for the draw path: only `status='open'` channels
/// are drawable, so a closed/closing channel returns `None` (the draw is
/// refused).
///
/// The persistent ledger fields are read from the row; `expected_request_digest`
/// is supplied by the caller (the digest of the request actually being served),
/// so the verifier can bind the voucher to this request. The 32-byte
/// `channel_id` and `session_key` are base58-decoded — a corrupt encoding is a
/// hard error, never a silently-empty key. The DB-sourced `agent_wallet` is
/// returned alongside for attribution (see [`DrawableChannel`]).
pub async fn load_open_channel_state(
    pool: &PgPool,
    channel_id: &str,
    expected_request_digest: [u8; 32],
) -> Result<Option<DrawableChannel>, ChannelRepoError> {
    let row: Option<(String, i64, i64, i64, String, String)> = sqlx::query_as(
        "SELECT channel_id, deposited_atomic, settled_atomic,
                last_voucher_cumulative_atomic, session_key, agent_wallet
           FROM channels
          WHERE channel_id = $1 AND status = 'open'",
    )
    .bind(channel_id)
    .fetch_optional(pool)
    .await?;

    let Some((channel_id_b58, deposited, settled, last, session_key_b58, agent_wallet)) = row
    else {
        return Ok(None);
    };

    let channel_id = solvela_x402::escrow::pda::decode_bs58_pubkey(&channel_id_b58)
        .map_err(|_| ChannelRepoError::BadChannelId)?;
    let session_key = solvela_x402::escrow::pda::decode_bs58_pubkey(&session_key_b58)
        .map_err(|_| ChannelRepoError::BadSessionKey)?;

    Ok(Some(DrawableChannel {
        state: ChannelState {
            channel_id,
            deposited_atomic: i64_to_atomic(deposited)?,
            settled_atomic: i64_to_atomic(settled)?,
            last_cumulative_atomic: i64_to_atomic(last)?,
            session_key,
            expected_request_digest,
        },
        agent_wallet,
    }))
}

/// Persist a draw voucher AND advance `channels.last_voucher_cumulative_atomic`
/// in **one transaction** (channel scope §2 / RFC §5 T6).
///
/// The voucher insert and the channel advance commit together or not at all: a
/// crash can never accept a draw it cannot later prove, and a corrupt advance
/// can never leave an orphan voucher. The advance is guarded monotonic at the DB
/// layer (`WHERE last_voucher_cumulative_atomic < $cumulative`); if it does not
/// apply (channel missing, or cumulative not strictly greater), the whole
/// transaction — including the voucher insert — is rolled back and
/// [`ChannelRepoError::AdvanceNotApplied`] is returned.
///
/// The caller (Pass B draw path) MUST have already accepted the voucher via
/// [`solvela_x402::channel::verify_voucher`]; this function is the durable
/// commit, not the validation.
pub async fn persist_voucher_and_advance(
    pool: &PgPool,
    v: &VoucherRecord<'_>,
) -> Result<(), ChannelRepoError> {
    let cumulative = atomic_to_i64(v.cumulative_atomic)?;
    let call_cost = atomic_to_i64(v.call_cost_atomic)?;
    let expiry = atomic_to_i64(v.expiry_slot)?;
    let nonce = atomic_to_i64(v.nonce)?;

    let mut tx = pool.begin().await?;

    // Append-only audit row. A duplicate (channel_id, cumulative_atomic) hits
    // the UNIQUE constraint and aborts the transaction — replay defense.
    sqlx::query(
        "INSERT INTO channel_vouchers
           (channel_id, cumulative_atomic, call_cost_atomic, expiry_slot,
            nonce, request_digest, signature)
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(v.channel_id)
    .bind(cumulative)
    .bind(call_cost)
    .bind(expiry)
    .bind(nonce)
    .bind(&v.request_digest[..])
    .bind(&v.signature[..])
    .execute(&mut *tx)
    .await?;

    // Advance the channel's authoritative cumulative — strictly increasing only.
    // The DB CHECK `last <= deposited` additionally rejects an over-draw advance.
    let advanced = sqlx::query(
        "UPDATE channels
            SET last_voucher_cumulative_atomic = $1, updated_at = NOW()
          WHERE channel_id = $2 AND last_voucher_cumulative_atomic < $1",
    )
    .bind(cumulative)
    .bind(v.channel_id)
    .execute(&mut *tx)
    .await?;

    if advanced.rows_affected() != 1 {
        // Roll back the voucher insert too: never persist a voucher whose
        // advance the ledger did not accept.
        tx.rollback().await?;
        return Err(ChannelRepoError::AdvanceNotApplied);
    }

    tx.commit().await?;
    Ok(())
}

/// Update a channel's `settled_atomic` (settlement checkpoint / on-chain settle
/// reconciliation). The DB CHECK `settled <= last` rejects a settle that exceeds
/// the highest signed cumulative — fail closed at the ledger.
pub async fn update_settled(
    pool: &PgPool,
    channel_id: &str,
    settled_atomic: u64,
) -> Result<(), ChannelRepoError> {
    let settled = atomic_to_i64(settled_atomic)?;
    let result = sqlx::query(
        "UPDATE channels SET settled_atomic = $1, updated_at = NOW() WHERE channel_id = $2",
    )
    .bind(settled)
    .bind(channel_id)
    .execute(pool)
    .await?;
    if result.rows_affected() != 1 {
        return Err(ChannelRepoError::ChannelNotFound);
    }
    Ok(())
}

/// Set a channel's lifecycle `status`.
pub async fn set_status(
    pool: &PgPool,
    channel_id: &str,
    status: ChannelStatus,
) -> Result<(), ChannelRepoError> {
    let result =
        sqlx::query("UPDATE channels SET status = $1, updated_at = NOW() WHERE channel_id = $2")
            .bind(status.as_str())
            .bind(channel_id)
            .execute(pool)
            .await?;
    if result.rows_affected() != 1 {
        return Err(ChannelRepoError::ChannelNotFound);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- pure conversion guards (no DB) -------------------------------------

    #[test]
    fn atomic_to_i64_roundtrips_realistic_values() {
        // $1,000,000 in atomic micro-USDC fits an i64 comfortably.
        assert_eq!(atomic_to_i64(1_000_000_000_000).unwrap(), 1_000_000_000_000);
        assert_eq!(atomic_to_i64(0).unwrap(), 0);
        assert_eq!(atomic_to_i64(i64::MAX as u64).unwrap(), i64::MAX);
    }

    #[test]
    fn atomic_to_i64_fails_closed_above_i64_max() {
        // A value above i64::MAX must NOT truncate/wrap into a small amount.
        let over = i64::MAX as u64 + 1;
        assert!(matches!(
            atomic_to_i64(over),
            Err(ChannelRepoError::ValueTooLargeForBigint(v)) if v == over
        ));
        assert!(matches!(
            atomic_to_i64(u64::MAX),
            Err(ChannelRepoError::ValueTooLargeForBigint(_))
        ));
    }

    #[test]
    fn i64_to_atomic_fails_closed_on_negative() {
        assert_eq!(i64_to_atomic(0).unwrap(), 0);
        assert_eq!(i64_to_atomic(42).unwrap(), 42);
        assert!(matches!(
            i64_to_atomic(-1),
            Err(ChannelRepoError::NegativeStoredAmount(-1))
        ));
    }

    #[test]
    fn channel_status_strings_match_db_check() {
        assert_eq!(ChannelStatus::Open.as_str(), "open");
        assert_eq!(ChannelStatus::Closing.as_str(), "closing");
        assert_eq!(ChannelStatus::Closed.as_str(), "closed");
    }

    // -- DB-backed repository tests (skip when no DATABASE_URL) --------------
    //
    // Channels are DB-backed (CLAUDE.md #12), so the transactional guarantees
    // can only be proven against real Postgres. To honor "add no new failing
    // tests without a live DB", these SKIP (return Ok) when `DATABASE_URL` is
    // unset, rather than using `#[sqlx::test]` (which fails without a DB, the
    // known pre-existing `escrow_queue.rs` category). With a DB they run in full
    // and apply the in-repo migrations idempotently. Each test uses a unique
    // random `channel_id`, so they are safe against a shared dev database.

    use base64::Engine as _;

    /// Connect + migrate, or `None` to skip when no `DATABASE_URL` is set.
    async fn db() -> Option<PgPool> {
        let url = std::env::var("DATABASE_URL").ok()?;
        let pool = PgPool::connect(&url).await.ok()?;
        sqlx::migrate!("../../migrations")
            .run(&pool)
            .await
            .expect("migrations must apply");
        Some(pool)
    }

    /// A fresh base58 32-byte channel id (also the voucher's 32-byte channel_id).
    fn fresh_channel_id() -> String {
        let mut bytes = [0u8; 32];
        bytes[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        bytes[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
        bs58::encode(bytes).into_string()
    }

    /// A valid base58 32-byte key for the agent/session fields.
    fn some_pubkey() -> String {
        "9noXzpXnkyEcKF3AeXqUHTdR59V5uvrRBUo9bwsHaByz".to_string()
    }

    fn new_channel(channel_id: &str, deposited: u64, funding_sig: &str) -> NewChannel {
        NewChannel {
            channel_id: channel_id.to_string(),
            agent_wallet: some_pubkey(),
            session_key: some_pubkey(),
            provider: "RecipientWallet111111111111111111111111111111".to_string(),
            mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            deposited_atomic: deposited,
            expiry_slot: Some(1_000_750),
            funding_tx_sig: Some(funding_sig.to_string()),
        }
    }

    #[tokio::test]
    async fn create_channel_persists_verified_deposit() {
        let Some(pool) = db().await else { return };
        let cid = fresh_channel_id();
        let sig = format!("sig-create-{cid}");
        create_channel(&pool, &new_channel(&cid, 2_625, &sig))
            .await
            .expect("create must succeed");

        let row = load_channel(&pool, &cid)
            .await
            .expect("load")
            .expect("row exists");
        // The credited deposit is exactly what was passed (the route passes the
        // on-chain-verified amount); fresh channel starts settled=0, last=0.
        assert_eq!(row.deposited_atomic, 2_625);
        assert_eq!(row.settled_atomic, 0);
        assert_eq!(row.last_voucher_cumulative_atomic, 0);
        assert_eq!(row.status, "open");
    }

    #[tokio::test]
    async fn create_channel_rejects_duplicate_funding_tx() {
        let Some(pool) = db().await else { return };
        let sig = format!("sig-dup-{}", uuid::Uuid::new_v4());
        create_channel(&pool, &new_channel(&fresh_channel_id(), 1_000, &sig))
            .await
            .expect("first open succeeds");
        // Same funding tx, different channel id → must NOT double-credit.
        let err = create_channel(&pool, &new_channel(&fresh_channel_id(), 1_000, &sig))
            .await
            .expect_err("replay of the same funding tx must be rejected");
        assert!(matches!(err, ChannelRepoError::FundingAlreadyUsed));
    }

    #[tokio::test]
    async fn persist_voucher_advances_last_cumulative() {
        let Some(pool) = db().await else { return };
        let cid = fresh_channel_id();
        create_channel(&pool, &new_channel(&cid, 50_000, &format!("sig-adv-{cid}")))
            .await
            .unwrap();

        persist_voucher_and_advance(
            &pool,
            &VoucherRecord {
                channel_id: &cid,
                cumulative_atomic: 10_500,
                call_cost_atomic: 10_500,
                expiry_slot: 1_000_750,
                nonce: 1,
                request_digest: &[0x22; 32],
                signature: &[0x33; 64],
            },
        )
        .await
        .expect("first draw advances");

        let row = load_channel(&pool, &cid).await.unwrap().unwrap();
        assert_eq!(row.last_voucher_cumulative_atomic, 10_500);
    }

    #[tokio::test]
    async fn persist_voucher_is_atomic_on_failed_advance() {
        let Some(pool) = db().await else { return };
        let cid = fresh_channel_id();
        create_channel(
            &pool,
            &new_channel(&cid, 50_000, &format!("sig-atomic-{cid}")),
        )
        .await
        .unwrap();

        // Advance to 2_000.
        persist_voucher_and_advance(
            &pool,
            &VoucherRecord {
                channel_id: &cid,
                cumulative_atomic: 2_000,
                call_cost_atomic: 2_000,
                expiry_slot: 1_000_750,
                nonce: 1,
                request_digest: &[0x22; 32],
                signature: &[0x33; 64],
            },
        )
        .await
        .unwrap();

        // A LOWER cumulative (1_000) cannot advance (WHERE last < new is false).
        // The whole transaction must roll back — the voucher insert too.
        let err = persist_voucher_and_advance(
            &pool,
            &VoucherRecord {
                channel_id: &cid,
                cumulative_atomic: 1_000,
                call_cost_atomic: 1_000,
                expiry_slot: 1_000_750,
                nonce: 2,
                request_digest: &[0x44; 32],
                signature: &[0x55; 64],
            },
        )
        .await
        .expect_err("a non-advancing voucher must fail");
        assert!(matches!(err, ChannelRepoError::AdvanceNotApplied));

        // last_cumulative is UNCHANGED (still 2_000) ...
        let row = load_channel(&pool, &cid).await.unwrap().unwrap();
        assert_eq!(
            row.last_voucher_cumulative_atomic, 2_000,
            "a failed advance must not move last_cumulative"
        );
        // ... and the rolled-back voucher (cumulative=1_000) left no row.
        let voucher_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM channel_vouchers WHERE channel_id = $1 AND cumulative_atomic = 1000",
        )
        .bind(&cid)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            voucher_count, 0,
            "the voucher insert must roll back with the failed advance"
        );
    }

    #[tokio::test]
    async fn load_open_channel_state_builds_voucher_state() {
        let Some(pool) = db().await else { return };
        let cid = fresh_channel_id();
        create_channel(
            &pool,
            &new_channel(&cid, 50_000, &format!("sig-state-{cid}")),
        )
        .await
        .unwrap();

        let digest = [0x66; 32];
        let drawable = load_open_channel_state(&pool, &cid, digest)
            .await
            .unwrap()
            .expect("open channel state");
        let state = &drawable.state;
        assert_eq!(state.deposited_atomic, 50_000);
        assert_eq!(state.settled_atomic, 0);
        assert_eq!(state.last_cumulative_atomic, 0);
        assert_eq!(state.expected_request_digest, digest);
        // The agent_wallet is DB-sourced (channel attribution never comes from
        // the voucher) — here it is the fixture's `some_pubkey()`.
        assert_eq!(drawable.agent_wallet, some_pubkey());
        // channel_id bytes round-trip through base58.
        assert_eq!(bs58::encode(state.channel_id).into_string(), cid);

        // A closed channel is NOT drawable → None.
        set_status(&pool, &cid, ChannelStatus::Closed)
            .await
            .unwrap();
        assert!(load_open_channel_state(&pool, &cid, digest)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn update_settled_rejects_exceeding_last_cumulative() {
        let Some(pool) = db().await else { return };
        let cid = fresh_channel_id();
        create_channel(
            &pool,
            &new_channel(&cid, 50_000, &format!("sig-settle-{cid}")),
        )
        .await
        .unwrap();
        // last_cumulative is 0; settling 1 (> last) violates the CHECK
        // `settled <= last` → the DB rejects it (fail closed at the ledger).
        let err = update_settled(&pool, &cid, 1)
            .await
            .expect_err("settling above the highest cumulative must fail");
        assert!(matches!(err, ChannelRepoError::Db(_)));
    }

    /// Confirms the golden-vector channel core links against this crate so the
    /// repository and the verifier agree on the same `ChannelState` shape.
    #[test]
    fn voucher_core_golden_vector_is_reachable() {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(solvela_x402::channel::GOLDEN_VECTOR_B64)
            .expect("golden vector decodes");
        assert_eq!(decoded.len(), 114);
    }
}
