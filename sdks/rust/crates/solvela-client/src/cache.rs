use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use ahash::AHasher;
use lru::LruCache;
use solvela_protocol::{ChatMessage, ChatResponse};

const DEFAULT_MAX_ENTRIES: usize = 200;
const DEFAULT_TTL_SECS: u64 = 600;
const DEFAULT_DEDUP_WINDOW_SECS: u64 = 30;

struct CacheEntry {
    response: ChatResponse,
    inserted: Instant,
}

pub(crate) struct ResponseCache {
    inner: Mutex<LruCache<u64, CacheEntry>>,
    ttl: Duration,
    dedup_window: Duration,
}

impl ResponseCache {
    pub(crate) fn new() -> Self {
        Self::with_config(
            DEFAULT_MAX_ENTRIES,
            Duration::from_secs(DEFAULT_TTL_SECS),
            Duration::from_secs(DEFAULT_DEDUP_WINDOW_SECS),
        )
    }

    pub(crate) fn with_config(max_entries: usize, ttl: Duration, dedup_window: Duration) -> Self {
        Self {
            inner: Mutex::new(LruCache::new(
                NonZeroUsize::new(max_entries).unwrap_or(NonZeroUsize::new(1).unwrap()),
            )),
            ttl,
            dedup_window,
        }
    }

    pub(crate) fn cache_key(model: &str, messages: &[ChatMessage]) -> u64 {
        // MEDIUM-2: use AHash explicitly for stable, deterministic cache keys.
        // `std::collections::hash_map::DefaultHasher` is randomized per-process
        // and slower than necessary for non-cryptographic in-memory keys.
        let mut hasher = AHasher::default();
        model.hash(&mut hasher);
        for msg in messages {
            // Hash role as its serialized form for consistency
            let role_str = serde_json::to_string(&msg.role).unwrap_or_default();
            role_str.hash(&mut hasher);
            msg.content.hash(&mut hasher);
        }
        hasher.finish()
    }

    pub(crate) fn get(&self, key: u64) -> Option<ChatResponse> {
        let mut cache = self.inner.lock().ok()?;
        let entry = cache.peek(&key)?;
        if entry.inserted.elapsed() > self.ttl {
            cache.pop(&key);
            return None;
        }
        Some(cache.get(&key)?.response.clone())
    }

    pub(crate) fn put(&self, key: u64, response: ChatResponse) {
        let Ok(mut cache) = self.inner.lock() else {
            return;
        };
        if let Some(existing) = cache.peek(&key) {
            if existing.inserted.elapsed() < self.dedup_window {
                return;
            }
        }
        cache.put(
            key,
            CacheEntry {
                response,
                inserted: Instant::now(),
            },
        );
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.inner.lock().map_or(0, |c| c.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use solvela_protocol::{ChatChoice, Role, Usage};

    fn make_response(content: &str) -> ChatResponse {
        ChatResponse {
            id: "test-id".to_string(),
            object: "chat.completion".to_string(),
            created: 0,
            model: "test-model".to_string(),
            choices: vec![ChatChoice {
                index: 0,
                message: ChatMessage {
                    role: Role::Assistant,
                    content: content.to_string(),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: Some(Usage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            }),
        }
    }

    fn make_messages(content: &str) -> Vec<ChatMessage> {
        vec![ChatMessage {
            role: Role::User,
            content: content.to_string(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }]
    }

    #[test]
    fn cache_miss_returns_none() {
        let cache = ResponseCache::new();
        assert!(cache.get(12345).is_none());
    }

    #[test]
    fn cache_hit_returns_response() {
        let cache = ResponseCache::new();
        let resp = make_response("hello");
        let key = ResponseCache::cache_key("model-a", &make_messages("hi"));
        cache.put(key, resp.clone());
        let cached = cache.get(key).expect("should be cached");
        assert_eq!(cached.choices[0].message.content, "hello");
    }

    #[test]
    fn ttl_expiry_removes_entry() {
        let cache =
            ResponseCache::with_config(10, Duration::from_millis(1), Duration::from_millis(0));
        let key = 42;
        cache.put(key, make_response("ephemeral"));
        std::thread::sleep(Duration::from_millis(5));
        assert!(cache.get(key).is_none());
    }

    #[test]
    fn lru_eviction() {
        let cache = ResponseCache::with_config(3, Duration::from_mins(1), Duration::from_millis(0));
        cache.put(1, make_response("a"));
        cache.put(2, make_response("b"));
        cache.put(3, make_response("c"));
        assert_eq!(cache.len(), 3);

        // Inserting 4th should evict key 1 (LRU)
        cache.put(4, make_response("d"));
        assert_eq!(cache.len(), 3);
        assert!(cache.get(1).is_none());
        assert!(cache.get(4).is_some());
    }

    #[test]
    fn dedup_window_prevents_update() {
        let cache = ResponseCache::with_config(10, Duration::from_mins(1), Duration::from_mins(1));
        let key = 99;
        cache.put(key, make_response("first"));
        cache.put(key, make_response("second"));
        let cached = cache.get(key).expect("should exist");
        assert_eq!(cached.choices[0].message.content, "first");
    }

    #[test]
    fn dedup_window_expires_allows_update() {
        let cache =
            ResponseCache::with_config(10, Duration::from_mins(1), Duration::from_millis(1));
        let key = 99;
        cache.put(key, make_response("first"));
        std::thread::sleep(Duration::from_millis(5));
        cache.put(key, make_response("second"));
        let cached = cache.get(key).expect("should exist");
        assert_eq!(cached.choices[0].message.content, "second");
    }

    #[test]
    fn cache_key_deterministic() {
        let msgs = make_messages("hello world");
        let k1 = ResponseCache::cache_key("model-x", &msgs);
        let k2 = ResponseCache::cache_key("model-x", &msgs);
        assert_eq!(k1, k2);
    }

    #[test]
    fn cache_key_different_for_different_models() {
        let msgs = make_messages("hello world");
        let k1 = ResponseCache::cache_key("model-a", &msgs);
        let k2 = ResponseCache::cache_key("model-b", &msgs);
        assert_ne!(k1, k2);
    }
}
