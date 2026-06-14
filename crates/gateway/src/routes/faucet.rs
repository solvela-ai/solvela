//! Gas-drip faucet — `POST /v1/faucet/gas`.
//!
//! Drips a dust of native SOL to USDC-funded agent wallets so the agent (the
//! on-chain fee payer for `exact` / `escrow` payments) can pay its own network
//! gas. The user funds USDC only; the gateway covers gas from a dedicated gas
//! wallet (NOT the fee-payer reserve).
//!
//! ## Public but hard-gated
//!
//! Agents call this route, so it is unauthenticated — but every drip must pass
//! ALL of the gates below, ordered cheapest-first so an abusive caller is
//! rejected before any RPC/chain work:
//!
//! 1. **enabled** — faucet config on + a source key present + a DB present.
//! 2. **valid wallet** — parses as a base58 Solana pubkey.
//! 3. **not already dripped** — an `INSERT ... ON CONFLICT DO NOTHING`
//!    *reservation* taken BEFORE sending. 0 rows inserted ⇒ already handled
//!    (or a concurrent request won) ⇒ `already_funded`. The PK serializes
//!    concurrent requests so we **never double-drip**.
//! 4. **daily cap** — global per-UTC-day total `< daily_cap_lamports`.
//! 5. **USDC floor** — wallet USDC `>= usdc_floor_atomic` (the abuse moat).
//! 6. **SOL low-water** — wallet SOL `< sol_low_water_lamports`.
//!
//! On gate failure AFTER the reservation (4/5/6, or a send failure) the
//! reservation row is DELETEd so a legitimate retry is possible. On send
//! SUCCESS the row's `tx_signature` is filled in.
//!
//! ## Money-path safety: prefer under-dripping to double-dripping
//!
//! A crash AFTER send but BEFORE the signature update leaves the reservation
//! in place (with a NULL `tx_signature`), which correctly blocks a re-drip —
//! the safe failure direction. We never delete a reservation once the transfer
//! has been broadcast.

use std::sync::Arc;

use async_trait::async_trait;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use zeroize::Zeroizing;

use solvela_x402::solana::{sign_system_transfer, SystemTransferError};
use solvela_x402::solana_types::Pubkey;

use crate::AppState;

/// Request body for `POST /v1/faucet/gas`.
#[derive(Debug, Deserialize)]
pub struct FaucetRequest {
    /// Base58 Solana wallet address to fund.
    pub wallet: String,
}

/// Response body for `POST /v1/faucet/gas`.
#[derive(Debug, Serialize, PartialEq)]
pub struct FaucetResponse {
    /// Whether a drip was sent (or was already present).
    pub funded: bool,
    /// Machine-readable reason when `funded == false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The drip transaction signature when a drip was sent (or the prior one
    /// for an `already_funded` wallet, if cheaply available).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_signature: Option<String>,
    /// Lamports dripped when `funded == true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lamports: Option<u64>,
}

// ---------------------------------------------------------------------------
// Injected dependencies (so the route is testable without a live RPC/DB)
// ---------------------------------------------------------------------------

/// Errors from the faucet pipeline.
#[derive(Debug, thiserror::Error)]
pub enum FaucetError {
    /// An RPC balance read or send failed.
    #[error("rpc error: {0}")]
    Rpc(String),
    /// A ledger (DB) operation failed.
    #[error("ledger error: {0}")]
    Ledger(String),
    /// Building/signing the SOL transfer failed.
    #[error("transfer build error: {0}")]
    Transfer(#[from] SystemTransferError),
}

/// On-chain operations the faucet needs. Mocked in tests.
#[async_trait]
pub trait GasSource: Send + Sync {
    /// Native SOL balance of `wallet_b58` in lamports.
    async fn sol_balance(&self, wallet_b58: &str) -> Result<u64, FaucetError>;
    /// USDC-SPL balance of `wallet_b58` in atomic (6-decimal) units; a missing
    /// ATA is treated as 0 by the implementation.
    async fn usdc_balance(&self, wallet_b58: &str) -> Result<u64, FaucetError>;
    /// Build, sign, and broadcast a `lamports` SOL transfer to `wallet_b58`.
    /// Returns the tx signature on success.
    async fn send_drip(&self, wallet_b58: &str, lamports: u64) -> Result<String, FaucetError>;
}

/// Idempotency / cap ledger backed by the `gas_drips` table. Mocked in tests.
#[async_trait]
pub trait GasLedger: Send + Sync {
    /// Reserve `wallet_b58` for a drip of `lamports`:
    /// `INSERT ... ON CONFLICT (wallet_address) DO NOTHING`. Returns `true`
    /// when THIS call inserted the row (reservation acquired), `false` on
    /// conflict (already handled / concurrent winner).
    async fn reserve(&self, wallet_b58: &str, lamports: u64) -> Result<bool, FaucetError>;
    /// Delete the reservation row for `wallet_b58` (rollback when a gate after
    /// the reservation rejects, or a send fails). Idempotent.
    async fn delete_reservation(&self, wallet_b58: &str) -> Result<(), FaucetError>;
    /// Total lamports dripped/reserved for the current UTC day (`SUM(lamports)
    /// WHERE created_at::date = CURRENT_DATE`).
    async fn day_total_lamports(&self) -> Result<u64, FaucetError>;
    /// Persist the broadcast `tx_signature` onto the wallet's reservation row.
    async fn record_signature(
        &self,
        wallet_b58: &str,
        tx_signature: &str,
    ) -> Result<(), FaucetError>;
    /// Best-effort prior signature for an already-funded wallet (cheap lookup;
    /// `None` if absent/unavailable).
    async fn prior_signature(&self, wallet_b58: &str) -> Result<Option<String>, FaucetError>;
}

// ---------------------------------------------------------------------------
// Faucet — the gated drip pipeline
// ---------------------------------------------------------------------------

/// Outcome of a drip attempt. Each variant maps 1:1 to a response + a
/// `result` metric label.
#[derive(Debug, PartialEq)]
pub enum DripOutcome {
    Funded { tx_signature: String, lamports: u64 },
    AlreadyFunded { tx_signature: Option<String> },
    InsufficientUsdc,
    AlreadyHasSol,
    DailyCap,
    SendFailed,
    BadWallet,
    Disabled,
}

impl DripOutcome {
    /// The `result` metric label.
    fn metric_label(&self) -> &'static str {
        match self {
            DripOutcome::Funded { .. } => "funded",
            DripOutcome::AlreadyFunded { .. } => "already_funded",
            DripOutcome::InsufficientUsdc => "insufficient_usdc",
            DripOutcome::AlreadyHasSol => "already_has_sol",
            DripOutcome::DailyCap => "daily_cap",
            DripOutcome::SendFailed => "send_failed",
            DripOutcome::BadWallet => "bad_wallet",
            DripOutcome::Disabled => "disabled",
        }
    }
}

/// Gating thresholds copied from `FaucetConfig` (so the `Faucet` is
/// self-contained and the route never re-reads config at request time).
#[derive(Debug, Clone, Copy)]
pub struct FaucetParams {
    pub drip_lamports: u64,
    pub usdc_floor_atomic: u64,
    pub sol_low_water_lamports: u64,
    pub daily_cap_lamports: u64,
}

/// The wired faucet: gating params + injected source/ledger. Held as
/// `Option<Arc<Faucet>>` on `AppState`; `None` ⇒ disabled.
pub struct Faucet {
    params: FaucetParams,
    source: Arc<dyn GasSource>,
    ledger: Arc<dyn GasLedger>,
}

impl Faucet {
    pub fn new(
        params: FaucetParams,
        source: Arc<dyn GasSource>,
        ledger: Arc<dyn GasLedger>,
    ) -> Self {
        Self {
            params,
            source,
            ledger,
        }
    }

    /// Run the full gated drip pipeline for `wallet_b58`.
    ///
    /// Gates are evaluated cheapest-first; the reservation (gate 3) is the
    /// concurrency-serialization point — every gate AFTER it that rejects
    /// rolls the reservation back so a later legitimate request can succeed.
    pub async fn drip(&self, wallet_b58: &str) -> DripOutcome {
        // Gate 2: valid base58 Solana pubkey. (Gate 1 — enabled — is enforced
        // at the route: an absent `Faucet` short-circuits to `Disabled`.)
        use std::str::FromStr;
        if Pubkey::from_str(wallet_b58).is_err() {
            return DripOutcome::BadWallet;
        }

        // Gate 3: reservation (idempotency + concurrency serialization).
        // Taken BEFORE any RPC so a duplicate request never even reads chain
        // state. A conflict means another request already owns this wallet.
        match self
            .ledger
            .reserve(wallet_b58, self.params.drip_lamports)
            .await
        {
            Ok(true) => { /* reservation acquired — continue */ }
            Ok(false) => {
                let prior = self
                    .ledger
                    .prior_signature(wallet_b58)
                    .await
                    .unwrap_or(None);
                return DripOutcome::AlreadyFunded {
                    tx_signature: prior,
                };
            }
            Err(e) => {
                warn!(error = %e, "faucet reservation failed");
                // Could not reserve → cannot guarantee no-double-drip → refuse.
                // Treat as send_failed-class (retryable) without dripping.
                return DripOutcome::SendFailed;
            }
        }

        // From here on, any rejection must roll back the reservation.

        // Gate 4: global daily cap. Re-read the day total AFTER reserving so
        // our own reservation is included — this is intentional: it makes the
        // cap a true ceiling on reserved+sent lamports for the day.
        match self.ledger.day_total_lamports().await {
            Ok(total) if total > self.params.daily_cap_lamports => {
                self.rollback(wallet_b58).await;
                return DripOutcome::DailyCap;
            }
            Ok(_) => {}
            Err(e) => {
                warn!(error = %e, "faucet daily-cap read failed");
                self.rollback(wallet_b58).await;
                return DripOutcome::SendFailed;
            }
        }

        // Gate 5: USDC floor (the abuse moat). Atomic-unit integer comparison.
        match self.source.usdc_balance(wallet_b58).await {
            Ok(usdc_atomic) if usdc_atomic >= self.params.usdc_floor_atomic => {}
            Ok(_) => {
                self.rollback(wallet_b58).await;
                return DripOutcome::InsufficientUsdc;
            }
            Err(e) => {
                warn!(error = %e, "faucet usdc balance read failed");
                self.rollback(wallet_b58).await;
                return DripOutcome::SendFailed;
            }
        }

        // Gate 6: SOL low-water — only drip a wallet that actually needs it.
        match self.source.sol_balance(wallet_b58).await {
            Ok(sol_lamports) if sol_lamports < self.params.sol_low_water_lamports => {}
            Ok(_) => {
                self.rollback(wallet_b58).await;
                return DripOutcome::AlreadyHasSol;
            }
            Err(e) => {
                warn!(error = %e, "faucet sol balance read failed");
                self.rollback(wallet_b58).await;
                return DripOutcome::SendFailed;
            }
        }

        // Send the drip. On failure roll back the reservation so a retry can
        // succeed (the tx was never broadcast).
        let signature = match self
            .source
            .send_drip(wallet_b58, self.params.drip_lamports)
            .await
        {
            Ok(sig) => sig,
            Err(e) => {
                warn!(error = %e, wallet = %wallet_b58, "faucet drip send failed");
                self.rollback(wallet_b58).await;
                return DripOutcome::SendFailed;
            }
        };

        // SUCCESS. Persist the signature. A failure HERE does NOT roll back:
        // the transfer is already on the wire, and the reservation (with a
        // NULL tx_signature) correctly continues to block a re-drip — the safe
        // failure direction (under-drip, never double-drip).
        if let Err(e) = self.ledger.record_signature(wallet_b58, &signature).await {
            warn!(
                error = %e,
                wallet = %wallet_b58,
                signature = %signature,
                "faucet drip sent but signature persist failed; reservation retained (no re-drip)"
            );
        }

        info!(
            wallet = %wallet_b58,
            lamports = self.params.drip_lamports,
            signature = %signature,
            "gas drip funded"
        );
        DripOutcome::Funded {
            tx_signature: signature,
            lamports: self.params.drip_lamports,
        }
    }

    /// Roll back a reservation. Best-effort: a delete failure is logged, never
    /// fatal — the worst case is a wallet that can't be re-dripped until the
    /// row is cleared, which is the safe (under-drip) direction.
    async fn rollback(&self, wallet_b58: &str) {
        if let Err(e) = self.ledger.delete_reservation(wallet_b58).await {
            warn!(error = %e, wallet = %wallet_b58, "faucet reservation rollback failed");
        }
    }
}

// ---------------------------------------------------------------------------
// Production wiring: sqlx ledger + x402-backed gas source
// ---------------------------------------------------------------------------

/// Postgres-backed [`GasLedger`] over the `gas_drips` table.
pub struct PgGasLedger {
    pool: sqlx::PgPool,
}

impl PgGasLedger {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl GasLedger for PgGasLedger {
    async fn reserve(&self, wallet_b58: &str, lamports: u64) -> Result<bool, FaucetError> {
        let res = sqlx::query(
            "INSERT INTO gas_drips (wallet_address, lamports) VALUES ($1, $2) \
             ON CONFLICT (wallet_address) DO NOTHING",
        )
        .bind(wallet_b58)
        .bind(lamports as i64)
        .execute(&self.pool)
        .await
        .map_err(|e| FaucetError::Ledger(e.to_string()))?;
        Ok(res.rows_affected() == 1)
    }

    async fn delete_reservation(&self, wallet_b58: &str) -> Result<(), FaucetError> {
        // Only delete a reservation that has NOT yet been broadcast
        // (tx_signature IS NULL). This is a belt-and-braces guard: rollback is
        // only ever called before a successful send, but scoping the DELETE to
        // un-broadcast rows makes a concurrent funded row impossible to clear.
        sqlx::query("DELETE FROM gas_drips WHERE wallet_address = $1 AND tx_signature IS NULL")
            .bind(wallet_b58)
            .execute(&self.pool)
            .await
            .map_err(|e| FaucetError::Ledger(e.to_string()))?;
        Ok(())
    }

    async fn day_total_lamports(&self) -> Result<u64, FaucetError> {
        let row: (i64,) = sqlx::query_as(
            "SELECT COALESCE(SUM(lamports), 0)::BIGINT FROM gas_drips \
             WHERE created_at::date = CURRENT_DATE",
        )
        .fetch_one(&self.pool)
        .await
        .map_err(|e| FaucetError::Ledger(e.to_string()))?;
        Ok(row.0.max(0) as u64)
    }

    async fn record_signature(
        &self,
        wallet_b58: &str,
        tx_signature: &str,
    ) -> Result<(), FaucetError> {
        sqlx::query("UPDATE gas_drips SET tx_signature = $2 WHERE wallet_address = $1")
            .bind(wallet_b58)
            .bind(tx_signature)
            .execute(&self.pool)
            .await
            .map_err(|e| FaucetError::Ledger(e.to_string()))?;
        Ok(())
    }

    async fn prior_signature(&self, wallet_b58: &str) -> Result<Option<String>, FaucetError> {
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT tx_signature FROM gas_drips WHERE wallet_address = $1")
                .bind(wallet_b58)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| FaucetError::Ledger(e.to_string()))?;
        Ok(row.and_then(|r| r.0))
    }
}

/// x402-backed [`GasSource`]: reads balances via JSON-RPC and signs/broadcasts
/// the drip from the dedicated gas keypair.
///
/// Holds the raw 64-byte gas keypair (the gateway pays this gas). The keypair
/// is zeroized on drop; only signed transactions leave the gateway.
pub struct RpcGasSource {
    http_client: reqwest::Client,
    rpc_url: String,
    usdc_mint: String,
    source_pubkey: [u8; 32],
    /// Base58 source pubkey for logging only.
    source_pubkey_b58: String,
    source_keypair: Zeroizing<[u8; 64]>,
}

impl std::fmt::Debug for RpcGasSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcGasSource")
            .field("rpc_url", &self.rpc_url)
            .field("usdc_mint", &self.usdc_mint)
            .field("source_pubkey", &self.source_pubkey_b58)
            .field("source_keypair", &"[REDACTED]")
            .finish()
    }
}

/// Error constructing an [`RpcGasSource`] from a base58 keypair.
#[derive(Debug, thiserror::Error)]
pub enum RpcGasSourceError {
    #[error("invalid gas source keypair: {0}")]
    InvalidKeypair(String),
}

impl RpcGasSource {
    /// Build from the base58-encoded 64-byte gas keypair.
    pub fn from_keypair_b58(
        http_client: reqwest::Client,
        rpc_url: String,
        usdc_mint: String,
        keypair_b58: &str,
    ) -> Result<Self, RpcGasSourceError> {
        let decoded = Zeroizing::new(
            bs58::decode(keypair_b58)
                .into_vec()
                .map_err(|e| RpcGasSourceError::InvalidKeypair(format!("base58: {e}")))?,
        );
        if decoded.len() != 64 {
            return Err(RpcGasSourceError::InvalidKeypair(format!(
                "expected 64 bytes, got {}",
                decoded.len()
            )));
        }
        let mut keypair = Zeroizing::new([0u8; 64]);
        keypair.copy_from_slice(&decoded);

        // Validate the keypair is a self-consistent ed25519 key and capture the
        // public half (== account_keys[0] for every drip). Delegated to x402 so
        // the gateway needs no direct ed25519-dalek production dependency.
        let source_pubkey = solvela_x402::solana::keypair_pubkey(&keypair)
            .map_err(|e| RpcGasSourceError::InvalidKeypair(e.to_string()))?;
        let source_pubkey_b58 = bs58::encode(source_pubkey).into_string();

        Ok(Self {
            http_client,
            rpc_url,
            usdc_mint,
            source_pubkey,
            source_pubkey_b58,
            source_keypair: keypair,
        })
    }

    /// Base58 source (gas wallet) pubkey — safe to log.
    pub fn source_pubkey_b58(&self) -> &str {
        &self.source_pubkey_b58
    }
}

#[async_trait]
impl GasSource for RpcGasSource {
    async fn sol_balance(&self, wallet_b58: &str) -> Result<u64, FaucetError> {
        solvela_x402::solana_rpc::get_sol_balance(&self.http_client, &self.rpc_url, wallet_b58)
            .await
            .map_err(|e| FaucetError::Rpc(e.to_string()))
    }

    async fn usdc_balance(&self, wallet_b58: &str) -> Result<u64, FaucetError> {
        solvela_x402::solana_rpc::get_usdc_balance(
            &self.http_client,
            &self.rpc_url,
            wallet_b58,
            &self.usdc_mint,
        )
        .await
        .map_err(|e| FaucetError::Rpc(e.to_string()))
    }

    async fn send_drip(&self, wallet_b58: &str, lamports: u64) -> Result<String, FaucetError> {
        use std::str::FromStr;
        let to = Pubkey::from_str(wallet_b58)
            .map_err(|e| FaucetError::Rpc(format!("invalid recipient pubkey: {e}")))?;

        // Fetch a fresh blockhash, then sign + broadcast.
        let blockhash =
            solvela_x402::solana_rpc::get_latest_blockhash(&self.http_client, &self.rpc_url)
                .await
                .map_err(|e| FaucetError::Rpc(e.to_string()))?;

        let signed_b64 = sign_system_transfer(
            &self.source_pubkey,
            &to.0,
            lamports,
            &blockhash,
            &self.source_keypair,
        )?;

        solvela_x402::solana_rpc::send_transaction(&self.http_client, &self.rpc_url, &signed_b64)
            .await
            .map_err(|e| FaucetError::Rpc(e.to_string()))
    }
}

// ---------------------------------------------------------------------------
// Route handler
// ---------------------------------------------------------------------------

/// `POST /v1/faucet/gas`
///
/// Public route. Returns 200 with `{ funded, reason?, tx_signature?, lamports? }`
/// for every gated/handled case, and 502 only for an actual send failure (so
/// callers can distinguish "declined" from "tried and failed").
pub async fn gas_faucet(
    State(state): State<Arc<AppState>>,
    Json(req): Json<FaucetRequest>,
) -> impl IntoResponse {
    let outcome = match &state.faucet {
        // Gate 1: enabled. An absent faucet (disabled, no source key, or no DB)
        // short-circuits — no chain or DB work.
        None => DripOutcome::Disabled,
        Some(faucet) => faucet.drip(req.wallet.trim()).await,
    };

    // Metrics: one labelled counter per outcome + a lamports counter on funded.
    metrics::counter!("solvela_gas_drips_total", "result" => outcome.metric_label()).increment(1);
    if let DripOutcome::Funded { lamports, .. } = &outcome {
        metrics::counter!("solvela_gas_drip_lamports_total").increment(*lamports);
    }

    let (status, body) = match outcome {
        DripOutcome::Funded {
            tx_signature,
            lamports,
        } => (
            StatusCode::OK,
            FaucetResponse {
                funded: true,
                reason: None,
                tx_signature: Some(tx_signature),
                lamports: Some(lamports),
            },
        ),
        DripOutcome::AlreadyFunded { tx_signature } => (
            StatusCode::OK,
            FaucetResponse {
                funded: false,
                reason: Some("already_funded".to_string()),
                tx_signature,
                lamports: None,
            },
        ),
        DripOutcome::InsufficientUsdc => (StatusCode::OK, decline("insufficient_usdc")),
        DripOutcome::AlreadyHasSol => (StatusCode::OK, decline("already_has_sol")),
        DripOutcome::DailyCap => (StatusCode::OK, decline("daily_cap")),
        DripOutcome::BadWallet => (StatusCode::BAD_REQUEST, decline("invalid_wallet")),
        DripOutcome::Disabled => (StatusCode::OK, decline("disabled")),
        // A send failure is the one "we tried and it broke" case → 502.
        DripOutcome::SendFailed => (StatusCode::BAD_GATEWAY, decline("send_failed")),
    };

    (status, Json(body))
}

/// Build a `funded: false` response with a reason.
fn decline(reason: &str) -> FaucetResponse {
    FaucetResponse {
        funded: false,
        reason: Some(reason.to_string()),
        tx_signature: None,
        lamports: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    const VALID_WALLET: &str = "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM";

    fn test_params() -> FaucetParams {
        FaucetParams {
            drip_lamports: 10_000_000,
            usdc_floor_atomic: 100_000,
            sol_low_water_lamports: 3_000_000,
            daily_cap_lamports: 1_000_000_000,
        }
    }

    /// In-memory ledger that models the PK-reservation semantics: a single
    /// `reserved` flag (one wallet under test) guarded so concurrent `reserve`
    /// calls behave like the DB's `ON CONFLICT DO NOTHING`.
    #[derive(Default)]
    struct MockLedger {
        reserved: Mutex<std::collections::HashMap<String, Option<String>>>,
        day_total: AtomicUsize,
        reserve_fail: bool,
    }

    #[async_trait]
    impl GasLedger for MockLedger {
        async fn reserve(&self, wallet_b58: &str, lamports: u64) -> Result<bool, FaucetError> {
            if self.reserve_fail {
                return Err(FaucetError::Ledger("forced reserve failure".to_string()));
            }
            let mut g = self.reserved.lock().unwrap();
            if g.contains_key(wallet_b58) {
                return Ok(false);
            }
            g.insert(wallet_b58.to_string(), None);
            self.day_total
                .fetch_add(lamports as usize, Ordering::SeqCst);
            Ok(true)
        }
        async fn delete_reservation(&self, wallet_b58: &str) -> Result<(), FaucetError> {
            let mut g = self.reserved.lock().unwrap();
            if let Some(sig) = g.get(wallet_b58) {
                // Mirror prod: only un-broadcast rows are deletable.
                if sig.is_none() {
                    g.remove(wallet_b58);
                }
            }
            Ok(())
        }
        async fn day_total_lamports(&self) -> Result<u64, FaucetError> {
            Ok(self.day_total.load(Ordering::SeqCst) as u64)
        }
        async fn record_signature(
            &self,
            wallet_b58: &str,
            tx_signature: &str,
        ) -> Result<(), FaucetError> {
            let mut g = self.reserved.lock().unwrap();
            g.insert(wallet_b58.to_string(), Some(tx_signature.to_string()));
            Ok(())
        }
        async fn prior_signature(&self, wallet_b58: &str) -> Result<Option<String>, FaucetError> {
            Ok(self
                .reserved
                .lock()
                .unwrap()
                .get(wallet_b58)
                .cloned()
                .flatten())
        }
    }

    struct MockSource {
        usdc: u64,
        sol: u64,
        send_ok: bool,
        sends: AtomicUsize,
    }

    impl MockSource {
        fn new(usdc: u64, sol: u64, send_ok: bool) -> Self {
            Self {
                usdc,
                sol,
                send_ok,
                sends: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl GasSource for MockSource {
        async fn sol_balance(&self, _w: &str) -> Result<u64, FaucetError> {
            Ok(self.sol)
        }
        async fn usdc_balance(&self, _w: &str) -> Result<u64, FaucetError> {
            Ok(self.usdc)
        }
        async fn send_drip(&self, _w: &str, _l: u64) -> Result<String, FaucetError> {
            self.sends.fetch_add(1, Ordering::SeqCst);
            if self.send_ok {
                Ok("MockDripSig".to_string())
            } else {
                Err(FaucetError::Rpc("forced send failure".to_string()))
            }
        }
    }

    fn faucet_with(source: Arc<MockSource>, ledger: Arc<MockLedger>) -> Faucet {
        Faucet::new(test_params(), source, ledger)
    }

    #[tokio::test]
    async fn happy_path_drips_once() {
        let source = Arc::new(MockSource::new(100_000, 0, true));
        let ledger = Arc::new(MockLedger::default());
        let faucet = faucet_with(source.clone(), ledger.clone());

        let out = faucet.drip(VALID_WALLET).await;
        assert_eq!(
            out,
            DripOutcome::Funded {
                tx_signature: "MockDripSig".to_string(),
                lamports: 10_000_000
            }
        );
        assert_eq!(source.sends.load(Ordering::SeqCst), 1);
        // Signature persisted.
        assert_eq!(
            ledger.prior_signature(VALID_WALLET).await.unwrap(),
            Some("MockDripSig".to_string())
        );
    }

    #[tokio::test]
    async fn bad_wallet_rejected_before_reservation() {
        let source = Arc::new(MockSource::new(100_000, 0, true));
        let ledger = Arc::new(MockLedger::default());
        let faucet = faucet_with(source.clone(), ledger);
        let out = faucet.drip("not-a-valid-pubkey!!!").await;
        assert_eq!(out, DripOutcome::BadWallet);
        assert_eq!(
            source.sends.load(Ordering::SeqCst),
            0,
            "no send for bad wallet"
        );
    }

    #[tokio::test]
    async fn already_funded_on_reservation_conflict() {
        let source = Arc::new(MockSource::new(100_000, 0, true));
        let ledger = Arc::new(MockLedger::default());
        let faucet = faucet_with(source.clone(), ledger.clone());

        // First drip funds + records a signature.
        let _ = faucet.drip(VALID_WALLET).await;
        // Second drip hits the reservation conflict → already_funded with the
        // prior signature.
        let out = faucet.drip(VALID_WALLET).await;
        assert_eq!(
            out,
            DripOutcome::AlreadyFunded {
                tx_signature: Some("MockDripSig".to_string())
            }
        );
        assert_eq!(
            source.sends.load(Ordering::SeqCst),
            1,
            "must not double-drip"
        );
    }

    #[tokio::test]
    async fn insufficient_usdc_declines_and_rolls_back() {
        // USDC below the floor.
        let source = Arc::new(MockSource::new(99_999, 0, true));
        let ledger = Arc::new(MockLedger::default());
        let faucet = faucet_with(source.clone(), ledger.clone());

        let out = faucet.drip(VALID_WALLET).await;
        assert_eq!(out, DripOutcome::InsufficientUsdc);
        assert_eq!(source.sends.load(Ordering::SeqCst), 0);
        // Reservation rolled back → a later (now-funded) request can succeed.
        let source2 = Arc::new(MockSource::new(100_000, 0, true));
        let faucet2 = Faucet::new(test_params(), source2.clone(), ledger);
        assert!(matches!(
            faucet2.drip(VALID_WALLET).await,
            DripOutcome::Funded { .. }
        ));
    }

    #[tokio::test]
    async fn usdc_floor_boundary_exactly_at_floor_qualifies() {
        // solvela-fintech: the comparison is `usdc >= floor` (integer atomic).
        // Exactly at the floor must qualify; one below must not.
        let at_floor = Arc::new(MockSource::new(100_000, 0, true));
        let f1 = faucet_with(at_floor, Arc::new(MockLedger::default()));
        assert!(matches!(
            f1.drip(VALID_WALLET).await,
            DripOutcome::Funded { .. }
        ));

        let below = Arc::new(MockSource::new(100_000 - 1, 0, true));
        let f2 = faucet_with(below, Arc::new(MockLedger::default()));
        assert_eq!(f2.drip(VALID_WALLET).await, DripOutcome::InsufficientUsdc);
    }

    #[tokio::test]
    async fn already_has_sol_when_above_low_water() {
        // SOL at the low-water mark (not strictly below) → declined.
        let source = Arc::new(MockSource::new(100_000, 3_000_000, true));
        let faucet = faucet_with(source.clone(), Arc::new(MockLedger::default()));
        assert_eq!(faucet.drip(VALID_WALLET).await, DripOutcome::AlreadyHasSol);
        assert_eq!(source.sends.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn daily_cap_declines_and_rolls_back() {
        let source = Arc::new(MockSource::new(100_000, 0, true));
        let ledger = Arc::new(MockLedger::default());
        // Pre-load the day total above the cap.
        ledger.day_total.store(2_000_000_000, Ordering::SeqCst);
        let faucet = faucet_with(source.clone(), ledger.clone());
        let out = faucet.drip(VALID_WALLET).await;
        assert_eq!(out, DripOutcome::DailyCap);
        assert_eq!(source.sends.load(Ordering::SeqCst), 0);
        // Rolled back → no lingering reservation.
        assert_eq!(ledger.prior_signature(VALID_WALLET).await.unwrap(), None);
        assert!(!ledger.reserved.lock().unwrap().contains_key(VALID_WALLET));
    }

    #[tokio::test]
    async fn send_failure_rolls_back_for_retry() {
        let source = Arc::new(MockSource::new(100_000, 0, false)); // send fails
        let ledger = Arc::new(MockLedger::default());
        let faucet = faucet_with(source.clone(), ledger.clone());
        let out = faucet.drip(VALID_WALLET).await;
        assert_eq!(out, DripOutcome::SendFailed);
        assert_eq!(source.sends.load(Ordering::SeqCst), 1);
        // Reservation rolled back so a retry is possible.
        assert!(!ledger.reserved.lock().unwrap().contains_key(VALID_WALLET));
    }

    #[tokio::test]
    async fn reserve_failure_refuses_without_drip() {
        let source = Arc::new(MockSource::new(100_000, 0, true));
        let ledger = Arc::new(MockLedger {
            reserve_fail: true,
            ..MockLedger::default()
        });
        let faucet = faucet_with(source.clone(), ledger);
        let out = faucet.drip(VALID_WALLET).await;
        assert_eq!(out, DripOutcome::SendFailed);
        assert_eq!(
            source.sends.load(Ordering::SeqCst),
            0,
            "never drip if reservation can't be guaranteed"
        );
    }

    #[tokio::test]
    async fn concurrent_requests_drip_at_most_once() {
        // Two concurrent drips for the SAME wallet: the PK-style reservation
        // must let at most one through to send.
        let source = Arc::new(MockSource::new(100_000, 0, true));
        let ledger = Arc::new(MockLedger::default());
        let f1 = Arc::new(faucet_with(source.clone(), ledger.clone()));
        let f2 = f1.clone();

        let (a, b) = tokio::join!(async { f1.drip(VALID_WALLET).await }, async {
            f2.drip(VALID_WALLET).await
        });

        let funded = [&a, &b]
            .iter()
            .filter(|o| matches!(o, DripOutcome::Funded { .. }))
            .count();
        let already = [&a, &b]
            .iter()
            .filter(|o| matches!(o, DripOutcome::AlreadyFunded { .. }))
            .count();
        assert_eq!(funded, 1, "exactly one drip must fund");
        assert_eq!(already, 1, "the other must see already_funded");
        assert_eq!(source.sends.load(Ordering::SeqCst), 1, "at most one send");
    }

    #[test]
    fn rpc_gas_source_rejects_bad_keypair() {
        let res = RpcGasSource::from_keypair_b58(
            reqwest::Client::new(),
            "https://api.devnet.solana.com".to_string(),
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            "not-base58!!!",
        );
        assert!(matches!(res, Err(RpcGasSourceError::InvalidKeypair(_))));
    }

    #[test]
    fn rpc_gas_source_debug_redacts_keypair() {
        use ed25519_dalek::SigningKey;
        let sk = SigningKey::from_bytes(&[9u8; 32]);
        let mut kp = [0u8; 64];
        kp[..32].copy_from_slice(&sk.to_bytes());
        kp[32..].copy_from_slice(sk.verifying_key().as_bytes());
        let kp_b58 = bs58::encode(kp).into_string();

        let src = RpcGasSource::from_keypair_b58(
            reqwest::Client::new(),
            "https://api.devnet.solana.com".to_string(),
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            &kp_b58,
        )
        .expect("valid keypair");

        let dbg = format!("{src:?}");
        assert!(
            dbg.contains("[REDACTED]"),
            "keypair must be redacted: {dbg}"
        );
        assert!(!dbg.contains(&kp_b58), "raw keypair must not leak: {dbg}");
        // Pubkey is safe to show.
        assert!(dbg.contains(src.source_pubkey_b58()));
    }
}
