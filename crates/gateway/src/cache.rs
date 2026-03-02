//! Response caching layer for LLM completions.
//!
//! Tier 1: Exact match — SHA256(model + messages + temperature) → Redis
//! TTL: 10min default, configurable per model.
//! Expected hit rate: 15–30%.

use sha2::{Digest, Sha256};
use tracing::{info, warn};

use rcr_common::types::{ChatRequest, ChatResponse};

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
pub struct ResponseCache {
    client: redis::Client,
    config: CacheConfig,
}

impl ResponseCache {
    /// Create a new cache connected to Redis at the given URL.
    pub fn new(redis_url: &str, config: CacheConfig) -> Result<Self, CacheError> {
        let client =
            redis::Client::open(redis_url).map_err(|e| CacheError::Connection(e.to_string()))?;
        Ok(Self { client, config })
    }

    /// Create a cache from an already-opened Redis client.
    ///
    /// Use this when the caller has already verified connectivity (e.g. `main.rs`
    /// probes the connection before building the cache so we don't duplicate effort).
    pub fn from_client(client: redis::Client, config: CacheConfig) -> Result<Self, CacheError> {
        Ok(Self { client, config })
    }

    /// Generate a cache key from a request.
    /// Key = SHA256(model + sorted_messages_json + temperature)
    pub fn cache_key(req: &ChatRequest) -> String {
        let mut hasher = Sha256::new();
        hasher.update(req.model.as_bytes());
        // Serialize messages deterministically
        if let Ok(msgs_json) = serde_json::to_string(&req.messages) {
            hasher.update(msgs_json.as_bytes());
        }
        if let Some(temp) = req.temperature {
            hasher.update(temp.to_le_bytes());
        }
        let hash = hasher.finalize();
        format!("rcr:cache:{:x}", hash)
    }

    /// Try to get a cached response.
    pub async fn get(&self, req: &ChatRequest) -> Option<ChatResponse> {
        if !self.config.enabled || req.stream {
            return None;
        }
        let key = Self::cache_key(req);

        let mut conn = self.client.get_multiplexed_async_connection().await.ok()?;
        let cached: Option<String> = redis::cmd("GET")
            .arg(&key)
            .query_async(&mut conn)
            .await
            .ok()?;

        match cached {
            Some(json_str) => match serde_json::from_str::<ChatResponse>(&json_str) {
                Ok(response) => {
                    info!(key = %key, "cache hit");
                    Some(response)
                }
                Err(e) => {
                    warn!(error = %e, key = %key, "failed to deserialize cached response");
                    None
                }
            },
            None => None,
        }
    }

    /// Store a response in the cache.
    pub async fn set(&self, req: &ChatRequest, response: &ChatResponse) {
        if !self.config.enabled || req.stream {
            return;
        }
        let key = Self::cache_key(req);

        let json_str = match serde_json::to_string(response) {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "failed to serialize response for cache");
                return;
            }
        };

        // Spawn async — never block the request path
        let client = self.client.clone();
        let ttl = self.config.default_ttl_secs;
        tokio::spawn(async move {
            match client.get_multiplexed_async_connection().await {
                Ok(mut conn) => {
                    let result: Result<(), redis::RedisError> = redis::cmd("SETEX")
                        .arg(&key)
                        .arg(ttl)
                        .arg(&json_str)
                        .query_async(&mut conn)
                        .await;

                    if let Err(e) = result {
                        warn!(error = %e, key = %key, "failed to write to cache");
                    } else {
                        info!(key = %key, ttl_secs = ttl, "cached response");
                    }
                }
                Err(e) => {
                    warn!(error = %e, "failed to connect to Redis for caching");
                }
            }
        });
    }

    /// Returns the cache configuration.
    pub fn config(&self) -> &CacheConfig {
        &self.config
    }

    /// Atomically check-and-record a transaction signature to prevent replay attacks.
    ///
    /// Uses Redis SET NX (set-if-not-exists) with a TTL longer than the Solana
    /// blockhash expiry window (~90 seconds). If the signature has been seen before,
    /// returns `Err(())`. On first sight, records it and returns `Ok(())`.
    ///
    /// TTL is set to 120 seconds — enough to cover the blockhash expiry window
    /// plus settlement latency, without persisting stale entries indefinitely.
    ///
    /// Gracefully degrades: if Redis is unavailable, returns `Ok(())` to avoid
    /// blocking payments on infrastructure failures. Log the warning.
    pub async fn check_and_record_tx(&self, tx_signature: &str) -> Result<(), ()> {
        let key = format!("rcr:txn:{}", tx_signature);

        match self.client.get_multiplexed_async_connection().await {
            Ok(mut conn) => {
                // SET key 1 NX EX 120 — atomic: only sets if key does NOT exist
                let result: Option<String> = redis::cmd("SET")
                    .arg(&key)
                    .arg("1")
                    .arg("NX")
                    .arg("EX")
                    .arg(120_u64)
                    .query_async(&mut conn)
                    .await
                    .unwrap_or(None);

                if result.is_some() {
                    // Key was newly set — first time seeing this tx
                    Ok(())
                } else {
                    // Key already existed — replay detected
                    Err(())
                }
            }
            Err(e) => {
                // Redis unavailable — log and allow through (fail-open on infra error)
                warn!(error = %e, tx = %tx_signature, "Redis unavailable for replay check, allowing payment through");
                Ok(())
            }
        }
    }
}

/// Cache error types.
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    #[error("cache connection failed: {0}")]
    Connection(String),

    #[error("cache operation failed: {0}")]
    Operation(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcr_common::types::{ChatMessage, Role};

    /// Helper to build a ChatRequest for testing.
    fn make_request(
        model: &str,
        messages: Vec<ChatMessage>,
        temperature: Option<f32>,
    ) -> ChatRequest {
        ChatRequest {
            model: model.to_string(),
            messages,
            max_tokens: None,
            temperature,
            top_p: None,
            stream: false,
        }
    }

    fn user_message(content: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: content.to_string(),
            name: None,
        }
    }

    #[test]
    fn test_cache_key_deterministic() {
        let req = make_request("openai/gpt-4o", vec![user_message("Hello")], Some(0.7));
        let key1 = ResponseCache::cache_key(&req);
        let key2 = ResponseCache::cache_key(&req);
        assert_eq!(key1, key2);
        assert!(key1.starts_with("rcr:cache:"));
    }

    #[test]
    fn test_cache_key_different_for_different_models() {
        let req_a = make_request("openai/gpt-4o", vec![user_message("Hello")], Some(0.7));
        let req_b = make_request(
            "anthropic/claude-3.5-sonnet",
            vec![user_message("Hello")],
            Some(0.7),
        );
        assert_ne!(
            ResponseCache::cache_key(&req_a),
            ResponseCache::cache_key(&req_b),
        );
    }

    #[test]
    fn test_cache_key_different_for_different_messages() {
        let req_a = make_request("openai/gpt-4o", vec![user_message("Hello")], Some(0.7));
        let req_b = make_request("openai/gpt-4o", vec![user_message("Goodbye")], Some(0.7));
        assert_ne!(
            ResponseCache::cache_key(&req_a),
            ResponseCache::cache_key(&req_b),
        );
    }

    #[test]
    fn test_cache_key_different_for_different_temperatures() {
        let req_a = make_request("openai/gpt-4o", vec![user_message("Hello")], Some(0.7));
        let req_b = make_request("openai/gpt-4o", vec![user_message("Hello")], Some(1.0));
        let req_c = make_request("openai/gpt-4o", vec![user_message("Hello")], None);
        let key_a = ResponseCache::cache_key(&req_a);
        let key_b = ResponseCache::cache_key(&req_b);
        let key_c = ResponseCache::cache_key(&req_c);
        assert_ne!(key_a, key_b);
        assert_ne!(key_a, key_c);
        assert_ne!(key_b, key_c);
    }

    #[tokio::test]
    async fn test_streaming_requests_not_cached() {
        // Use a bogus Redis URL — we should never connect because stream=true
        // causes an early return.
        let cache = ResponseCache::new("redis://127.0.0.1:1", CacheConfig::default())
            .expect("client creation should not connect");

        let req = ChatRequest {
            model: "openai/gpt-4o".to_string(),
            messages: vec![user_message("Hello")],
            max_tokens: None,
            temperature: None,
            top_p: None,
            stream: true,
        };
        assert!(cache.get(&req).await.is_none());
    }

    #[tokio::test]
    async fn test_cache_disabled() {
        let config = CacheConfig {
            default_ttl_secs: 600,
            enabled: false,
        };
        let cache = ResponseCache::new("redis://127.0.0.1:1", config)
            .expect("client creation should not connect");

        let req = make_request("openai/gpt-4o", vec![user_message("Hello")], None);
        assert!(cache.get(&req).await.is_none());
    }

    #[test]
    fn test_cache_config_default() {
        let config = CacheConfig::default();
        assert_eq!(config.default_ttl_secs, 600);
        assert!(config.enabled);
    }

    #[test]
    fn test_cache_error_display() {
        let err = CacheError::Connection("refused".to_string());
        assert_eq!(err.to_string(), "cache connection failed: refused");

        let err = CacheError::Operation("timeout".to_string());
        assert_eq!(err.to_string(), "cache operation failed: timeout");
    }
}
