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
    /// Key = SHA256(model ‖ messages_json ‖ temperature ‖ tools_json ‖
    /// tool_choice_json). Message order is significant (it's part of the
    /// conversation), so messages are NOT sorted — see
    /// `cache_key_is_sensitive_to_message_order`.
    ///
    /// `tools` and `tool_choice` are part of the key because they materially
    /// change the response shape: with tools available the model may emit a
    /// `tool_calls` completion instead of prose, and `tool_choice` can force
    /// or forbid a specific function. Two otherwise-identical requests that
    /// differ only in their tool spec would otherwise collide on the same
    /// SHA-256 key and serve each other's responses — a wrong-answer money
    /// loss for the agent that paid for a tool-capable response and received
    /// prose from cache. The fields are serialised on a stable, sorted-key
    /// JSON form to make `Some(...)` distinct from `None` regardless of map
    /// ordering quirks.
    ///
    /// Returns `None` if any of the JSON-serialised inputs cannot be
    /// serialised. **This must never be coerced to a default key**: silently
    /// omitting any field from the hash would collapse the key, making every
    /// prompt sharing the remaining fields collide — a wallet A request
    /// served wallet B's response. Callers treat `None` as a guaranteed cache
    /// miss and fall through to the upstream provider.
    pub fn cache_key(req: &ChatRequest) -> Option<String> {
        let msgs_json = match serde_json::to_string(&req.messages) {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, model = %req.model, "cache_key: failed to serialise messages; treating as guaranteed miss");
                return None;
            }
        };
        // Tool spec affects response shape (`tool_calls` vs prose). Serialise
        // `Option` directly so `None` and `Some([])` produce distinct bytes.
        let tools_json = match serde_json::to_string(&req.tools) {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, model = %req.model, "cache_key: failed to serialise tools; treating as guaranteed miss");
                return None;
            }
        };
        let tool_choice_json = match serde_json::to_string(&req.tool_choice) {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, model = %req.model, "cache_key: failed to serialise tool_choice; treating as guaranteed miss");
                return None;
            }
        };
        let mut hasher = Sha256::new();
        hasher.update(req.model.as_bytes());
        // Domain-separator bytes guard against the (theoretical) collision
        // where one field's serialised tail concatenates with the next field's
        // head into an identical byte stream as a different split. SHA-256
        // doesn't need this for security, but it makes the encoding
        // unambiguous and the test-side reasoning trivial.
        hasher.update(b"|msgs|");
        hasher.update(msgs_json.as_bytes());
        hasher.update(b"|tools|");
        hasher.update(tools_json.as_bytes());
        hasher.update(b"|tool_choice|");
        hasher.update(tool_choice_json.as_bytes());
        if let Some(temp) = req.temperature {
            hasher.update(b"|temp|");
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
                        metrics::counter!("solvela_exact_cache_read_evicted_no_usage_total")
                            .increment(1);
                        warn!(
                            key = %key,
                            "cached response has no usage block; evicting and \
                             treating as miss so the wallet's budget reservation \
                             can settle"
                        );
                        // Fire-and-forget DEL: without this, the stale entry
                        // survives until TTL (default 600s) and every read in
                        // that window re-fires the counter, re-emits the warn,
                        // and re-calls upstream. The counter is named
                        // `..._evicted_..._total` to communicate one-shot
                        // remediation to on-call — the DEL makes that honest.
                        let client = self.client.clone();
                        let evict_key = key.clone();
                        tokio::spawn(async move {
                            if let Ok(mut conn) = client.get_multiplexed_async_connection().await {
                                let _: Result<(), redis::RedisError> = redis::cmd("DEL")
                                    .arg(&evict_key)
                                    .query_async(&mut conn)
                                    .await;
                            }
                        });
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
            metrics::counter!("solvela_exact_cache_write_skipped_no_usage_total").increment(1);
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

    use crate::cache::test_metrics::{counter_value, install_test_recorder};

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
            content: content.into(),
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

    /// Tool spec is part of the response shape (`tool_calls` vs prose). Two
    /// requests with the same prompt + temperature but different tools MUST
    /// produce different keys; otherwise an agent that paid for a tool-capable
    /// response would receive a prose response cached by a tool-less peer
    /// (a wrong-answer money loss). Falsifiability: removing `req.tools` from
    /// `cache_key` would make this test fail.
    #[test]
    fn cache_key_is_sensitive_to_tools() {
        use solvela_protocol::{FunctionDefinitionInner, ToolDefinition};
        let base = make_request("openai/gpt-4o", vec![user_message("Hello")], Some(0.7));
        let with_tools = {
            let mut r = base.clone();
            r.tools = Some(vec![ToolDefinition {
                r#type: "function".to_string(),
                function: FunctionDefinitionInner {
                    name: "get_weather".to_string(),
                    description: Some("Get weather".to_string()),
                    parameters: Some(serde_json::json!({"type":"object"})),
                },
            }]);
            r
        };
        let with_other_tools = {
            let mut r = base.clone();
            r.tools = Some(vec![ToolDefinition {
                r#type: "function".to_string(),
                function: FunctionDefinitionInner {
                    name: "search_web".to_string(),
                    description: Some("Search the web".to_string()),
                    parameters: Some(serde_json::json!({"type":"object"})),
                },
            }]);
            r
        };
        let key_base = ResponseCache::cache_key(&base).expect("must serialise");
        let key_tools = ResponseCache::cache_key(&with_tools).expect("must serialise");
        let key_other = ResponseCache::cache_key(&with_other_tools).expect("must serialise");
        assert_ne!(
            key_base, key_tools,
            "request with tools must hash differently from request with no tools"
        );
        assert_ne!(
            key_tools, key_other,
            "different tool definitions must produce different keys"
        );
    }

    /// `tool_choice` (auto vs none vs a specific function) forces the response
    /// shape. Two requests with identical prompts + tools but different
    /// `tool_choice` MUST produce different keys.
    #[test]
    fn cache_key_is_sensitive_to_tool_choice() {
        let base = make_request("openai/gpt-4o", vec![user_message("Hello")], Some(0.7));
        let auto = {
            let mut r = base.clone();
            r.tool_choice = Some(serde_json::json!("auto"));
            r
        };
        let none = {
            let mut r = base.clone();
            r.tool_choice = Some(serde_json::json!("none"));
            r
        };
        let key_base = ResponseCache::cache_key(&base).expect("must serialise");
        let key_auto = ResponseCache::cache_key(&auto).expect("must serialise");
        let key_none = ResponseCache::cache_key(&none).expect("must serialise");
        assert_ne!(key_base, key_auto);
        assert_ne!(key_auto, key_none);
        assert_ne!(key_base, key_none);
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
    ///
    /// Falsifiability: asserts the `solvela_exact_cache_write_skipped_no_usage_total`
    /// counter incremented. A regression that deletes the guard would let
    /// execution continue past the counter call into the Redis path — the
    /// counter would NOT increment and this test would fail. (Earlier draft
    /// of this test passed vacuously because the bad-port Redis connection
    /// failed at step 2, observationally identical to the guard firing.)
    #[tokio::test]
    async fn set_refuses_to_cache_response_without_usage() {
        let handle = install_test_recorder();
        let before = counter_value(&handle, "solvela_exact_cache_write_skipped_no_usage_total");

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
        cache.set(&req, &response).await;

        let after = counter_value(&handle, "solvela_exact_cache_write_skipped_no_usage_total");
        assert_eq!(
            after,
            before + 1,
            "guard must increment the skip counter; otherwise a regression \
             that removed the guard would slip past this test"
        );
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

    /// Read-side defense-in-depth: `get` must evict any cached entry whose
    /// `usage` is `None`, even though the write-side guard now prevents new
    /// such entries from being stored. Verifies the read-side branch via a
    /// direct `SET` of a malformed entry that the write guard would have
    /// rejected — simulating either a pre-guard legacy entry or a future
    /// bypass.
    ///
    /// Gated on a reachable Redis (any Redis works; this test does not need
    /// RediSearch). The counter assertion makes this falsifiable: removing
    /// the read-side `usage.is_none()` check would surface as a non-incremented
    /// `solvela_exact_cache_read_evicted_no_usage_total` and a returned
    /// `Some(_)` instead of `None`.
    #[tokio::test]
    async fn get_evicts_cached_response_without_usage() {
        let redis_url =
            std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379/".to_string());

        // Probe the connection first so we can skip cleanly if Redis is down.
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

        // Build a request whose key we can compute, then SET a usage-less
        // ChatResponse JSON under that key — bypassing the write guard.
        let req = make_request(
            "openai/gpt-4o",
            vec![user_message("read-side guard probe — usage-less seed")],
            None,
        );
        let key = ResponseCache::cache_key(&req).expect("test request must serialise");
        let usage_less = ChatResponse {
            id: "legacy-no-usage".to_string(),
            object: "chat.completion".to_string(),
            created: 0,
            model: "gpt-4o".to_string(),
            choices: vec![],
            usage: None,
        };
        let json = serde_json::to_string(&usage_less).unwrap();
        let _: () = redis::cmd("SETEX")
            .arg(&key)
            .arg(60_u64)
            .arg(&json)
            .query_async(&mut conn)
            .await
            .expect("seed write must succeed");

        let handle = install_test_recorder();
        let before = counter_value(&handle, "solvela_exact_cache_read_evicted_no_usage_total");

        let result = cache.get(&req).await;
        assert!(
            result.is_none(),
            "cached entry with usage:None must be treated as miss; got {result:?}"
        );

        let after = counter_value(&handle, "solvela_exact_cache_read_evicted_no_usage_total");
        assert_eq!(
            after,
            before + 1,
            "read-side guard must increment the eviction counter; otherwise \
             a regression that removed the guard would slip past this test"
        );

        // Clean up the seeded key so a second run of this test starts clean.
        let _: () = redis::cmd("DEL")
            .arg(&key)
            .query_async(&mut conn)
            .await
            .unwrap_or(());
    }
}
