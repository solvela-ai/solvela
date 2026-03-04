//! NoncePool — manages durable nonce account addresses for pre-signed Solana transactions.
//!
//! Durable nonces allow transactions to be pre-signed and submitted without blockhash expiry.
//! The pool maps each fee payer wallet to its paired nonce account.
//!
//! Env var convention:
//! - `RCR_SOLANA__NONCE_ACCOUNT` — primary nonce account pubkey (paired with primary fee payer)
//! - `RCR_SOLANA__NONCE_ACCOUNT_2` through `RCR_SOLANA__NONCE_ACCOUNT_8`

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

/// Pool of durable nonce accounts with round-robin rotation.
///
/// Clients fetch a nonce entry and embed the nonce value in their pre-signed
/// payment transaction instead of a recent blockhash.
#[derive(Debug)]
pub struct NoncePool {
    entries: Vec<NonceEntry>,
    counter: AtomicUsize,
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

impl NoncePool {
    /// Create a pool from a list of nonce entries, validating each pubkey.
    pub fn from_entries(entries: Vec<NonceEntry>) -> Result<Self, NoncePoolError> {
        for entry in &entries {
            validate_base58_pubkey(&entry.nonce_account)?;
            validate_base58_pubkey(&entry.authority)?;
        }

        Ok(Self {
            entries,
            counter: AtomicUsize::new(0),
        })
    }

    /// Load nonce accounts from environment variables.
    ///
    /// Reads:
    /// - `RCR_SOLANA__NONCE_ACCOUNT` + `RCR_SOLANA__FEE_PAYER_KEY` (primary, index 0 authority
    ///   is derived from the key, but we only need the pubkey — stored separately from the pool
    ///   concept, so we read the authority pubkey from `RCR_SOLANA__NONCE_AUTHORITY` env var or
    ///   fall back to a placeholder)
    ///
    /// Actually: the authority pubkey is the fee payer's pubkey. Since `FeePayerPool` derives
    /// the pubkey from bytes 32..64 of the keypair, and we want to avoid re-deriving it here,
    /// the operator must supply both `RCR_SOLANA__NONCE_ACCOUNT` and
    /// `RCR_SOLANA__NONCE_AUTHORITY` (the fee payer's base58 pubkey).
    ///
    /// Returns an **empty** pool (not an error) if no nonce accounts are configured.
    /// This is intentional — nonce accounts require manual on-chain setup.
    pub fn from_env() -> Self {
        let mut entries = Vec::new();

        // Primary nonce account (index 0)
        if let (Ok(account), Ok(authority)) = (
            std::env::var("RCR_SOLANA__NONCE_ACCOUNT"),
            std::env::var("RCR_SOLANA__NONCE_AUTHORITY"),
        ) {
            let account = account.trim().to_string();
            let authority = authority.trim().to_string();
            if !account.is_empty()
                && !authority.is_empty()
                && validate_base58_pubkey(&account).is_ok()
                && validate_base58_pubkey(&authority).is_ok()
            {
                entries.push(NonceEntry {
                    nonce_account: account,
                    authority,
                });
            }
        }

        // Additional nonce accounts (indices 1..7)
        for i in 2..=MAX_NONCE_ACCOUNTS {
            let account_var = format!("RCR_SOLANA__NONCE_ACCOUNT_{i}");
            let authority_var = format!("RCR_SOLANA__NONCE_AUTHORITY_{i}");
            if let (Ok(account), Ok(authority)) =
                (std::env::var(&account_var), std::env::var(&authority_var))
            {
                let account = account.trim().to_string();
                let authority = authority.trim().to_string();
                if !account.is_empty()
                    && !authority.is_empty()
                    && validate_base58_pubkey(&account).is_ok()
                    && validate_base58_pubkey(&authority).is_ok()
                {
                    entries.push(NonceEntry {
                        nonce_account: account,
                        authority,
                    });
                }
            }
        }

        Self {
            entries,
            counter: AtomicUsize::new(0),
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
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .unwrap_or_default();

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getAccountInfo",
            "params": [
                entry.nonce_account,
                { "encoding": "base64" }
            ]
        });

        let response = client
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

        // Nonce hash is at bytes 40..72
        if data.len() < 72 {
            return Err(NoncePoolError::ParseError(format!(
                "nonce account data too short: {} bytes (need ≥ 72)",
                data.len()
            )));
        }

        let nonce_bytes = &data[40..72];
        let nonce_value = bs58::encode(nonce_bytes).into_string();
        Ok(nonce_value)
    }
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
        // Ensure env vars are NOT set (they shouldn't be in CI)
        // We unset them to be safe.
        // SAFETY: tests run in a single-threaded context for env var manipulation.
        let _guard = EnvGuard::clear(&["RCR_SOLANA__NONCE_ACCOUNT", "RCR_SOLANA__NONCE_AUTHORITY"]);

        let pool = NoncePool::from_env();
        assert!(
            pool.is_empty(),
            "from_env() with no env vars must return empty pool"
        );
        assert!(
            pool.next().is_none(),
            "next() on from_env empty pool must return None"
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
    // Helper: simple env-var cleanup guard for tests
    // -----------------------------------------------------------------------

    struct EnvGuard {
        vars: Vec<(String, Option<String>)>,
    }

    impl EnvGuard {
        fn clear(names: &[&str]) -> Self {
            let vars = names
                .iter()
                .map(|&name| {
                    let prev = std::env::var(name).ok();
                    // SAFETY: test isolation — each test that uses this guard
                    // must not run in parallel with tests that read the same vars.
                    // Cargo runs tests in parallel by default; use `--test-threads=1`
                    // if flakiness occurs, or prefix vars with a unique UUID.
                    std::env::remove_var(name);
                    (name.to_string(), prev)
                })
                .collect();
            Self { vars }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (name, prev) in &self.vars {
                match prev {
                    Some(val) => std::env::set_var(name, val),
                    None => std::env::remove_var(name),
                }
            }
        }
    }
}
