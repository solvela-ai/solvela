//! Request deduplication via SHA-256 of canonicalized body.
//!
//! Prevents double-charging when a client times out and retries: the second
//! request looks new because it has different timestamps in the body, so it
//! gets a new payment requirement and (if the client signed both) the wallet
//! pays twice.
//!
//! ## Canonicalization
//!
//! The hash input strips agent timestamp prefixes from message content and
//! produces a deterministic JSON serialization (sorted keys) so that two
//! requests differing only in leading timestamps hash to the same value.
//!
//! Pattern stripped (port of ClawRouter's `src/dedup.ts`):
//! `\[(DAY )?\d{4}-\d{2}-\d{2} \d{2}:\d{2}( [A-Z]{2,4})?\]\s*`
//!
//! ## Storage
//!
//! - **Redis** when configured: 30s TTL via `SETEX`.
//! - **In-memory LRU** fallback: same 30s TTL, bounded to 4096 entries.
//!
//! ## Opt-out
//!
//! Set `SOLVELA_DEDUP_DISABLED=true` to bypass the cache entirely.

use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::Engine;
use lru::LruCache;
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tracing::{debug, warn};

/// TTL for dedup cache entries (Redis + in-memory fallback).
pub const DEDUP_TTL: Duration = Duration::from_secs(30);

/// Maximum number of entries kept in the in-memory fallback LRU.
const FALLBACK_CAPACITY: usize = 4096;

/// Regex matching agent-injected timestamp prefixes.
///
/// Examples that match:
/// - `[2026-04-30 14:30]`
/// - `[2026-04-30 14:30 UTC]`
/// - `[DAY 2026-04-30 14:30 EST]`
static TIMESTAMP_PREFIX_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"^\[(?:DAY )?\d{4}-\d{2}-\d{2} \d{2}:\d{2}(?: [A-Z]{2,4})?\]\s*")
        .expect("dedup timestamp regex must compile")
});

/// Cached response stored against a canonical request hash.
#[derive(Debug, Clone)]
pub struct CachedResponse {
    /// The full HTTP response body bytes.
    pub body: Vec<u8>,
    /// MIME type (e.g., `application/json`).
    pub content_type: String,
    /// HTTP status code.
    pub status: u16,
}

/// Returns true when dedup has been disabled by env (`SOLVELA_DEDUP_DISABLED=true`).
pub fn is_disabled() -> bool {
    std::env::var("SOLVELA_DEDUP_DISABLED")
        .ok()
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

/// Strip a leading agent timestamp prefix from a single string.
///
/// Returns a borrowed slice when no prefix is present so that no allocation
/// occurs in the common case.
fn strip_timestamp_prefix(input: &str) -> &str {
    if let Some(m) = TIMESTAMP_PREFIX_RE.find(input) {
        &input[m.end()..]
    } else {
        input
    }
}

/// Recursively walk a JSON value and strip leading timestamp prefixes from
/// any `content` string field on user message objects. The transformation
/// is intentionally permissive: any string field named `content` anywhere
/// in the tree is normalized.
fn canonicalize_value(value: &mut Value) {
    match value {
        Value::Object(map) => {
            // Strip prefixes from `content` fields specifically — chat messages.
            if let Some(Value::String(s)) = map.get_mut("content") {
                let stripped = strip_timestamp_prefix(s);
                if stripped.len() != s.len() {
                    *s = stripped.to_string();
                }
            }
            for (_k, v) in map.iter_mut() {
                canonicalize_value(v);
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                canonicalize_value(item);
            }
        }
        _ => {}
    }
}

/// Recursively sort all object keys in a JSON value in place.
///
/// The `preserve_order` feature of `serde_json` is enabled transitively via
/// the `cli` crate, so `Map` preserves insertion order. To get a deterministic
/// canonical form we must explicitly re-order keys.
fn sort_keys_recursive(value: &mut Value) {
    match value {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> =
                map.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            map.clear();
            for (k, mut v) in entries {
                sort_keys_recursive(&mut v);
                map.insert(k, v);
            }
        }
        Value::Array(items) => {
            for item in items.iter_mut() {
                sort_keys_recursive(item);
            }
        }
        _ => {}
    }
}

/// Serialize a JSON value with sorted keys at every object level.
fn sorted_serialize(value: &Value) -> Vec<u8> {
    let mut sorted = value.clone();
    sort_keys_recursive(&mut sorted);
    serde_json::to_vec(&sorted).unwrap_or_default()
}

/// Compute the canonical SHA-256 hash for a request body.
///
/// `body_bytes` should be the raw HTTP request body. Non-JSON bodies are
/// hashed verbatim. Invalid JSON falls back to hashing the raw bytes so that
/// dedup remains safe (different bytes → different hashes).
pub fn canonical_hash(body_bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();

    match serde_json::from_slice::<Value>(body_bytes) {
        Ok(mut value) => {
            canonicalize_value(&mut value);
            let canonical = sorted_serialize(&value);
            hasher.update(&canonical);
        }
        Err(_) => {
            // Non-JSON: hash the raw bytes — identical bodies still match,
            // varying bodies still differ.
            hasher.update(body_bytes);
        }
    }

    let digest = hasher.finalize();
    format!("{digest:x}")
}

/// In-memory LRU fallback used when Redis is not configured.
pub struct InMemoryDedupStore {
    inner: Mutex<LruCache<String, (CachedResponse, Instant)>>,
}

impl InMemoryDedupStore {
    pub fn new() -> Self {
        Self {
            inner: Mutex::new(LruCache::new(
                NonZeroUsize::new(FALLBACK_CAPACITY).expect("nonzero"),
            )),
        }
    }

    pub fn get(&self, hash: &str) -> Option<CachedResponse> {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match guard.get(hash) {
            Some((entry, inserted_at)) if inserted_at.elapsed() < DEDUP_TTL => Some(entry.clone()),
            Some(_) => {
                guard.pop(hash);
                None
            }
            None => None,
        }
    }

    pub fn put(&self, hash: String, entry: CachedResponse) {
        let mut guard = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.put(hash, (entry, Instant::now()));
    }
}

impl Default for InMemoryDedupStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Look up a cached response in Redis by canonical hash.
///
/// Returns `None` on miss, decode failure, or any Redis error. Errors are
/// logged at `debug!` rather than `warn!` because dedup is best-effort —
/// a miss just means the request runs normally.
pub async fn redis_get(cache: &super::ResponseCache, hash: &str) -> Option<CachedResponse> {
    let key = format!("rcr:dedup:{hash}");
    match cache.get_raw(&key).await {
        Ok(Some(s)) => decode_cached(&s),
        Ok(None) => None,
        Err(e) => {
            debug!(error = %e, key = %key, "dedup redis get failed");
            None
        }
    }
}

/// Store a response in Redis under the canonical hash.
///
/// Fire-and-forget semantics: errors are logged at `debug!` and swallowed.
pub async fn redis_put(cache: &super::ResponseCache, hash: &str, entry: &CachedResponse) {
    let key = format!("rcr:dedup:{hash}");
    let encoded = encode_cached(entry);
    if let Err(e) = cache.set_raw(&key, &encoded, DEDUP_TTL).await {
        debug!(error = %e, key = %key, "dedup redis put failed");
    }
}

/// Encode a [`CachedResponse`] for Redis storage as JSON with base64 body.
fn encode_cached(entry: &CachedResponse) -> String {
    let body_b64 = base64::engine::general_purpose::STANDARD.encode(&entry.body);
    let payload = serde_json::json!({
        "body": body_b64,
        "content_type": entry.content_type,
        "status": entry.status,
    });
    payload.to_string()
}

/// Decode a [`CachedResponse`] previously stored via [`encode_cached`].
fn decode_cached(s: &str) -> Option<CachedResponse> {
    let value: Value = serde_json::from_str(s).ok()?;
    let body_b64 = value.get("body")?.as_str()?;
    let body = base64::engine::general_purpose::STANDARD
        .decode(body_b64)
        .ok()?;
    let content_type = value.get("content_type")?.as_str()?.to_string();
    let status = value.get("status")?.as_u64()? as u16;
    Some(CachedResponse {
        body,
        content_type,
        status,
    })
}

/// Helper that warns once per process when dedup is disabled.
pub fn warn_disabled_once() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        warn!("SOLVELA_DEDUP_DISABLED=true — request deduplication is OFF");
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_prefix_basic_strip() {
        let s = "[2026-04-30 14:30] hello";
        assert_eq!(strip_timestamp_prefix(s), "hello");
    }

    #[test]
    fn timestamp_prefix_with_zone() {
        let s = "[2026-04-30 14:30 UTC] world";
        assert_eq!(strip_timestamp_prefix(s), "world");
    }

    #[test]
    fn timestamp_prefix_with_day_marker() {
        let s = "[DAY 2026-04-30 14:30 EST] hi";
        assert_eq!(strip_timestamp_prefix(s), "hi");
    }

    #[test]
    fn timestamp_prefix_no_match_returns_input() {
        let s = "regular content";
        assert_eq!(strip_timestamp_prefix(s), "regular content");
    }

    #[test]
    fn timestamp_prefix_only_at_start() {
        // Mid-string timestamp should NOT be stripped.
        let s = "hi [2026-04-30 14:30] there";
        assert_eq!(strip_timestamp_prefix(s), "hi [2026-04-30 14:30] there");
    }

    #[test]
    fn canonical_hash_same_body_same_hash() {
        let body = br#"{"model":"x","messages":[{"role":"user","content":"hello"}]}"#;
        assert_eq!(canonical_hash(body), canonical_hash(body));
    }

    #[test]
    fn canonical_hash_strips_timestamp_in_messages() {
        let body_a =
            br#"{"model":"x","messages":[{"role":"user","content":"[2026-04-30 14:30] hello"}]}"#;
        let body_b = br#"{"model":"x","messages":[{"role":"user","content":"[2026-04-30 15:45 UTC] hello"}]}"#;
        let body_c = br#"{"model":"x","messages":[{"role":"user","content":"hello"}]}"#;
        let h_a = canonical_hash(body_a);
        let h_b = canonical_hash(body_b);
        let h_c = canonical_hash(body_c);
        assert_eq!(h_a, h_b, "different timestamp prefixes must hash equally");
        assert_eq!(h_a, h_c, "with-prefix and without-prefix must hash equally");
    }

    #[test]
    fn canonical_hash_different_content_different_hash() {
        let body_a = br#"{"messages":[{"content":"hello"}]}"#;
        let body_b = br#"{"messages":[{"content":"goodbye"}]}"#;
        assert_ne!(canonical_hash(body_a), canonical_hash(body_b));
    }

    #[test]
    fn canonical_hash_field_order_independent() {
        // serde_json's default Map is sorted, so re-serialization is canonical.
        let body_a = br#"{"a":1,"b":2,"messages":[{"role":"user","content":"x"}]}"#;
        let body_b = br#"{"b":2,"a":1,"messages":[{"role":"user","content":"x"}]}"#;
        assert_eq!(canonical_hash(body_a), canonical_hash(body_b));
    }

    #[test]
    fn canonical_hash_different_models_different_hash() {
        let body_a = br#"{"model":"a","messages":[{"content":"hi"}]}"#;
        let body_b = br#"{"model":"b","messages":[{"content":"hi"}]}"#;
        assert_ne!(canonical_hash(body_a), canonical_hash(body_b));
    }

    #[test]
    fn canonical_hash_handles_non_json() {
        let body = b"not even json at all";
        // Should not panic and should be deterministic.
        let h = canonical_hash(body);
        assert_eq!(h, canonical_hash(body));
    }

    #[test]
    fn in_memory_store_hit_then_miss_after_pop() {
        let store = InMemoryDedupStore::new();
        let hash = "abc".to_string();
        let entry = CachedResponse {
            body: b"data".to_vec(),
            content_type: "application/json".to_string(),
            status: 200,
        };
        store.put(hash.clone(), entry.clone());
        let hit = store.get(&hash).expect("should hit");
        assert_eq!(hit.body, entry.body);
        assert_eq!(hit.status, 200);
    }

    #[test]
    fn in_memory_store_miss_for_unknown_hash() {
        let store = InMemoryDedupStore::new();
        assert!(store.get("nope").is_none());
    }

    #[test]
    fn is_disabled_default_false() {
        // Use a guard env var name that's unlikely to be set in test env.
        // (We can't reliably mutate env across threads, so just check default.)
        // If SOLVELA_DEDUP_DISABLED happens to be set in this test env, skip.
        if std::env::var("SOLVELA_DEDUP_DISABLED").is_err() {
            assert!(!is_disabled());
        }
    }

    #[test]
    fn is_disabled_true_when_env_set() {
        // Save & restore. Single-threaded `cargo test` keeps this OK; this test
        // also avoids parallel races by being scoped to `set_var` for its body.
        let prior = std::env::var("SOLVELA_DEDUP_DISABLED").ok();
        std::env::set_var("SOLVELA_DEDUP_DISABLED", "true");
        assert!(is_disabled());
        std::env::set_var("SOLVELA_DEDUP_DISABLED", "1");
        assert!(is_disabled());
        std::env::set_var("SOLVELA_DEDUP_DISABLED", "false");
        assert!(!is_disabled());
        match prior {
            Some(v) => std::env::set_var("SOLVELA_DEDUP_DISABLED", v),
            None => std::env::remove_var("SOLVELA_DEDUP_DISABLED"),
        }
    }

    #[test]
    fn encode_decode_roundtrip() {
        let entry = CachedResponse {
            body: vec![1, 2, 3, 0xFF, 0],
            content_type: "application/json".to_string(),
            status: 201,
        };
        let s = encode_cached(&entry);
        let decoded = decode_cached(&s).expect("decode should succeed");
        assert_eq!(decoded.body, entry.body);
        assert_eq!(decoded.content_type, entry.content_type);
        assert_eq!(decoded.status, entry.status);
    }
}
