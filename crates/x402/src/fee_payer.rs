//! FeePayerPool — manages multiple fee payer keypairs with round-robin rotation
//! and automatic failover with cooldown.
//!
//! Keys are loaded from environment variables:
//! - `SOLVELA_SOLANA__FEE_PAYER_KEY` (primary, index 0)
//! - `SOLVELA_SOLANA__FEE_PAYER_KEY_2` .. `SOLVELA_SOLANA__FEE_PAYER_KEY_8`
//!
//! Legacy `RCR_SOLANA__FEE_PAYER_KEY[_N]` names are accepted as a fallback for
//! backwards compatibility; `SOLVELA_*` is always tried first.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Maximum number of fee payer keys supported.
const MAX_KEYS: usize = 8;

/// Default cooldown period before retrying a failed wallet.
const DEFAULT_COOLDOWN_SECS: u64 = 60;

/// A single fee payer wallet with failure tracking.
pub struct FeePayerWallet {
    /// Ed25519 keypair bytes (64 bytes: 32-byte secret + 32-byte public).
    /// Private — access via `sign()` or `keypair_bytes()`.
    keypair: [u8; 64],
    /// Base58-encoded public key for logging (derived from keypair bytes 32..64).
    pub pubkey_b58: String,
    /// Index in the pool (for `mark_failed` / logging).
    pub index: usize,
    /// Timestamp when this wallet was last marked as failed. `None` = healthy.
    failed_at: Mutex<Option<Instant>>,
}

impl FeePayerWallet {
    /// Sign `message` with this wallet's ed25519 secret key.
    ///
    /// Returns 64 raw signature bytes, or a [`FeePayerError`] if the keypair
    /// bytes are invalid.
    pub fn sign(&self, message: &[u8]) -> Result<[u8; 64], FeePayerError> {
        use ed25519_dalek::{Signer, SigningKey};
        // from_keypair_bytes expects [secret || public] which is exactly our layout.
        let signing_key = SigningKey::from_keypair_bytes(&self.keypair)
            .map_err(|e| FeePayerError::SigningFailed(e.to_string()))?;
        Ok(signing_key.sign(message).to_bytes())
    }

    /// Return a reference to the raw 64-byte keypair.
    ///
    /// **Only for trusted signing code** (e.g. escrow claimer) that needs raw
    /// bytes to build transactions.  Do not use for logging or serialisation.
    pub fn keypair_bytes(&self) -> &[u8; 64] {
        &self.keypair
    }
}

impl Drop for FeePayerWallet {
    fn drop(&mut self) {
        // Zero out secret key material on drop.
        self.keypair.iter_mut().for_each(|b| *b = 0);
    }
}

impl std::fmt::Debug for FeePayerWallet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeePayerWallet")
            .field("pubkey_b58", &self.pubkey_b58)
            .field("index", &self.index)
            .field("keypair", &"[REDACTED]")
            .finish()
    }
}

/// Pool of fee payer wallets with round-robin selection and failover.
pub struct FeePayerPool {
    wallets: Vec<Arc<FeePayerWallet>>,
    counter: AtomicUsize,
    cooldown: Duration,
}

/// Errors from `FeePayerPool` construction or selection.
#[derive(Debug, thiserror::Error)]
pub enum FeePayerError {
    #[error("no fee payer keys configured")]
    Empty,
    #[error("invalid base58 keypair: {0}")]
    InvalidKey(String),
    #[error("all fee payer wallets are in cooldown")]
    AllFailed,
    #[error("signing failed: {0}")]
    SigningFailed(String),
}

/// Try `solvela_name` first; fall back to `rcr_name` for backwards compatibility.
/// Returns `None` only when neither variable is set.
fn read_env_var(solvela_name: &str, rcr_name: &str) -> Option<String> {
    std::env::var(solvela_name)
        .or_else(|_| std::env::var(rcr_name))
        .ok()
}

impl FeePayerPool {
    /// Load from a slice of base58-encoded 64-byte keypair strings.
    pub fn from_keys(keys: &[String]) -> Result<Self, FeePayerError> {
        Self::from_keys_with_cooldown(keys, Duration::from_secs(DEFAULT_COOLDOWN_SECS))
    }

    /// Load from keys with a custom cooldown duration (useful for testing).
    pub fn from_keys_with_cooldown(
        keys: &[String],
        cooldown: Duration,
    ) -> Result<Self, FeePayerError> {
        if keys.is_empty() {
            return Err(FeePayerError::Empty);
        }

        let mut wallets = Vec::with_capacity(keys.len());
        for (i, key_b58) in keys.iter().enumerate() {
            let key_bytes = bs58::decode(key_b58)
                .into_vec()
                .map_err(|e| FeePayerError::InvalidKey(format!("key {i}: {e}")))?;

            if key_bytes.len() != 64 {
                return Err(FeePayerError::InvalidKey(format!(
                    "key {i}: expected 64 bytes, got {}",
                    key_bytes.len()
                )));
            }

            let mut keypair = [0u8; 64];
            keypair.copy_from_slice(&key_bytes);

            // Public key is bytes 32..64
            let pubkey_b58 = bs58::encode(&keypair[32..64]).into_string();

            wallets.push(Arc::new(FeePayerWallet {
                keypair,
                pubkey_b58,
                index: i,
                failed_at: Mutex::new(None),
            }));
        }

        Ok(Self {
            wallets,
            counter: AtomicUsize::new(0),
            cooldown,
        })
    }

    /// Load from environment variables following the `SOLVELA_SOLANA__FEE_PAYER_KEY[_N]` convention.
    ///
    /// Reads (canonical names; legacy `RCR_*` accepted as fallback):
    /// - `SOLVELA_SOLANA__FEE_PAYER_KEY` (index 0)
    /// - `SOLVELA_SOLANA__FEE_PAYER_KEY_2` through `SOLVELA_SOLANA__FEE_PAYER_KEY_8`
    pub fn from_env() -> Result<Self, FeePayerError> {
        let mut keys = Vec::new();

        // Primary key (index 0) — try SOLVELA_* first, fall back to legacy RCR_*
        if let Some(k) = read_env_var("SOLVELA_SOLANA__FEE_PAYER_KEY", "RCR_SOLANA__FEE_PAYER_KEY")
        {
            if !k.is_empty() {
                keys.push(k);
            }
        }

        // Additional keys (index 1..7) — try SOLVELA_* first, fall back to legacy RCR_*
        for i in 2..=MAX_KEYS {
            if let Some(k) = read_env_var(
                &format!("SOLVELA_SOLANA__FEE_PAYER_KEY_{i}"),
                &format!("RCR_SOLANA__FEE_PAYER_KEY_{i}"),
            ) {
                if !k.is_empty() {
                    keys.push(k);
                }
            }
        }

        Self::from_keys(&keys)
    }

    /// Round-robin selection, skipping wallets currently in cooldown.
    ///
    /// Tries up to `len()` candidates starting from the current counter position.
    /// A wallet is eligible if it has never failed, or if its cooldown has expired
    /// (in which case the `failed_at` timestamp is cleared).
    pub fn next(&self) -> Result<Arc<FeePayerWallet>, FeePayerError> {
        let n = self.wallets.len();
        if n == 0 {
            return Err(FeePayerError::AllFailed);
        }

        let start = self.counter.fetch_add(1, Ordering::Relaxed);

        for offset in 0..n {
            let idx = (start + offset) % n;
            let wallet = &self.wallets[idx];

            // Check if wallet is in cooldown
            let is_available = {
                let mut guard = wallet
                    .failed_at
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());

                match *guard {
                    None => true, // never failed
                    Some(failed_time) => {
                        if failed_time.elapsed() >= self.cooldown {
                            // Cooldown expired — recover the wallet
                            *guard = None;
                            true
                        } else {
                            false
                        }
                    }
                }
            };

            if is_available {
                // Advance counter past this wallet so next call starts at the next one
                if offset > 0 {
                    // We skipped `offset` wallets, advance counter accordingly
                    self.counter.store(start + offset + 1, Ordering::Relaxed);
                }
                return Ok(Arc::clone(wallet));
            }
        }

        Err(FeePayerError::AllFailed)
    }

    /// Mark a wallet as failed (starts cooldown timer).
    ///
    /// If `index` is out of bounds, this is a no-op.
    pub fn mark_failed(&self, index: usize) {
        if let Some(wallet) = self.wallets.get(index) {
            let mut guard = wallet
                .failed_at
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *guard = Some(Instant::now());
        }
    }

    /// Mark a wallet as healthy (clears cooldown).
    ///
    /// If `index` is out of bounds, this is a no-op.
    pub fn mark_healthy(&self, index: usize) {
        if let Some(wallet) = self.wallets.get(index) {
            let mut guard = wallet
                .failed_at
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *guard = None;
        }
    }

    /// Returns the pubkey strings for all wallets in the pool.
    pub fn pubkeys(&self) -> Vec<String> {
        self.wallets.iter().map(|w| w.pubkey_b58.clone()).collect()
    }

    /// Number of wallets in the pool.
    pub fn len(&self) -> usize {
        self.wallets.len()
    }

    /// Whether the pool is empty.
    pub fn is_empty(&self) -> bool {
        self.wallets.is_empty()
    }
}

impl std::fmt::Debug for FeePayerPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeePayerPool")
            .field("wallet_count", &self.wallets.len())
            .field(
                "pubkeys",
                &self
                    .wallets
                    .iter()
                    .map(|w| w.pubkey_b58.as_str())
                    .collect::<Vec<_>>(),
            )
            .field("cooldown_secs", &self.cooldown.as_secs())
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Tests — written FIRST (RED phase)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Generate a deterministic valid base58-encoded ed25519 keypair (64 bytes)
    /// from a seed byte. Each distinct `seed` produces a distinct keypair.
    fn test_keypair_b58_from_seed(seed: u8) -> String {
        use ed25519_dalek::SigningKey;

        let mut secret = [0u8; 32];
        secret[0] = seed;
        secret[31] = seed.wrapping_add(1); // ensure non-zero variation
        let signing_key = SigningKey::from_bytes(&secret);
        let mut keypair_bytes = [0u8; 64];
        keypair_bytes[..32].copy_from_slice(&signing_key.to_bytes());
        keypair_bytes[32..].copy_from_slice(signing_key.verifying_key().as_bytes());
        bs58::encode(&keypair_bytes).into_string()
    }

    /// Generate N distinct valid keypairs with seeds 1..=N.
    fn test_keypairs(n: usize) -> Vec<String> {
        (1..=n)
            .map(|i| test_keypair_b58_from_seed(i as u8))
            .collect()
    }

    // -----------------------------------------------------------------------
    // 1. Pool loads a single key
    // -----------------------------------------------------------------------

    #[test]
    fn test_pool_loads_single_key() {
        let keys = test_keypairs(1);
        let pool = FeePayerPool::from_keys(&keys).expect("should load 1 key");
        assert_eq!(pool.len(), 1);
        assert!(!pool.is_empty());
    }

    // -----------------------------------------------------------------------
    // 2. Pool loads multiple keys
    // -----------------------------------------------------------------------

    #[test]
    fn test_pool_loads_multiple_keys() {
        let keys = test_keypairs(3);
        let pool = FeePayerPool::from_keys(&keys).expect("should load 3 keys");
        assert_eq!(pool.len(), 3);
    }

    // -----------------------------------------------------------------------
    // 3. Empty keys returns error
    // -----------------------------------------------------------------------

    #[test]
    fn test_pool_empty_fails() {
        let keys: Vec<String> = vec![];
        let result = FeePayerPool::from_keys(&keys);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, FeePayerError::Empty),
            "expected FeePayerError::Empty, got: {err:?}"
        );
    }

    // -----------------------------------------------------------------------
    // 4. Invalid base58 key returns error
    // -----------------------------------------------------------------------

    #[test]
    fn test_pool_invalid_key_fails() {
        let keys = vec!["not-valid-base58!!!".to_string()];
        let result = FeePayerPool::from_keys(&keys);
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), FeePayerError::InvalidKey(_)),
            "expected FeePayerError::InvalidKey"
        );
    }

    #[test]
    fn test_pool_wrong_length_key_fails() {
        // Valid base58 but only 32 bytes (a pubkey, not a keypair)
        let keys = vec!["11111111111111111111111111111111".to_string()];
        let result = FeePayerPool::from_keys(&keys);
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), FeePayerError::InvalidKey(_)),
            "expected FeePayerError::InvalidKey for wrong-length key"
        );
    }

    // -----------------------------------------------------------------------
    // 5. Round-robin selection cycles through all keys
    // -----------------------------------------------------------------------

    #[test]
    fn test_round_robin_selection() {
        let keys = test_keypairs(3);
        let pool = FeePayerPool::from_keys(&keys).expect("should load 3 keys");

        // Collect pubkeys from the pool to verify round-robin order
        let w0 = pool.next().expect("next() should return wallet 0");
        let w1 = pool.next().expect("next() should return wallet 1");
        let w2 = pool.next().expect("next() should return wallet 2");

        // Verify they are distinct wallets
        assert_ne!(w0.pubkey_b58, w1.pubkey_b58);
        assert_ne!(w1.pubkey_b58, w2.pubkey_b58);
        assert_ne!(w0.pubkey_b58, w2.pubkey_b58);

        // Fourth call wraps around to wallet 0
        let w3 = pool.next().expect("next() should wrap around to wallet 0");
        assert_eq!(w3.pubkey_b58, w0.pubkey_b58);
    }

    // -----------------------------------------------------------------------
    // 6. Failover skips failed wallet
    // -----------------------------------------------------------------------

    #[test]
    fn test_failover_skips_failed_wallet() {
        let keys = test_keypairs(3);
        let pool = FeePayerPool::from_keys(&keys).expect("should load 3 keys");

        // Get wallet 0, then mark it failed
        let w0 = pool.next().expect("wallet 0");
        pool.mark_failed(w0.index);

        // Next calls should skip wallet 0
        let w1 = pool.next().expect("should skip to wallet 1");
        assert_ne!(
            w1.pubkey_b58, w0.pubkey_b58,
            "should have skipped failed wallet 0"
        );

        let w2 = pool.next().expect("should skip to wallet 2");
        assert_ne!(
            w2.pubkey_b58, w0.pubkey_b58,
            "should still skip failed wallet 0"
        );

        // Wraps around but still skips 0
        let w3 = pool.next().expect("should wrap around skipping 0");
        assert_eq!(w3.pubkey_b58, w1.pubkey_b58, "should wrap to wallet 1");
    }

    // -----------------------------------------------------------------------
    // 7. All wallets failed returns error
    // -----------------------------------------------------------------------

    #[test]
    fn test_all_failed_returns_error() {
        let keys = test_keypairs(2);
        let pool = FeePayerPool::from_keys(&keys).expect("should load 2 keys");

        // Mark all wallets as failed
        pool.mark_failed(0);
        pool.mark_failed(1);

        let result = pool.next();
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), FeePayerError::AllFailed),
            "expected FeePayerError::AllFailed when all wallets are in cooldown"
        );
    }

    // -----------------------------------------------------------------------
    // 8. Failed wallet recovers after cooldown
    // -----------------------------------------------------------------------

    #[test]
    fn test_failed_wallet_recovers_after_cooldown() {
        let keys = test_keypairs(1);
        // Use a very short cooldown for test speed
        let pool = FeePayerPool::from_keys_with_cooldown(&keys, Duration::from_millis(50))
            .expect("should load 1 key");

        let w0 = pool.next().expect("wallet 0");
        pool.mark_failed(w0.index);

        // Immediately after marking failed, next() should fail
        let result = pool.next();
        assert!(
            result.is_err(),
            "should fail immediately after marking wallet as failed"
        );

        // Wait for cooldown to expire
        std::thread::sleep(Duration::from_millis(60));

        // Now the wallet should be available again
        let recovered = pool.next().expect("wallet should recover after cooldown");
        assert_eq!(recovered.pubkey_b58, w0.pubkey_b58);
    }

    // -----------------------------------------------------------------------
    // 9. Debug output redacts keypair bytes
    // -----------------------------------------------------------------------

    #[test]
    fn test_debug_redacts_keypair() {
        let keys = test_keypairs(1);
        let pool = FeePayerPool::from_keys(&keys).expect("should load 1 key");
        let wallet = pool.next().expect("should get wallet");

        let debug_output = format!("{:?}", wallet);
        assert!(
            debug_output.contains("[REDACTED]"),
            "wallet debug must contain [REDACTED]"
        );
        assert!(
            debug_output.contains(&wallet.pubkey_b58),
            "wallet debug must show pubkey for identification"
        );

        let pool_debug = format!("{:?}", pool);
        assert!(
            !pool_debug.contains("keypair"),
            "pool debug must not contain raw keypair data"
        );
    }

    // -----------------------------------------------------------------------
    // 10. mark_healthy clears cooldown immediately
    // -----------------------------------------------------------------------

    #[test]
    fn test_mark_healthy_clears_cooldown() {
        let keys = test_keypairs(1);
        let pool = FeePayerPool::from_keys(&keys).expect("should load 1 key");

        let w0 = pool.next().expect("wallet 0");
        pool.mark_failed(w0.index);

        // Wallet is now in cooldown
        let result = pool.next();
        assert!(result.is_err(), "wallet should be in cooldown");

        // Explicitly mark healthy
        pool.mark_healthy(w0.index);

        // Wallet should be immediately available
        let recovered = pool
            .next()
            .expect("wallet should be available after mark_healthy");
        assert_eq!(recovered.pubkey_b58, w0.pubkey_b58);
    }

    // -----------------------------------------------------------------------
    // 11. mark_healthy out of bounds is a no-op
    // -----------------------------------------------------------------------

    #[test]
    fn test_mark_healthy_out_of_bounds_noop() {
        let keys = test_keypairs(1);
        let pool = FeePayerPool::from_keys(&keys).expect("should load 1 key");
        // Should not panic
        pool.mark_healthy(999);
    }

    // -----------------------------------------------------------------------
    // 12. pubkeys returns all wallet pubkeys
    // -----------------------------------------------------------------------

    #[test]
    fn test_pubkeys_returns_all() {
        let keys = test_keypairs(3);
        let pool = FeePayerPool::from_keys(&keys).expect("should load 3 keys");
        let pubkeys = pool.pubkeys();
        assert_eq!(pubkeys.len(), 3);
        // Each pubkey should be distinct
        let unique: std::collections::HashSet<_> = pubkeys.iter().collect();
        assert_eq!(unique.len(), 3);
    }

    // -----------------------------------------------------------------------
    // 13. Fee payer rotation on claim failure: failed wallet is skipped
    // -----------------------------------------------------------------------

    #[test]
    fn test_fee_payer_rotation_on_failure_skips_to_next() {
        let keys = test_keypairs(3);
        let pool = FeePayerPool::from_keys(&keys).expect("should load 3 keys");

        // Get wallet 0
        let w0 = pool.next().expect("wallet 0");

        // Simulate claim failure by marking wallet 0 as failed
        pool.mark_failed(w0.index);

        // Next two calls should return wallets 1 and 2
        let w1 = pool.next().expect("should get wallet 1");
        let w2 = pool.next().expect("should get wallet 2");
        assert_ne!(w1.pubkey_b58, w0.pubkey_b58, "wallet 0 should be skipped");
        assert_ne!(
            w2.pubkey_b58, w0.pubkey_b58,
            "wallet 0 should still be skipped"
        );
        assert_ne!(
            w1.pubkey_b58, w2.pubkey_b58,
            "wallets 1 and 2 should differ"
        );

        // Wrap around still skips wallet 0
        let w3 = pool.next().expect("should wrap to wallet 1 again");
        assert_eq!(
            w3.pubkey_b58, w1.pubkey_b58,
            "should wrap to first available wallet"
        );
    }

    // -----------------------------------------------------------------------
    // 14. All payers unhealthy returns AllFailed error
    // -----------------------------------------------------------------------

    #[test]
    fn test_all_payers_unhealthy_returns_all_failed() {
        let keys = test_keypairs(3);
        let pool = FeePayerPool::from_keys(&keys).expect("should load 3 keys");

        // Mark all as failed
        pool.mark_failed(0);
        pool.mark_failed(1);
        pool.mark_failed(2);

        let result = pool.next();
        assert!(result.is_err());
        assert!(
            matches!(result.unwrap_err(), FeePayerError::AllFailed),
            "should return AllFailed when all payers are unhealthy"
        );
    }
}
