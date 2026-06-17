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

/// Tier 2 — semantic (embedding-similarity) cache over RediSearch.
pub mod semantic;

/// Redis key prefix for response cache entries.
///
/// Centralised here so a rename never requires hunting down inline literals.
/// Replacing the legacy `rcr:` prefix completes the Solvela rebrand in the
/// Redis keyspace; existing `rcr:` keys will expire naturally — no dual-write.
const CACHE_KEY_PREFIX: &str = "solvela:cache:";

/// Redis key prefix for transaction replay-protection entries.
const REPLAY_KEY_PREFIX: &str = "solvela:txn:";

/// Redis key prefix for the A2A per-task settlement lock (issue #566).
///
/// One key per `taskId`; held across `verify_and_settle` so only a single
/// concurrent `message/send` settles a given task. See
/// [`ResponseCache::acquire_settle_lock`].
const A2A_SETTLE_LOCK_PREFIX: &str = "solvela:a2a:settle_lock:";

/// TTL (seconds) for the A2A settlement lock (issue #566).
///
/// A single settlement is seconds (on-chain confirm + provider call). 120s
/// comfortably outlives the worst-case attempt and matches the standard-tx
/// replay-key window (so a crashed-mid-settlement task's lock and its replay
/// key expire together). It is well under the 600s A2A task TTL, so a task
/// whose holder crashed AFTER acquiring but BEFORE settling re-opens for one
/// legitimate retry inside the task's own lifetime rather than being stranded.
pub const A2A_SETTLE_LOCK_TTL_SECS: u64 = 120;

/// Redis key prefix for the cross-instance aggregate free-tier RPM counter.
/// The full key is `free_tier:global_rpm:<epoch_minute>` (see
/// [`ResponseCache::incr_global_free_window`]).
const FREE_TIER_GLOBAL_RPM_PREFIX: &str = "free_tier:global_rpm:";

/// TTL (seconds) applied to a free-tier RPM window key on its first increment.
/// Longer than the 60s window so the key outlives its own minute; old windows
/// self-expire rather than accumulating.
const FREE_TIER_GLOBAL_WINDOW_TTL_SECS: u64 = 120;

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

    /// Atomically acquire the A2A per-task settlement lock (issue #566).
    ///
    /// Uses Redis `SET key 1 NX EX <ttl>` — the same atomic, cross-instance
    /// compare-and-swap idiom as [`Self::check_and_record_tx`]. Returns:
    /// - `Ok(true)`  — lock newly acquired (this caller is the sole settler),
    /// - `Ok(false)` — lock already held by a concurrent settlement (caller
    ///   MUST reject without settling),
    /// - `Err(_)`    — Redis was unreachable or the command failed.
    ///
    /// On `Err` the caller MUST treat the request as un-settleable (fail
    /// closed) rather than proceed: the A2A task store already requires Redis,
    /// so a lock-store error means we cannot guarantee exactly-once settlement.
    /// Unlike the replay check, there is deliberately **no in-memory fallback**
    /// — an in-memory lock cannot serialise across gateway instances, which is
    /// exactly the multi-instance race this lock exists to prevent.
    ///
    /// The `ttl` bounds a holder that crashes mid-settlement (see
    /// [`A2A_SETTLE_LOCK_TTL_SECS`]).
    pub async fn acquire_settle_lock(
        &self,
        task_id: &str,
        ttl_secs: u64,
    ) -> Result<bool, CacheError> {
        let key = format!("{A2A_SETTLE_LOCK_PREFIX}{task_id}");
        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| CacheError::Operation(e.to_string()))?;

        // SET key 1 NX EX <ttl> — atomic: only sets (and returns Some) if the
        // key does NOT already exist. `None` means a concurrent settler holds it.
        let result: Option<String> = redis::cmd("SET")
            .arg(&key)
            .arg("1")
            .arg("NX")
            .arg("EX")
            .arg(ttl_secs)
            .query_async(&mut conn)
            .await
            .map_err(|e| CacheError::Operation(e.to_string()))?;

        Ok(result.is_some())
    }

    /// Release the A2A per-task settlement lock (issue #566).
    ///
    /// Best-effort `DEL`. Called only on a settlement FAILURE so the agent can
    /// retry with a corrected payment without waiting out the TTL. On SUCCESS
    /// the lock is intentionally NOT released: the task is now `Completed` and
    /// the lock must keep blocking any in-flight concurrent attempt from
    /// re-settling already-moved funds until it self-expires. A failed `DEL`
    /// degrades to TTL-based release (the task is still payable after the TTL,
    /// within the task's own lifetime), so the error is logged, not propagated.
    pub async fn release_settle_lock(&self, task_id: &str) {
        let key = format!("{A2A_SETTLE_LOCK_PREFIX}{task_id}");
        if let Err(e) = self.del_raw(&key).await {
            // F3 (#566): make a lost release observable. The lock still self-
            // expires via TTL (so the task re-opens for retry within its own
            // lifetime), but a non-zero rate here means legitimate retries are
            // waiting out the full TTL after a failed payment — alertable.
            metrics::counter!("solvela_a2a_settle_lock_release_failed_total").increment(1);
            warn!(
                task_id,
                error = %e,
                "A2A settlement lock release failed — lock will expire via TTL"
            );
        }
    }

    /// Increment the global free-tier fixed-window counter for `epoch_minute`
    /// and return the post-increment count.
    ///
    /// Backs the cross-instance aggregate free-tier rate cap (PR B). Multiple
    /// gateway instances share one upstream provider API key whose free-tier
    /// quota is a SINGLE shared ceiling (Google's free Gemini tier: ~15 RPM
    /// across the whole key). A per-instance in-memory counter × N instances
    /// would collectively blow past that ceiling, so the authoritative counter
    /// MUST live in Redis when Redis is configured.
    ///
    /// Fixed-window counter: `INCR free_tier:global_rpm:<epoch_minute>`, with a
    /// short TTL set on the FIRST increment so stale window keys self-expire.
    /// The TTL (120s) comfortably outlives the 60s window — the window key is
    /// only read during its own minute, and the extra slack absorbs clock skew
    /// without ever letting two live windows share a key.
    ///
    /// Returns `Err(CacheError::Operation)` on any Redis error. The caller
    /// (`FreeTierGlobalCap`) DEGRADES to its in-memory counter on `Err` rather
    /// than failing the request — an infra blip must not hard-fail free traffic,
    /// and the upstream provider's own 429 remains the ultimate backstop.
    pub async fn incr_global_free_window(&self, epoch_minute: u64) -> Result<u64, CacheError> {
        let key = format!("{FREE_TIER_GLOBAL_RPM_PREFIX}{epoch_minute}");
        let mut conn = self
            .client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| CacheError::Operation(e.to_string()))?;

        // INCR is atomic and returns the new value. Redis creates the key at 0
        // then increments, so a return of exactly 1 means we just created the
        // window key — set its TTL only then (avoids resetting the TTL on every
        // increment, which would keep a hot key alive indefinitely).
        let count: u64 = redis::cmd("INCR")
            .arg(&key)
            .query_async(&mut conn)
            .await
            .map_err(|e| CacheError::Operation(e.to_string()))?;

        if count == 1 {
            // Best-effort TTL on first sight. If EXPIRE fails the counter still
            // bounds the current minute correctly; the key would just linger,
            // and Redis key eviction / a later window's own EXPIRE bounds that.
            let _: Result<i64, redis::RedisError> = redis::cmd("EXPIRE")
                .arg(&key)
                .arg(FREE_TIER_GLOBAL_WINDOW_TTL_SECS)
                .query_async(&mut conn)
                .await;
        }

        Ok(count)
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

/// Test-only metrics helpers shared by the exact and semantic tier test
/// modules.
///
/// The `metrics` crate enforces a single process-wide recorder: the first
/// `install_recorder()` call wins, and every subsequent install fails with
/// `FailedToSetGlobalRecorder`. The exact-tier and semantic-tier `#[cfg(test)]`
/// modules both need to read counter values, so they MUST share one handle —
/// otherwise the second module's install panics, and a per-module `OnceLock`
/// holding the install result cannot be reached from the other module.
///
/// Both tier modules import `install_test_recorder` and `counter_value` from
/// here; the static `OnceLock` here is the single source of truth for the
/// handle, so order-of-invocation between modules no longer matters.
#[cfg(test)]
pub(super) mod test_metrics {
    pub(crate) fn install_test_recorder() -> metrics_exporter_prometheus::PrometheusHandle {
        use std::sync::OnceLock;
        static HANDLE: OnceLock<metrics_exporter_prometheus::PrometheusHandle> = OnceLock::new();
        HANDLE
            .get_or_init(|| {
                metrics_exporter_prometheus::PrometheusBuilder::new()
                    .install_recorder()
                    .expect(
                        "first install_test_recorder caller must succeed; \
                         later callers reuse this handle via OnceLock",
                    )
            })
            .clone()
    }

    /// Parse a single counter's current value from the Prometheus text
    /// rendering. Returns 0 if the counter has never been incremented (no
    /// exposition line emitted yet).
    pub(crate) fn counter_value(
        handle: &metrics_exporter_prometheus::PrometheusHandle,
        name: &str,
    ) -> u64 {
        let body = handle.render();
        // Counter exposition lines look like `name 5` or `name{label="x"} 5`.
        // Sum every line starting with the metric name to handle both
        // unlabeled and labeled counter families.
        body.lines()
            .filter(|l| l.starts_with(&format!("{name} ")) || l.starts_with(&format!("{name}{{")))
            .filter_map(|l| l.rsplit_once(' ').and_then(|(_, v)| v.parse::<u64>().ok()))
            .sum()
    }

    /// Like [`counter_value`], but sums only the label-series of `name` whose
    /// exposition line's label block CONTAINS `label_substr`.
    ///
    /// The single process-wide recorder means `counter_value` (which sums the
    /// WHOLE family across labels) is contaminated when sibling tests run
    /// concurrently and increment the same metric family under different
    /// labels. Tests that emit labeled counters give themselves a UNIQUE label
    /// value (e.g. a unique `model="..."`) and read with this filter so their
    /// before/after delta is attributable only to themselves — independent of
    /// whatever other tests do to the same family in parallel.
    pub(crate) fn counter_value_filtered(
        handle: &metrics_exporter_prometheus::PrometheusHandle,
        name: &str,
        label_substr: &str,
    ) -> u64 {
        let body = handle.render();
        body.lines()
            .filter(|l| l.starts_with(&format!("{name}{{")))
            .filter(|l| l.contains(label_substr))
            .filter_map(|l| l.rsplit_once(' ').and_then(|(_, v)| v.parse::<u64>().ok()))
            .sum()
    }
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

    /// The Redis-backed aggregate free-tier window counter increments
    /// monotonically within one window and sets a TTL on first sight.
    ///
    /// Gated on a reachable Redis (skips cleanly if down). Uses a unique
    /// per-run epoch_minute key so concurrent test runs don't collide. Cleans
    /// the key up afterward.
    #[tokio::test]
    async fn incr_global_free_window_counts_and_sets_ttl() {
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379/".to_string());
        let client = match redis::Client::open(redis_url.clone()) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("skipping: cannot open Redis client ({e})");
                return;
            }
        };
        let mut conn = match client.get_multiplexed_async_connection().await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("skipping: Redis unreachable ({e})");
                return;
            }
        };

        let cache = ResponseCache::new(&redis_url, CacheConfig::default())
            .expect("client creation should not connect");

        // Unique window id so this test is isolated from real traffic and other
        // runs. Derived from nanos to avoid collisions.
        let minute = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        let key = format!("{FREE_TIER_GLOBAL_RPM_PREFIX}{minute}");

        // First increment → 1, and a TTL must now be set on the key.
        let c1 = cache
            .incr_global_free_window(minute)
            .await
            .expect("redis incr should succeed");
        assert_eq!(c1, 1, "first increment of a fresh window returns 1");

        let ttl: i64 = redis::cmd("TTL")
            .arg(&key)
            .query_async(&mut conn)
            .await
            .expect("TTL query");
        assert!(
            ttl > 0 && ttl as u64 <= FREE_TIER_GLOBAL_WINDOW_TTL_SECS,
            "first increment must set a positive TTL (<= {FREE_TIER_GLOBAL_WINDOW_TTL_SECS}s), got {ttl}"
        );

        // Subsequent increments keep counting up in the same window.
        let c2 = cache.incr_global_free_window(minute).await.unwrap();
        let c3 = cache.incr_global_free_window(minute).await.unwrap();
        assert_eq!(c2, 2);
        assert_eq!(c3, 3);

        // Cleanup.
        let _: Result<i64, redis::RedisError> =
            redis::cmd("DEL").arg(&key).query_async(&mut conn).await;
    }
}
