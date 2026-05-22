//! Response caching layer for LLM completions.
//!
//! Tier 1: Exact match — SHA256(model + messages + temperature) → Redis
//! TTL: 10min default, configurable per model.
//! Expected hit rate: 15–30%.
//!
//! # Cache key design
//! Cache keys are keyed on `SHA-256(model ‖ serialised_messages ‖ temperature)` —
//! deliberately **wallet-agnostic**. A response cached for wallet A will be returned
//! to wallet B if the prompt is identical. Both wallets pay the gateway's 402 fee
//! (payment verification runs before the cache check), but the upstream LLM is only
//! charged once. This is an intentional design trade-off: prompt deduplication lowers
//! upstream costs and improves margin. The trade-off is that wallet B receives wallet
//! A's response without the gateway incurring a new upstream cost.
//!
//! If per-wallet response isolation is ever required, the cache key must include the
//! payer wallet address.

use std::num::NonZeroUsize;
use std::sync::Mutex;

use lru::LruCache;

use tracing::{info, warn};

/// Tier 1 — exact-match cache (`cache_key` / `get` / `set`). Split out so the
/// orchestrator here stays focused on shared infra (replay protection, raw KV,
/// connection handling) and the upcoming semantic tier lands as a sibling.
mod exact;

/// Prompt embedding backend for the semantic cache tier.
pub mod embedder;

/// Redis key prefix for response cache entries.
///
/// Centralised here so a rename never requires hunting down inline literals.
/// Replacing the legacy `rcr:` prefix completes the Solvela rebrand in the
/// Redis keyspace; existing `rcr:` keys will expire naturally — no dual-write.
const CACHE_KEY_PREFIX: &str = "solvela:cache:";

/// Redis key prefix for transaction replay-protection entries.
const REPLAY_KEY_PREFIX: &str = "solvela:txn:";

/// Cache configuration.
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// Default TTL in seconds (600 = 10 minutes).
    pub default_ttl_secs: u64,
    /// Whether caching is enabled.
    pub enabled: bool,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            default_ttl_secs: 600,
            enabled: true,
        }
    }
}

/// Response cache backed by Redis.
///
/// Uses exact-match SHA-256 hashing for cache keys.
/// Streaming requests (stream=true) are NOT cached.
///
/// When Redis is unavailable, replay protection degrades to an in-memory
/// LRU cache bounded to 10,000 entries. The LRU cache automatically evicts
/// the oldest entries when full, eliminating the clear-on-overflow gap that
/// a `HashSet` would have.
pub struct ResponseCache {
    client: redis::Client,
    config: CacheConfig,
    /// In-memory replay protection fallback used when Redis is unreachable.
    /// LRU eviction ensures oldest entries are dropped first — no full clears.
    fallback_replay_set: Mutex<LruCache<String, ()>>,
}

impl ResponseCache {
    /// Create a new cache connected to Redis at the given URL.
    pub fn new(redis_url: &str, config: CacheConfig) -> Result<Self, CacheError> {
        let client =
            redis::Client::open(redis_url).map_err(|e| CacheError::Connection(e.to_string()))?;
        Ok(Self {
            client,
            config,
            fallback_replay_set: Mutex::new(LruCache::new(
                NonZeroUsize::new(10_000).expect("nonzero"),
            )),
        })
    }

    /// Create a cache from an already-opened Redis client.
    ///
    /// Use this when the caller has already verified connectivity (e.g. `main.rs`
    /// probes the connection before building the cache so we don't duplicate effort).
    pub fn from_client(client: redis::Client, config: CacheConfig) -> Result<Self, CacheError> {
        Ok(Self {
            client,
            config,
            fallback_replay_set: Mutex::new(LruCache::new(
                NonZeroUsize::new(10_000).expect("nonzero"),
            )),
        })
    }

    /// Ping Redis to check connectivity.
    ///
    /// Returns `true` if Redis responds to PING, `false` on any error.
    pub async fn ping(&self) -> bool {
        let conn = self.client.get_multiplexed_async_connection().await;
        match conn {
            Ok(mut c) => redis::cmd("PING")
                .query_async::<String>(&mut c)
                .await
                .is_ok(),
            Err(_) => false,
        }
    }

    /// Atomically check-and-record a transaction signature to prevent replay attacks.
    ///
    /// Uses Redis SET NX (set-if-not-exists) with a TTL. If the signature has been
    /// seen before, returns `Err(CacheError::Replay)`. On first sight, records it
    /// and returns `Ok(())`.
    ///
    /// ## TTL strategy
    ///
    /// - **Standard transactions** (recent blockhash): 120 seconds — covers the
    ///   blockhash expiry window (~90s) plus settlement latency.
    /// - **Durable nonce transactions**: 86,400 seconds (24 hours) — durable nonce
    ///   transactions never expire on-chain, so a short TTL would allow replay
    ///   after the entry expires. 24 hours provides strong protection for the
    ///   realistic threat window while still allowing Redis key cleanup.
    ///
    /// Set `uses_durable_nonce` to `true` when the transaction contains an
    /// `AdvanceNonceAccount` instruction (detectable from the tx structure).
    ///
    /// **Degraded mode**: if Redis is unavailable, the method falls back to an
    /// in-memory LRU cache (bounded to 10,000 entries).  A warning is emitted
    /// so operators know protection is degraded.  The LRU cache automatically
    /// evicts the oldest entries when full, so there is no clear-on-overflow gap.
    pub async fn check_and_record_tx(
        &self,
        tx_signature: &str,
        uses_durable_nonce: bool,
    ) -> Result<(), CacheError> {
        // Standard blockhash txs expire on-chain in ~90s; 120s TTL is sufficient.
        // Durable nonce txs never expire on-chain; use 24h to prevent replay.
        let ttl_secs: u64 = if uses_durable_nonce {
            86_400 // 24 hours
        } else {
            120
        };

        let key = format!("{}{}", REPLAY_KEY_PREFIX, tx_signature);

        match self.client.get_multiplexed_async_connection().await {
            Ok(mut conn) => {
                // SET key 1 NX EX <ttl> — atomic: only sets if key does NOT exist
                let result: Result<Option<String>, redis::RedisError> = redis::cmd("SET")
                    .arg(&key)
                    .arg("1")
                    .arg("NX")
                    .arg("EX")
                    .arg(ttl_secs)
                    .query_async(&mut conn)
                    .await;

                match result {
                    Ok(Some(_)) => {
                        // Key was newly set — first time seeing this tx
                        if uses_durable_nonce {
                            info!(
                                tx = %tx_signature,
                                ttl_secs = ttl_secs,
                                "recorded durable nonce transaction with extended replay TTL"
                            );
                        }
                        Ok(())
                    }
                    Ok(None) => {
                        // Key already existed — replay detected
                        Err(CacheError::Replay)
                    }
                    Err(e) => {
                        // Redis command error (timeout, OOM, etc.) — do NOT treat as replay.
                        // Fall through to in-memory LRU fallback below.
                        warn!(
                            error = %e,
                            tx = %tx_signature,
                            "Redis SET NX failed — falling back to in-memory replay check"
                        );

                        let mut cache = self
                            .fallback_replay_set
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);

                        if cache.get(tx_signature).is_some() {
                            Err(CacheError::Replay)
                        } else {
                            cache.put(tx_signature.to_string(), ());
                            warn!(
                                tx = %tx_signature,
                                "payment accepted under degraded in-memory replay protection (Redis SET NX error)"
                            );
                            Ok(())
                        }
                    }
                }
            }
            Err(e) => {
                warn!(
                    error = %e,
                    tx = %tx_signature,
                    "Redis unavailable for replay check — falling back to in-memory replay protection (degraded)"
                );

                // LRU cache automatically evicts oldest entries when full —
                // no clearing needed, no replay window gaps.
                let mut cache = self
                    .fallback_replay_set
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);

                if cache.get(tx_signature).is_some() {
                    // Already seen — replay detected
                    Err(CacheError::Replay)
                } else {
                    cache.put(tx_signature.to_string(), ());
                    warn!(
                        tx = %tx_signature,
                        "payment accepted under degraded in-memory replay protection"
                    );
                    Ok(())
                }
            }
        }
    }

    /// Get a raw string value by key.
    pub async fn get_raw(&self, key: &str) -> Result<Option<String>, CacheError> {
        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| CacheError::Operation(e.to_string()))?;
        redis::cmd("GET")
            .arg(key)
            .query_async(&mut conn)
            .await
            .map_err(|e| CacheError::Operation(e.to_string()))
    }

    /// Set a raw string value with TTL.
    pub async fn set_raw(
        &self,
        key: &str,
        value: &str,
        ttl: std::time::Duration,
    ) -> Result<(), CacheError> {
        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| CacheError::Operation(e.to_string()))?;
        redis::cmd("SETEX")
            .arg(key)
            .arg(ttl.as_secs())
            .arg(value)
            .query_async(&mut conn)
            .await
            .map_err(|e| CacheError::Operation(e.to_string()))
    }

    /// Delete a key.
    pub async fn del_raw(&self, key: &str) -> Result<(), CacheError> {
        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| CacheError::Operation(e.to_string()))?;
        redis::cmd("DEL")
            .arg(key)
            .query_async::<i64>(&mut conn)
            .await
            .map_err(|e| CacheError::Operation(e.to_string()))?;
        Ok(())
    }
}

/// Cache error types.
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("cache connection failed: {0}")]
    Connection(String),

    #[error("cache operation failed: {0}")]
    Operation(String),

    #[error("transaction replay detected")]
    Replay,

    #[error("cache unavailable")]
    Unavailable,
}

#[cfg(test)]
mod tests {
    //! Tests here cover the shared infra owned by `ResponseCache`: config
    //! defaults, client construction, error display, and the in-memory replay
    //! fallback. Exact-match cache-key / get / set behaviour is tested in
    //! `exact.rs` alongside the code it exercises.
    use super::*;

    #[test]
    fn test_cache_config_default() {
        let config = CacheConfig::default();
        assert_eq!(config.default_ttl_secs, 600);
        assert!(config.enabled);
    }

    /// `from_client` accepts an already-opened Redis client without making any
    /// network round-trip. Used by `main.rs` after a connectivity probe.
    #[test]
    fn from_client_constructs_without_connecting() {
        let client =
            redis::Client::open("redis://127.0.0.1:1").expect("client open should not connect");
        let _cache = ResponseCache::from_client(client, CacheConfig::default())
            .expect("from_client should not connect");
    }

    #[test]
    fn test_cache_error_display() {
        let err = CacheError::Connection("refused".to_string());
        assert_eq!(err.to_string(), "cache connection failed: refused");

        let err = CacheError::Operation("timeout".to_string());
        assert_eq!(err.to_string(), "cache operation failed: timeout");

        let err = CacheError::Replay;
        assert_eq!(err.to_string(), "transaction replay detected");

        let err = CacheError::Unavailable;
        assert_eq!(err.to_string(), "cache unavailable");
    }

    /// Test the in-memory fallback LRU cache directly (no Redis connection needed).
    ///
    /// This exercises the same logic that `check_and_record_tx` delegates to when
    /// Redis is unavailable, without incurring a network timeout.
    #[test]
    fn test_fallback_replay_set_first_insert_succeeds() {
        let cache = ResponseCache::new("redis://127.0.0.1:1", CacheConfig::default())
            .expect("client creation should not connect");

        let sig = "test_tx_signature_abc123";
        let mut lru = cache.fallback_replay_set.lock().unwrap();

        // First lookup — signature is new, get returns None
        assert!(
            lru.get(sig).is_none(),
            "first lookup of a new signature should return None"
        );
        lru.put(sig.to_string(), ());

        // Second lookup — signature already present, get returns Some
        assert!(
            lru.get(sig).is_some(),
            "duplicate lookup should return Some (replay detected)"
        );

        // Different signature — should not be found
        assert!(
            lru.get("different_sig_xyz789").is_none(),
            "a new distinct signature should not be found"
        );
    }

    /// When the fallback LRU cache reaches its capacity limit, the oldest entry
    /// is evicted (not the entire set), so recent entries remain protected.
    #[test]
    fn test_fallback_replay_set_lru_eviction() {
        let cache = ResponseCache::new("redis://127.0.0.1:1", CacheConfig::default())
            .expect("client creation should not connect");

        let mut lru = cache.fallback_replay_set.lock().unwrap();
        let cap = lru.cap().get();

        // Fill the LRU cache to its exact capacity.
        for i in 0..cap {
            lru.put(format!("sig_{i}"), ());
        }
        assert_eq!(lru.len(), cap);

        // Insert one more — should evict the oldest (sig_0).
        lru.put("new_sig".to_string(), ());
        assert_eq!(lru.len(), cap, "LRU cache should stay at capacity");

        // The oldest entry (sig_0) should have been evicted.
        assert!(
            lru.get("sig_0").is_none(),
            "oldest entry should be evicted by LRU"
        );

        // The newest entry should still be present.
        assert!(
            lru.get("new_sig").is_some(),
            "newest entry should remain in LRU cache"
        );

        // A recent entry (sig_9999) should still be present.
        assert!(
            lru.get(&format!("sig_{}", cap - 1)).is_some(),
            "recent entries should remain in LRU cache"
        );
    }
}
