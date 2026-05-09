//! NoncePool — manages durable nonce account addresses for pre-signed Solana transactions.
//!
//! Durable nonces allow transactions to be pre-signed and submitted without blockhash expiry.
//! The pool maps each fee payer wallet to its paired nonce account.
//!
//! Env var convention (canonical names; legacy `RCR_*` accepted as fallback):
//! - `SOLVELA_SOLANA__NONCE_ACCOUNT` — primary nonce account pubkey (paired with primary fee payer)
//! - `SOLVELA_SOLANA__NONCE_ACCOUNT_2` through `SOLVELA_SOLANA__NONCE_ACCOUNT_8`
//! - `SOLVELA_SOLANA__NONCE_AUTHORITY` — authority pubkey for primary nonce account
//! - `SOLVELA_SOLANA__NONCE_AUTHORITY_2` through `SOLVELA_SOLANA__NONCE_AUTHORITY_8`

use std::sync::atomic::{AtomicUsize, Ordering};

/// Maximum number of nonce accounts supported (mirrors FeePayerPool).
const MAX_NONCE_ACCOUNTS: usize = 8;

/// A single nonce account entry paired with its authority (fee payer).
#[derive(Debug, Clone)]
pub struct NonceEntry {
    /// Base58 pubkey of the on-chain nonce account.
    pub nonce_account: String,
    /// Base58 pubkey of the fee payer wallet that is the nonce authority.
    pub authority: String,
}

/// HTTP request timeout for nonce-account RPC fetches.
///
/// 10 s mirrors the previous per-call value but is now defined once instead of
/// being re-baked into a fresh `reqwest::Client` on every call.
const RPC_HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Pool of durable nonce accounts with round-robin rotation.
///
/// Clients fetch a nonce entry and embed the nonce value in their pre-signed
/// payment transaction instead of a recent blockhash.
///
/// `http_client` is shared across all `fetch_nonce_value` calls so connection
/// reuse, TLS sessions, and DNS caching are not torn down between RPC calls.
/// Mirror the pattern used by `EscrowVerifier`.
#[derive(Debug)]
pub struct NoncePool {
    entries: Vec<NonceEntry>,
    counter: AtomicUsize,
    http_client: reqwest::Client,
}

/// Errors from `NoncePool` construction or nonce fetching.
#[derive(Debug, thiserror::Error)]
pub enum NoncePoolError {
    #[error("invalid nonce account pubkey: {0}")]
    InvalidPubkey(String),
    #[error("RPC request failed: {0}")]
    RpcError(String),
    #[error("failed to parse nonce account data: {0}")]
    ParseError(String),
}

/// Try `solvela_name` first; fall back to `rcr_name` for backwards
/// compatibility. Returns `None` only when neither variable is set.
///
/// Reader is injected so tests can avoid mutating the process env — that
/// is not thread-safe and Rust nightly is moving toward marking
/// `std::env::set_var` as `unsafe`.
fn read_env_with<F: Fn(&str) -> Option<String>>(
    solvela_name: &str,
    rcr_name: &str,
    read: F,
) -> Option<String> {
    read(solvela_name).or_else(|| read(rcr_name))
}

impl NoncePool {
    /// Build the shared HTTP client used for nonce RPC fetches.
    ///
    /// Falls back to `reqwest::Client::new()` if the builder fails — this can
    /// only happen if the underlying TLS backend cannot initialise, in which
    /// case downstream RPC calls will fail anyway.
    fn build_http_client() -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(RPC_HTTP_TIMEOUT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    }

    /// Create a pool from a list of nonce entries, validating each pubkey.
    pub fn from_entries(entries: Vec<NonceEntry>) -> Result<Self, NoncePoolError> {
        for entry in &entries {
            validate_base58_pubkey(&entry.nonce_account)?;
            validate_base58_pubkey(&entry.authority)?;
        }

        Ok(Self {
            entries,
            counter: AtomicUsize::new(0),
            http_client: Self::build_http_client(),
        })
    }

    /// Load nonce accounts from environment variables.
    ///
    /// Reads (canonical `SOLVELA_*` names; legacy `RCR_*` accepted as fallback):
    /// - `SOLVELA_SOLANA__NONCE_ACCOUNT` — primary nonce account pubkey (index 0)
    /// - `SOLVELA_SOLANA__NONCE_AUTHORITY` — authority pubkey for the primary nonce account
    ///
    /// The authority pubkey is the fee payer's pubkey. Since `FeePayerPool` derives
    /// the pubkey from bytes 32..64 of the keypair, and we want to avoid re-deriving it here,
    /// the operator must supply both `SOLVELA_SOLANA__NONCE_ACCOUNT` and
    /// `SOLVELA_SOLANA__NONCE_AUTHORITY` (the fee payer's base58 pubkey).
    ///
    /// Returns an **empty** pool (not an error) if no nonce accounts are configured.
    /// This is intentional — nonce accounts require manual on-chain setup.
    pub fn from_env() -> Self {
        Self::from_env_reader(|name| std::env::var(name).ok())
    }

    /// `from_env` with the env reader injected — used by tests to avoid
    /// mutating the process env (which is not thread-safe; nightly Rust
    /// is moving toward marking `std::env::set_var` as `unsafe`).
    fn from_env_reader<F: Fn(&str) -> Option<String>>(read: F) -> Self {
        let mut entries = Vec::new();
        let collect_entry = |solvela_account: &str,
                             rcr_account: &str,
                             solvela_authority: &str,
                             rcr_authority: &str|
         -> Option<NonceEntry> {
            let account = read_env_with(solvela_account, rcr_account, &read)?;
            let authority = read_env_with(solvela_authority, rcr_authority, &read)?;
            let account = account.trim().to_string();
            let authority = authority.trim().to_string();
            if account.is_empty()
                || authority.is_empty()
                || validate_base58_pubkey(&account).is_err()
                || validate_base58_pubkey(&authority).is_err()
            {
                return None;
            }
            Some(NonceEntry {
                nonce_account: account,
                authority,
            })
        };

        // Primary nonce account (index 0)
        if let Some(entry) = collect_entry(
            "SOLVELA_SOLANA__NONCE_ACCOUNT",
            "RCR_SOLANA__NONCE_ACCOUNT",
            "SOLVELA_SOLANA__NONCE_AUTHORITY",
            "RCR_SOLANA__NONCE_AUTHORITY",
        ) {
            entries.push(entry);
        }

        // Additional nonce accounts (indices 1..MAX_NONCE_ACCOUNTS)
        for i in 2..=MAX_NONCE_ACCOUNTS {
            if let Some(entry) = collect_entry(
                &format!("SOLVELA_SOLANA__NONCE_ACCOUNT_{i}"),
                &format!("RCR_SOLANA__NONCE_ACCOUNT_{i}"),
                &format!("SOLVELA_SOLANA__NONCE_AUTHORITY_{i}"),
                &format!("RCR_SOLANA__NONCE_AUTHORITY_{i}"),
            ) {
                entries.push(entry);
            }
        }

        Self {
            entries,
            counter: AtomicUsize::new(0),
            http_client: Self::build_http_client(),
        }
    }

    /// Round-robin selection. Returns `None` when the pool is empty.
    ///
    /// Clients that receive `None` must fall back to using a recent blockhash.
    pub fn next(&self) -> Option<&NonceEntry> {
        if self.entries.is_empty() {
            return None;
        }
        let idx = self.counter.fetch_add(1, Ordering::Relaxed) % self.entries.len();
        Some(&self.entries[idx])
    }

    /// Number of nonce accounts in the pool.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the pool is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate over all entries in the pool.
    pub fn entries_iter(&self) -> impl Iterator<Item = &NonceEntry> {
        self.entries.iter()
    }

    /// Fetch the current nonce value from the on-chain nonce account via RPC.
    ///
    /// The nonce account data layout (system program nonce):
    /// - bytes  0..4  : version (u32 LE)
    /// - bytes  4..8  : state (u32 LE, 1 = initialized)
    /// - bytes  8..40 : authority pubkey (32 bytes)
    /// - bytes 40..72 : nonce hash (32 bytes) ← THIS is the durable nonce value
    ///
    /// Returns the nonce value as a base58-encoded string.
    pub async fn fetch_nonce_value(
        &self,
        rpc_url: &str,
        entry: &NonceEntry,
    ) -> Result<String, NoncePoolError> {
        // TODO(production): add a per-account nonce-value cache with a ~5-second TTL.
        // The nonce only changes when a transaction using it lands on-chain, so caching
        // it briefly avoids hammering the RPC on high-traffic deployments.  A simple
        // `tokio::sync::Mutex<HashMap<String, (String, Instant)>>` on AppState suffices.
        // Also add per-endpoint rate limiting at the gateway layer to prevent DoS.

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getAccountInfo",
            "params": [
                entry.nonce_account,
                { "encoding": "base64" }
            ]
        });

        // Reuse the pool-wide `http_client` so connection pooling, TLS sessions,
        // and DNS caching survive across nonce fetches.
        let response = self
            .http_client
            .post(rpc_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| NoncePoolError::RpcError(e.to_string()))?;

        let json: serde_json::Value = response
            .json()
            .await
            .map_err(|e| NoncePoolError::RpcError(e.to_string()))?;

        if let Some(error) = json.get("error") {
            return Err(NoncePoolError::RpcError(error.to_string()));
        }

        // Navigate: result.value.data[0] → base64 string
        let data_b64 = json
            .pointer("/result/value/data/0")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                NoncePoolError::ParseError(
                    "missing or null nonce account — account may not exist on-chain".to_string(),
                )
            })?;

        let data = base64_decode(data_b64)
            .map_err(|e| NoncePoolError::ParseError(format!("base64 decode failed: {e}")))?;

        parse_nonce_account_data(&data, entry)
    }
}

/// Parse a Solana Nonce account's raw data and return the current nonce
/// hash, after verifying the on-chain authority matches the pool entry.
///
/// Solana Nonce account layout (80 bytes):
/// `version(0..4) + state(4..8) + authority(8..40) + nonce_hash(40..72)
/// + fee_calculator(72..80)`.
///
/// M8 — verifies that bytes 8..40 (on-chain authority) match
/// `entry.authority`. A mismatch usually means the operator misconfigured
/// the pool (wrong authority pubkey for the given nonce account) —
/// proceeding would just produce opaque AdvanceNonceAccount signature
/// failures later. Surfacing the misconfiguration here makes it
/// debuggable.
fn parse_nonce_account_data(data: &[u8], entry: &NonceEntry) -> Result<String, NoncePoolError> {
    if data.len() < 72 {
        return Err(NoncePoolError::ParseError(format!(
            "nonce account data too short: {} bytes (need ≥ 72)",
            data.len()
        )));
    }

    let onchain_authority = bs58::encode(&data[8..40]).into_string();
    if onchain_authority != entry.authority {
        return Err(NoncePoolError::ParseError(format!(
            "nonce account authority mismatch for {}: pool entry says {}, on-chain says {}",
            entry.nonce_account, entry.authority, onchain_authority
        )));
    }

    let nonce_bytes = &data[40..72];
    Ok(bs58::encode(nonce_bytes).into_string())
}

/// Validate that `s` is a valid base58-encoded 32-byte pubkey.
fn validate_base58_pubkey(s: &str) -> Result<(), NoncePoolError> {
    let decoded = bs58::decode(s)
        .into_vec()
        .map_err(|_| NoncePoolError::InvalidPubkey(s.to_string()))?;

    if decoded.len() != 32 {
        return Err(NoncePoolError::InvalidPubkey(format!(
            "{s} decoded to {} bytes, expected 32",
            decoded.len()
        )));
    }
    Ok(())
}

/// Decode base64 without exposing the engine import everywhere.
fn base64_decode(s: &str) -> Result<Vec<u8>, base64::DecodeError> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(s)
}

// ---------------------------------------------------------------------------
// Tests — written FIRST (RED phase) before any implementation is complete
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A valid mainnet USDC mint — 32 bytes, valid base58.
    const VALID_PUBKEY_1: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
    /// System program — another valid 32-byte base58 pubkey.
    const VALID_PUBKEY_2: &str = "11111111111111111111111111111111";
    /// A third distinct valid pubkey.
    const VALID_PUBKEY_3: &str = "So11111111111111111111111111111111111111112";

    fn make_entry(nonce_account: &str, authority: &str) -> NonceEntry {
        NonceEntry {
            nonce_account: nonce_account.to_string(),
            authority: authority.to_string(),
        }
    }

    // -----------------------------------------------------------------------
    // 1. test_empty_pool_returns_none
    // -----------------------------------------------------------------------

    #[test]
    fn test_empty_pool_returns_none() {
        let pool = NoncePool::from_entries(vec![]).expect("empty pool should be Ok");
        assert!(pool.is_empty());
        assert_eq!(pool.len(), 0);
        assert!(
            pool.next().is_none(),
            "next() on empty pool must return None"
        );
    }

    // -----------------------------------------------------------------------
    // 2. test_single_entry_pool
    // -----------------------------------------------------------------------

    #[test]
    fn test_single_entry_pool() {
        let entry = make_entry(VALID_PUBKEY_1, VALID_PUBKEY_2);
        let pool = NoncePool::from_entries(vec![entry]).expect("pool with one entry should be Ok");

        assert_eq!(pool.len(), 1);
        assert!(!pool.is_empty());

        let got = pool
            .next()
            .expect("next() should return Some for non-empty pool");
        assert_eq!(got.nonce_account, VALID_PUBKEY_1);
        assert_eq!(got.authority, VALID_PUBKEY_2);
    }

    // -----------------------------------------------------------------------
    // 3. test_round_robin
    // -----------------------------------------------------------------------

    #[test]
    fn test_round_robin() {
        let entries = vec![
            make_entry(VALID_PUBKEY_1, VALID_PUBKEY_2),
            make_entry(VALID_PUBKEY_2, VALID_PUBKEY_1),
            make_entry(VALID_PUBKEY_3, VALID_PUBKEY_2),
        ];
        let pool = NoncePool::from_entries(entries).expect("pool with 3 entries should be Ok");

        // Call next() 6 times → should cycle through all 3 entries twice
        let results: Vec<String> = (0..6)
            .map(|_| pool.next().expect("must return Some").nonce_account.clone())
            .collect();

        // First 3 calls: one of each
        let first_cycle: std::collections::HashSet<_> = results[0..3].iter().collect();
        assert_eq!(
            first_cycle.len(),
            3,
            "first 3 calls must return 3 distinct entries"
        );

        // Second 3 calls must repeat the same pattern as the first 3
        assert_eq!(
            results[0], results[3],
            "4th call must match 1st (round-robin wrap)"
        );
        assert_eq!(
            results[1], results[4],
            "5th call must match 2nd (round-robin wrap)"
        );
        assert_eq!(
            results[2], results[5],
            "6th call must match 3rd (round-robin wrap)"
        );
    }

    // -----------------------------------------------------------------------
    // 4. test_from_env_empty
    // -----------------------------------------------------------------------

    #[test]
    fn test_from_env_empty() {
        // Use the testable variant with a synthetic reader that returns
        // None for every var name. No process-env mutation required, so
        // this test is parallel-safe.
        let pool = NoncePool::from_env_reader(|_| None);
        assert!(
            pool.is_empty(),
            "from_env_reader with no values must return empty pool"
        );
        assert!(
            pool.next().is_none(),
            "next() on empty pool must return None"
        );
    }

    #[test]
    fn test_from_env_reader_picks_up_primary_entry() {
        let read = |name: &str| -> Option<String> {
            match name {
                "SOLVELA_SOLANA__NONCE_ACCOUNT" => Some(VALID_PUBKEY_1.to_string()),
                "SOLVELA_SOLANA__NONCE_AUTHORITY" => Some(VALID_PUBKEY_2.to_string()),
                _ => None,
            }
        };
        let pool = NoncePool::from_env_reader(read);
        assert_eq!(pool.len(), 1);
        let entry = pool.next().expect("primary entry");
        assert_eq!(entry.nonce_account, VALID_PUBKEY_1);
        assert_eq!(entry.authority, VALID_PUBKEY_2);
    }

    #[test]
    fn test_from_env_reader_falls_back_to_legacy_rcr_names() {
        let read = |name: &str| -> Option<String> {
            match name {
                "RCR_SOLANA__NONCE_ACCOUNT" => Some(VALID_PUBKEY_1.to_string()),
                "RCR_SOLANA__NONCE_AUTHORITY" => Some(VALID_PUBKEY_2.to_string()),
                _ => None, // SOLVELA_* names not set
            }
        };
        let pool = NoncePool::from_env_reader(read);
        assert_eq!(pool.len(), 1, "legacy RCR_* names must still work");
    }

    #[test]
    fn test_from_env_reader_skips_invalid_pubkeys() {
        let read = |name: &str| -> Option<String> {
            match name {
                "SOLVELA_SOLANA__NONCE_ACCOUNT" => Some("garbage".to_string()),
                "SOLVELA_SOLANA__NONCE_AUTHORITY" => Some(VALID_PUBKEY_2.to_string()),
                _ => None,
            }
        };
        let pool = NoncePool::from_env_reader(read);
        assert!(
            pool.is_empty(),
            "invalid nonce_account pubkey must skip the entry, not error"
        );
    }

    // -----------------------------------------------------------------------
    // 5. test_nonce_account_pubkey_format
    // -----------------------------------------------------------------------

    #[test]
    fn test_nonce_account_pubkey_format_valid() {
        // Valid base58 32-byte pubkeys
        let entry = make_entry(VALID_PUBKEY_1, VALID_PUBKEY_2);
        let result = NoncePool::from_entries(vec![entry]);
        assert!(
            result.is_ok(),
            "valid pubkeys must produce Ok, got: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_nonce_account_pubkey_format_invalid_garbage() {
        let entry = make_entry("not-a-valid-base58-pubkey!!!", VALID_PUBKEY_2);
        let result = NoncePool::from_entries(vec![entry]);
        assert!(
            result.is_err(),
            "garbage nonce_account pubkey must produce Err"
        );
        assert!(
            matches!(result.unwrap_err(), NoncePoolError::InvalidPubkey(_)),
            "error must be NoncePoolError::InvalidPubkey"
        );
    }

    #[test]
    fn test_nonce_account_pubkey_format_wrong_length() {
        // Valid base58 but only 4 bytes when decoded
        let short_b58 = bs58::encode([1u8, 2u8, 3u8, 4u8]).into_string();
        let entry = make_entry(&short_b58, VALID_PUBKEY_2);
        let result = NoncePool::from_entries(vec![entry]);
        assert!(result.is_err(), "short pubkey must produce Err");
    }

    #[test]
    fn test_authority_pubkey_format_invalid() {
        // Valid nonce account but garbage authority
        let entry = make_entry(VALID_PUBKEY_1, "!!!garbage!!!");
        let result = NoncePool::from_entries(vec![entry]);
        assert!(result.is_err(), "invalid authority pubkey must produce Err");
    }

    // -----------------------------------------------------------------------
    // parse_nonce_account_data: layout, authority check (M8)
    // -----------------------------------------------------------------------

    /// Build a synthetic 80-byte nonce account payload with the given
    /// authority bytes (32 bytes) and nonce hash bytes (32 bytes).
    fn synth_nonce_data(authority: &[u8; 32], nonce: &[u8; 32]) -> Vec<u8> {
        let mut data = vec![0u8; 80];
        // 0..4: state version (zeros for test)
        // 4..8: state internal (zeros for test)
        data[8..40].copy_from_slice(authority);
        data[40..72].copy_from_slice(nonce);
        // 72..80: fee calculator (zeros for test)
        data
    }

    #[test]
    fn parse_nonce_account_data_returns_nonce_when_authority_matches() {
        let authority_bytes = bs58::decode(VALID_PUBKEY_2)
            .into_vec()
            .expect("decode authority");
        let mut auth = [0u8; 32];
        auth.copy_from_slice(&authority_bytes);
        let nonce = [7u8; 32];
        let data = synth_nonce_data(&auth, &nonce);

        let entry = make_entry(VALID_PUBKEY_1, VALID_PUBKEY_2);
        let value = parse_nonce_account_data(&data, &entry).expect("must parse");
        assert_eq!(value, bs58::encode(&nonce).into_string());
    }

    #[test]
    fn parse_nonce_account_data_rejects_authority_mismatch() {
        let wrong_auth = [9u8; 32];
        let nonce = [7u8; 32];
        let data = synth_nonce_data(&wrong_auth, &nonce);

        // Pool entry says authority is VALID_PUBKEY_2, but on-chain is [9; 32]
        let entry = make_entry(VALID_PUBKEY_1, VALID_PUBKEY_2);
        let err = parse_nonce_account_data(&data, &entry).expect_err("must reject");
        let msg = match err {
            NoncePoolError::ParseError(m) => m,
            other => panic!("expected ParseError, got {other:?}"),
        };
        assert!(
            msg.contains("authority mismatch"),
            "error must mention authority mismatch: {msg}"
        );
    }

    #[test]
    fn parse_nonce_account_data_rejects_truncated_data() {
        let entry = make_entry(VALID_PUBKEY_1, VALID_PUBKEY_2);
        let err = parse_nonce_account_data(&[0u8; 40], &entry).expect_err("too short");
        let msg = match err {
            NoncePoolError::ParseError(m) => m,
            other => panic!("expected ParseError, got {other:?}"),
        };
        assert!(
            msg.contains("too short"),
            "expected truncation error: {msg}"
        );
    }
}
