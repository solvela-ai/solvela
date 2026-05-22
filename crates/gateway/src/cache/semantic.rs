//! Tier 2 — semantic (embedding-based) response cache.
//!
//! Where the exact tier matches `SHA-256(model ‖ messages ‖ temperature)`, this
//! tier matches by *prompt-embedding similarity*: a paraphrase of a previously
//! answered prompt can hit. Backed by RediSearch (`FT.CREATE` / `FT.SEARCH` KNN
//! over an HNSW cosine index) so the lookup is a single vector query.
//!
//! ## Scoping & correctness
//! Hits are scoped to the **same model** (stored as a RediSearch TAG and filtered
//! in the query). A cached `openai/gpt-4o` answer must never be served for an
//! `anthropic/...` request — the response shape and pricing differ. Like the
//! exact tier this stays **wallet-agnostic** (CLAUDE.md rule #16): the stored
//! entry holds no payer identity.
//!
//! ## Lookup gate
//! A candidate is returned only if `similarity >= threshold` (default 0.85,
//! operator-configurable). RediSearch returns cosine *distance* (`0` identical,
//! `2` opposite); we convert to similarity (`1 - distance`).

use std::sync::Arc;

use redis::aio::ConnectionManager;

use solvela_protocol::{ChatRequest, ChatResponse};

use super::embedder::Embedder;

/// Redis key prefix for semantic cache entries (distinct from the exact tier's
/// `solvela:cache:` so the two never collide in the keyspace).
const SEMANTIC_PREFIX: &str = "solvela:scache";
/// RediSearch index name over the semantic entries.
const SEMANTIC_INDEX: &str = "solvela_scache_idx";

/// Configuration for the semantic cache tier.
#[derive(Debug, Clone)]
pub struct SemanticConfig {
    /// Whether the semantic tier is enabled. Default off → zero behaviour change.
    pub enabled: bool,
    /// Minimum cosine similarity for a hit, in `[0, 1]`.
    pub threshold: f32,
    /// TTL (seconds) for stored semantic entries.
    pub ttl_secs: u64,
    /// Embedding dimension (must match the embedder).
    pub dim: usize,
}

impl Default for SemanticConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold: 0.85,
            ttl_secs: 600,
            dim: super::embedder::BGE_SMALL_DIM,
        }
    }
}

/// A semantic-cache hit: the stored response plus the similarity that matched it.
#[derive(Debug, Clone)]
pub struct SemanticHit {
    pub response: ChatResponse,
    pub similarity: f32,
}

/// Semantic cache backed by RediSearch + a pluggable embedder.
pub struct SemanticCache {
    conn: ConnectionManager,
    embedder: Arc<dyn Embedder>,
    config: SemanticConfig,
}

impl SemanticCache {
    /// Connect to Redis and ensure the vector index exists.
    pub async fn connect(
        redis_url: &str,
        embedder: Arc<dyn Embedder>,
        config: SemanticConfig,
    ) -> Result<Self, super::CacheError> {
        let client = redis::Client::open(redis_url)
            .map_err(|e| super::CacheError::Connection(e.to_string()))?;
        let conn = client
            .get_connection_manager()
            .await
            .map_err(|e| super::CacheError::Connection(e.to_string()))?;
        let cache = Self {
            conn,
            embedder,
            config,
        };
        cache.ensure_index().await?;
        Ok(cache)
    }

    /// Idempotently create the RediSearch index. No-op if it already exists.
    pub async fn ensure_index(&self) -> Result<(), super::CacheError> {
        let mut conn = self.conn.clone();
        let info: redis::RedisResult<redis::Value> = redis::cmd("FT.INFO")
            .arg(SEMANTIC_INDEX)
            .query_async(&mut conn)
            .await;
        if info.is_ok() {
            return Ok(());
        }
        redis::cmd("FT.CREATE")
            .arg(SEMANTIC_INDEX)
            .arg("ON")
            .arg("HASH")
            .arg("PREFIX")
            .arg(1)
            .arg(format!("{SEMANTIC_PREFIX}:"))
            .arg("SCHEMA")
            .arg("model")
            .arg("TAG")
            .arg("response")
            .arg("TEXT")
            .arg("embedding")
            .arg("VECTOR")
            .arg("HNSW")
            .arg(6)
            .arg("TYPE")
            .arg("FLOAT32")
            .arg("DIM")
            .arg(self.config.dim as i64)
            .arg("DISTANCE_METRIC")
            .arg("COSINE")
            .query_async::<()>(&mut conn)
            .await
            .map_err(|e| super::CacheError::Operation(e.to_string()))?;
        Ok(())
    }

    /// Look up a semantically-similar cached response for this request.
    /// Returns `None` on miss, when disabled, or for streaming requests.
    pub async fn get(&self, _req: &ChatRequest) -> Option<SemanticHit> {
        None // RED: lookup not yet implemented
    }

    /// Store a response under this request's prompt embedding.
    pub async fn set(&self, _req: &ChatRequest, _response: &ChatResponse) {
        // RED: store not yet implemented
    }
}

/// Canonicalise a chat request's messages into the single string we embed.
/// Deterministic and order-sensitive (message order changes meaning).
pub(crate) fn prompt_text(_req: &ChatRequest) -> String {
    String::new() // RED: not yet implemented
}

/// Escape a value for safe use inside a RediSearch TAG filter (`@model:{...}`).
/// RediSearch treats much of ASCII punctuation as special inside tags; model
/// IDs like `openai/gpt-4o` contain `/` and `-`, so every non-alphanumeric byte
/// is backslash-escaped.
pub(crate) fn escape_tag(value: &str) -> String {
    value.to_string() // RED: not yet implemented
}

/// Encode an f32 slice as little-endian bytes for a RediSearch vector param.
/// Only used by the GREEN store/lookup implementation.
#[allow(dead_code)]
fn f32_slice_to_le_bytes(v: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(v.len() * 4);
    for &f in v {
        bytes.extend_from_slice(&f.to_le_bytes());
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::embedder::LocalBge;
    use solvela_protocol::{ChatMessage, Role, Usage};
    use std::path::PathBuf;

    fn model_cache_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.fastembed_cache")
    }

    fn redis_url() -> String {
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379/".to_string())
    }

    fn user_msg(content: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: content.to_string(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    fn req(model: &str, content: &str) -> ChatRequest {
        ChatRequest {
            model: model.to_string(),
            messages: vec![user_msg(content)],
            max_tokens: None,
            temperature: None,
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
        }
    }

    fn resp(text: &str) -> ChatResponse {
        ChatResponse {
            id: format!("resp-{text}"),
            object: "chat.completion".to_string(),
            created: 0,
            model: "test".to_string(),
            choices: vec![],
            usage: Some(Usage {
                prompt_tokens: 1,
                completion_tokens: 1,
                total_tokens: 2,
            }),
        }
    }

    /// Build a SemanticCache against a fresh index, or skip if model/redis are
    /// unavailable (mirrors the redis-gated + model-gated patterns elsewhere).
    async fn fresh_cache(threshold: f32) -> Option<SemanticCache> {
        let embedder = LocalBge::with_cache_dir(model_cache_dir()).ok()?;
        let config = SemanticConfig {
            enabled: true,
            threshold,
            ttl_secs: 600,
            dim: embedder.dim(),
        };
        let cache = match SemanticCache::connect(&redis_url(), Arc::new(embedder), config).await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("skipping: redis-stack unreachable ({e}). `docker compose up -d redis`.");
                return None;
            }
        };
        // Drop any prior index + entries so tests are isolated.
        let mut conn = cache.conn.clone();
        let _: redis::RedisResult<redis::Value> = redis::cmd("FT.DROPINDEX")
            .arg(SEMANTIC_INDEX)
            .arg("DD")
            .query_async(&mut conn)
            .await;
        cache.ensure_index().await.ok()?;
        Some(cache)
    }

    // ---- pure helpers (no infra) ----

    #[test]
    fn prompt_text_is_order_sensitive_and_nonempty() {
        let mut a = req("m", "first");
        a.messages.push(user_msg("second"));
        let mut b = req("m", "second");
        b.messages.push(user_msg("first"));
        let ta = prompt_text(&a);
        let tb = prompt_text(&b);
        assert!(!ta.is_empty(), "prompt_text must not be empty for non-empty messages");
        assert_ne!(ta, tb, "message order must change the embedded text");
    }

    #[test]
    fn prompt_text_includes_message_content() {
        let r = req("m", "explain semaphores");
        assert!(
            prompt_text(&r).contains("explain semaphores"),
            "prompt_text should include the user content"
        );
    }

    #[test]
    fn escape_tag_escapes_model_punctuation() {
        // `/`, `-`, and `.` are RediSearch tag-special and must be escaped.
        let escaped = escape_tag("openai/gpt-4o");
        assert_eq!(escaped, "openai\\/gpt\\-4o");
        let escaped = escape_tag("anthropic/claude-3.5-sonnet");
        assert_eq!(escaped, "anthropic\\/claude\\-3\\.5\\-sonnet");
    }

    #[test]
    fn escape_tag_leaves_alphanumerics() {
        assert_eq!(escape_tag("gpt4o"), "gpt4o");
    }

    // ---- model-backed + redis-backed (skip if unavailable) ----

    #[tokio::test]
    async fn exact_prompt_hits_with_high_similarity() {
        let Some(cache) = fresh_cache(0.85).await else { return };
        let r = req("openai/gpt-4o", "What is the capital of France?");
        cache.set(&r, &resp("paris")).await;
        let hit = cache.get(&r).await.expect("identical prompt should hit");
        assert_eq!(hit.response.id, "resp-paris");
        assert!(hit.similarity > 0.99, "identical prompt similarity {} too low", hit.similarity);
    }

    #[tokio::test]
    async fn paraphrase_hits_above_threshold() {
        let Some(cache) = fresh_cache(0.85).await else { return };
        let stored = req("openai/gpt-4o", "What is the capital of France?");
        cache.set(&stored, &resp("paris")).await;
        let query = req("openai/gpt-4o", "What's France's capital?");
        let hit = cache.get(&query).await.expect("paraphrase should hit");
        assert_eq!(hit.response.id, "resp-paris");
        assert!(hit.similarity >= 0.85);
    }

    #[tokio::test]
    async fn unrelated_prompt_misses() {
        let Some(cache) = fresh_cache(0.85).await else { return };
        cache
            .set(&req("openai/gpt-4o", "What is the capital of France?"), &resp("paris"))
            .await;
        let miss = cache
            .get(&req("openai/gpt-4o", "How do I make sourdough bread?"))
            .await;
        assert!(miss.is_none(), "unrelated prompt must miss, got {miss:?}");
    }

    #[tokio::test]
    async fn different_model_misses() {
        let Some(cache) = fresh_cache(0.85).await else { return };
        cache
            .set(&req("openai/gpt-4o", "What is the capital of France?"), &resp("paris"))
            .await;
        // Same prompt, different model — must not hit (response/pricing differ).
        let miss = cache
            .get(&req("anthropic/claude-3.5-sonnet", "What is the capital of France?"))
            .await;
        assert!(miss.is_none(), "cross-model hit must not happen, got {miss:?}");
    }

    #[tokio::test]
    async fn streaming_request_is_not_served_from_cache() {
        let Some(cache) = fresh_cache(0.85).await else { return };
        let mut r = req("openai/gpt-4o", "What is the capital of France?");
        cache.set(&r, &resp("paris")).await;
        r.stream = true;
        assert!(cache.get(&r).await.is_none(), "streaming requests must not hit the cache");
    }

    #[tokio::test]
    async fn disabled_cache_returns_none() {
        let embedder = match LocalBge::with_cache_dir(model_cache_dir()) {
            Ok(e) => e,
            Err(_) => return,
        };
        let config = SemanticConfig {
            enabled: false,
            ..SemanticConfig::default()
        };
        let cache = match SemanticCache::connect(&redis_url(), Arc::new(embedder), config).await {
            Ok(c) => c,
            Err(_) => return,
        };
        let r = req("openai/gpt-4o", "What is the capital of France?");
        cache.set(&r, &resp("paris")).await;
        assert!(cache.get(&r).await.is_none(), "disabled cache must return None");
    }
}
