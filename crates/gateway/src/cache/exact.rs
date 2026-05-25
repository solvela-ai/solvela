//! Tier 1 — exact-match response cache.
//!
//! Keys are `SHA-256(model ‖ serialised_messages ‖ temperature)` over Redis.
//! See the [`super`] module docs for the wallet-agnostic key design and its
//! cost/margin trade-off. These methods hang off [`ResponseCache`]; the struct
//! and its shared infra (connection handling, replay protection) live in
//! `mod.rs`.

use sha2::{Digest, Sha256};
use tracing::{info, warn};

use solvela_protocol::{ChatRequest, ChatResponse};

use super::{ResponseCache, CACHE_KEY_PREFIX};

impl ResponseCache {
    /// Generate a cache key from a request.
    /// Key = SHA256(model + messages_json + temperature). Message order is
    /// significant (it's part of the conversation), so messages are NOT sorted —
    /// see `cache_key_is_sensitive_to_message_order`.
    ///
    /// Returns `None` if `messages` cannot be serialised. **This must never
    /// be coerced to a default key**: silently omitting messages from the
    /// hash would collapse the key to `SHA-256(model ‖ temperature)`, making
    /// every prompt sharing those two fields collide — a wallet A request
    /// served wallet B's response. Callers treat `None` as a guaranteed cache
    /// miss and fall through to the upstream provider. In practice
    /// `serde_json::to_string` on `Vec<ChatMessage>` (plain `String` content +
    /// scalars) does not fail, so this branch is defence-in-depth.
    pub fn cache_key(req: &ChatRequest) -> Option<String> {
        let msgs_json = match serde_json::to_string(&req.messages) {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, model = %req.model, "cache_key: failed to serialise messages; treating as guaranteed miss");
                return None;
            }
        };
        let mut hasher = Sha256::new();
        hasher.update(req.model.as_bytes());
        hasher.update(msgs_json.as_bytes());
        if let Some(temp) = req.temperature {
            hasher.update(temp.to_le_bytes());
        }
        let hash = hasher.finalize();
        Some(format!("{}{}", CACHE_KEY_PREFIX, hex::encode(hash)))
    }

    /// Try to get a cached response.
    pub async fn get(&self, req: &ChatRequest) -> Option<ChatResponse> {
        if !self.config.enabled || req.stream {
            return None;
        }
        let key = Self::cache_key(req)?;

        let mut conn = self.client.get_multiplexed_async_connection().await.ok()?;
        let cached: Option<String> = redis::cmd("GET")
            .arg(&key)
            .query_async(&mut conn)
            .await
            .ok()?;

        match cached {
            Some(json_str) => match serde_json::from_str::<ChatResponse>(&json_str) {
                Ok(response) => {
                    // Defense-in-depth: the write-side guard in `set` blocks
                    // new caching of usage-less responses, but entries written
                    // before this guard (or via a future bypass) could still
                    // be in Redis. Serving them would skip both `log_spend`
                    // reconciliation branches in `chat/mod.rs` and strand the
                    // `check_budget` reservation. Treat as a miss → upstream
                    // call, real settlement.
                    if response.usage.is_none() {
                        warn!(
                            key = %key,
                            "cached response has no usage block; treating as miss \
                             so the wallet's budget reservation can settle"
                        );
                        return None;
                    }
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
    ///
    /// Refuses to cache responses that lack a `usage` block. On a future hit
    /// the downstream `log_spend` reconciliation branches both require either
    /// `usage` or a semantic discount; an exact hit returns `cost_outcome: None`
    /// and threads `cached.usage` through, so a `None` usage would skip
    /// settlement and permanently strand the `check_budget` reservation on the
    /// wallet's counter (drains budget by `estimated_cost` per repeat).
    pub async fn set(&self, req: &ChatRequest, response: &ChatResponse) {
        if !self.config.enabled || req.stream {
            return;
        }
        if response.usage.is_none() {
            warn!(
                model = %req.model,
                "refusing to cache response with no usage block; \
                 settlement requires usage to reconcile the budget reservation"
            );
            return;
        }
        let key = match Self::cache_key(req) {
            Some(k) => k,
            None => return, // serialisation failure already logged inside `cache_key`
        };

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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::CacheConfig;
    use solvela_protocol::{ChatMessage, Role};

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
            tools: None,
            tool_choice: None,
        }
    }

    fn user_message(content: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: content.to_string(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    #[test]
    fn test_cache_key_deterministic() {
        let req = make_request("openai/gpt-4o", vec![user_message("Hello")], Some(0.7));
        let key1 = ResponseCache::cache_key(&req).expect("test request must serialise");
        let key2 = ResponseCache::cache_key(&req).expect("test request must serialise");
        assert_eq!(key1, key2);
        assert!(key1.starts_with("solvela:cache:"));
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
            ResponseCache::cache_key(&req_a).expect("test request must serialise"),
            ResponseCache::cache_key(&req_b).expect("test request must serialise"),
        );
    }

    #[test]
    fn test_cache_key_different_for_different_messages() {
        let req_a = make_request("openai/gpt-4o", vec![user_message("Hello")], Some(0.7));
        let req_b = make_request("openai/gpt-4o", vec![user_message("Goodbye")], Some(0.7));
        assert_ne!(
            ResponseCache::cache_key(&req_a).expect("test request must serialise"),
            ResponseCache::cache_key(&req_b).expect("test request must serialise"),
        );
    }

    #[test]
    fn test_cache_key_different_for_different_temperatures() {
        let req_a = make_request("openai/gpt-4o", vec![user_message("Hello")], Some(0.7));
        let req_b = make_request("openai/gpt-4o", vec![user_message("Hello")], Some(1.0));
        let req_c = make_request("openai/gpt-4o", vec![user_message("Hello")], None);
        let key_a = ResponseCache::cache_key(&req_a).expect("test request must serialise");
        let key_b = ResponseCache::cache_key(&req_b).expect("test request must serialise");
        let key_c = ResponseCache::cache_key(&req_c).expect("test request must serialise");
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
            tools: None,
            tool_choice: None,
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

    /// CLAUDE.md rule #16: cache keys are wallet-agnostic. Two payers with
    /// identical (model, messages, temperature) MUST produce the same key,
    /// regardless of any other field on the request. We prove this structurally
    /// by varying every non-key field and asserting the hash is unchanged.
    #[test]
    fn cache_key_ignores_non_key_request_fields() {
        let base = make_request("openai/gpt-4o", vec![user_message("Hello")], Some(0.7));
        let key_base = ResponseCache::cache_key(&base).expect("test request must serialise");

        // max_tokens differs — must not affect the key.
        let mut variant = base.clone();
        variant.max_tokens = Some(2048);
        assert_eq!(
            ResponseCache::cache_key(&variant).expect("test request must serialise"),
            key_base,
            "max_tokens must not be part of the cache key"
        );

        // top_p differs — must not affect the key.
        let mut variant = base.clone();
        variant.top_p = Some(0.9);
        assert_eq!(
            ResponseCache::cache_key(&variant).expect("test request must serialise"),
            key_base,
            "top_p must not be part of the cache key"
        );

        // stream differs — cache_key itself ignores it (the get/set methods
        // gate on stream separately).
        let mut variant = base.clone();
        variant.stream = true;
        assert_eq!(
            ResponseCache::cache_key(&variant).expect("test request must serialise"),
            key_base,
            "stream flag must not be part of the cache key"
        );
    }

    /// Message order is part of the conversation; two prompts with the same
    /// content in different order are NOT the same prompt.
    #[test]
    fn cache_key_is_sensitive_to_message_order() {
        let req_a = make_request(
            "openai/gpt-4o",
            vec![user_message("first"), user_message("second")],
            Some(0.7),
        );
        let req_b = make_request(
            "openai/gpt-4o",
            vec![user_message("second"), user_message("first")],
            Some(0.7),
        );
        assert_ne!(
            ResponseCache::cache_key(&req_a).expect("test request must serialise"),
            ResponseCache::cache_key(&req_b).expect("test request must serialise"),
            "message order must affect the cache key"
        );
    }

    /// Adding a message must produce a different key.
    #[test]
    fn cache_key_is_sensitive_to_message_count() {
        let req_a = make_request("openai/gpt-4o", vec![user_message("Hello")], Some(0.7));
        let req_b = make_request(
            "openai/gpt-4o",
            vec![user_message("Hello"), user_message("again")],
            Some(0.7),
        );
        assert_ne!(
            ResponseCache::cache_key(&req_a).expect("test request must serialise"),
            ResponseCache::cache_key(&req_b).expect("test request must serialise"),
            "additional messages must affect the cache key"
        );
    }

    /// `set` early-exits for streaming requests without spawning a Redis writer.
    /// We use a bogus Redis URL — if `set` tried to connect, the spawned task
    /// would log a warning, but the function itself must return immediately.
    #[tokio::test]
    async fn set_short_circuits_for_streaming_requests() {
        let cache = ResponseCache::new("redis://127.0.0.1:1", CacheConfig::default())
            .expect("client creation should not connect");

        let req = ChatRequest {
            model: "openai/gpt-4o".to_string(),
            messages: vec![user_message("Hello")],
            max_tokens: None,
            temperature: None,
            top_p: None,
            stream: true,
            tools: None,
            tool_choice: None,
        };
        let response = ChatResponse {
            id: "test".to_string(),
            object: "chat.completion".to_string(),
            created: 0,
            model: "gpt-4o".to_string(),
            choices: vec![],
            usage: None,
        };
        // Should return without panic; nothing to assert besides "doesn't try to connect".
        cache.set(&req, &response).await;
    }

    /// `set` refuses to cache a response with no `usage` block — serving it on a
    /// future hit would skip both `log_spend` reconciliation branches in
    /// `chat/mod.rs` (one needs `usage`, the other needs a semantic
    /// `cost_outcome`), permanently stranding the `check_budget` reservation
    /// and draining the wallet's budget by `estimated_cost` per repeat.
    #[tokio::test]
    async fn set_refuses_to_cache_response_without_usage() {
        let cache = ResponseCache::new("redis://127.0.0.1:1", CacheConfig::default())
            .expect("client creation should not connect");
        let req = make_request("openai/gpt-4o", vec![user_message("Hello")], None);
        let response = ChatResponse {
            id: "test".to_string(),
            object: "chat.completion".to_string(),
            created: 0,
            model: "gpt-4o".to_string(),
            choices: vec![],
            usage: None,
        };
        // No panic, no Redis connection attempt — early-exit on the usage guard.
        cache.set(&req, &response).await;
    }

    /// `set` early-exits when caching is disabled.
    #[tokio::test]
    async fn set_short_circuits_when_disabled() {
        let config = CacheConfig {
            default_ttl_secs: 600,
            enabled: false,
        };
        let cache = ResponseCache::new("redis://127.0.0.1:1", config)
            .expect("client creation should not connect");

        let req = make_request("openai/gpt-4o", vec![user_message("Hello")], None);
        let response = ChatResponse {
            id: "test".to_string(),
            object: "chat.completion".to_string(),
            created: 0,
            model: "gpt-4o".to_string(),
            choices: vec![],
            usage: None,
        };
        cache.set(&req, &response).await;
    }
}
