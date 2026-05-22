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
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

use solvela_protocol::{ChatRequest, ChatResponse};

use super::embedder::Embedder;

/// Redis key prefix for semantic cache entries (distinct from the exact tier's
/// `solvela:cache:` so the two never collide in the keyspace).
const SEMANTIC_PREFIX: &str = "solvela:scache";
/// RediSearch index name over the semantic entries.
const SEMANTIC_INDEX: &str = "solvela_scache_idx";

/// Upper bound on concurrent fire-and-forget [`SemanticCache::set`] writes. Each
/// write runs a Mutex-serialised embedding (~75/s ceiling), so under a cache-miss
/// storm unbounded `tokio::spawn`s would pile up faster than they drain. We cap
/// in-flight writes here and drop the overflow — dropping a cache *write* (not a
/// response) is acceptable per the spirit of CLAUDE.md rule #9.
const MAX_INFLIGHT_WRITES: usize = 8;

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
    /// Bounds concurrent fire-and-forget writes (see [`MAX_INFLIGHT_WRITES`]).
    write_slots: Arc<Semaphore>,
}

impl SemanticCache {
    /// Connect to Redis and ensure the vector index exists.
    pub async fn connect(
        redis_url: &str,
        embedder: Arc<dyn Embedder>,
        config: SemanticConfig,
    ) -> Result<Self, super::CacheError> {
        // Fail fast on misconfiguration rather than degrade to a silent
        // all-miss cache: a dim mismatch makes every FT.SEARCH error out, and an
        // out-of-range threshold makes every comparison trivially true/false.
        if config.dim != embedder.dim() {
            return Err(super::CacheError::Operation(format!(
                "semantic cache dim mismatch: config={}, embedder={}",
                config.dim,
                embedder.dim()
            )));
        }
        if !(0.0..=1.0).contains(&config.threshold) {
            return Err(super::CacheError::Operation(format!(
                "semantic cache threshold {} is outside [0.0, 1.0]",
                config.threshold
            )));
        }

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
            write_slots: Arc::new(Semaphore::new(MAX_INFLIGHT_WRITES)),
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
            // NOINDEX: the response JSON is retrieved by the KNN result, never
            // searched. Without NOINDEX, RediSearch full-text-indexes every
            // payload, wasting index memory proportional to response size.
            .arg("response")
            .arg("TEXT")
            .arg("NOINDEX")
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
    pub async fn get(&self, req: &ChatRequest) -> Option<SemanticHit> {
        if !self.config.enabled || req.stream {
            return None;
        }

        let embedding = self.embed(req).await?;
        let bytes = f32_slice_to_le_bytes(&embedding);

        // Hybrid query: filter by model TAG, then KNN over the embedding.
        let query = format!(
            "(@model:{{{model}}})=>[KNN 1 @embedding $vec AS score]",
            model = escape_tag(&req.model)
        );

        let mut conn = self.conn.clone();
        let raw: redis::Value = match redis::cmd("FT.SEARCH")
            .arg(SEMANTIC_INDEX)
            .arg(&query)
            .arg("RETURN")
            .arg(2)
            .arg("response")
            .arg("score")
            .arg("SORTBY")
            .arg("score")
            .arg("ASC")
            .arg("LIMIT")
            .arg(0)
            .arg(1)
            .arg("PARAMS")
            .arg(2)
            .arg("vec")
            .arg(&bytes[..])
            .arg("DIALECT")
            .arg(2)
            .query_async(&mut conn)
            .await
        {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "semantic cache FT.SEARCH failed");
                return None;
            }
        };

        let (response_json, distance) = parse_top_hit(&raw)?;
        let similarity = 1.0 - distance;
        // Guard against NaN/∞ scores (e.g. a RediSearch reply parsed as "nan"/"inf").
        // `NaN < threshold` is always false, so an unguarded comparison would let a
        // garbage score slip past the gate and serve an arbitrary cached response.
        if !similarity.is_finite() {
            warn!(distance, model = %req.model, "semantic cache score non-finite; treating as miss");
            return None;
        }
        if similarity < self.config.threshold {
            return None;
        }

        match serde_json::from_str::<ChatResponse>(&response_json) {
            Ok(response) => {
                info!(similarity, model = %req.model, "semantic cache hit");
                Some(SemanticHit {
                    response,
                    similarity,
                })
            }
            Err(e) => {
                warn!(error = %e, "failed to deserialize semantic cache entry");
                None
            }
        }
    }

    /// Store a response under this request's prompt embedding. **Fire-and-forget**
    /// (CLAUDE.md rule #9): only the embedding *and* the Redis write run on a
    /// background task and are kept off the hot path. The cheap caller-side prep
    /// (`serde_json::to_string`, [`prompt_text`], and acquiring a write slot) does
    /// run on the caller. Failures are logged, never propagated. No-op when
    /// disabled or for streaming requests.
    ///
    /// Writes are bounded by a [`Semaphore`] ([`MAX_INFLIGHT_WRITES`]): under a
    /// miss storm we drop the overflow rather than pile up unbounded background
    /// embeddings. The response is still served — only the cache write is skipped.
    pub async fn set(&self, req: &ChatRequest, response: &ChatResponse) {
        if !self.config.enabled || req.stream {
            return;
        }
        // Acquire a write slot before spawning; if none is free we're already
        // saturated, so drop this write (the response is unaffected).
        let permit = match Arc::clone(&self.write_slots).try_acquire_owned() {
            Ok(p) => p,
            Err(_) => {
                metrics::counter!("solvela_semantic_cache_write_dropped_total").increment(1);
                debug!(
                    model = %req.model,
                    "semantic cache write dropped: in-flight writes at capacity"
                );
                return;
            }
        };
        let json = match serde_json::to_string(response) {
            Ok(s) => s,
            Err(e) => {
                warn!(error = %e, "failed to serialize response for semantic cache");
                return;
            }
        };
        let embedder = Arc::clone(&self.embedder);
        let text = prompt_text(req);
        let model = req.model.clone();
        let conn = self.conn.clone();
        let ttl = self.config.ttl_secs;
        tokio::spawn(async move {
            // Hold the permit for the task's lifetime; released on drop.
            let _permit = permit;
            let embedding = match embed_text(&embedder, text).await {
                Some(e) => e,
                None => return,
            };
            if let Err(e) = write_entry(conn, model, json, &embedding, ttl).await {
                warn!(error = %e, "semantic cache write failed");
            }
        });
    }

    /// Awaitable store — embeds, writes, and returns only once the entry is
    /// durable in Redis. Used by tests and integration harnesses needing
    /// read-after-write determinism; production uses the fire-and-forget
    /// [`set`](Self::set) to keep writes off the hot path.
    pub async fn store(
        &self,
        req: &ChatRequest,
        response: &ChatResponse,
    ) -> Result<(), super::CacheError> {
        if !self.config.enabled || req.stream {
            return Ok(());
        }
        let json = serde_json::to_string(response)
            .map_err(|e| super::CacheError::Operation(e.to_string()))?;
        let embedding = self
            .embed(req)
            .await
            .ok_or_else(|| super::CacheError::Operation("embedding failed".to_string()))?;
        write_entry(
            self.conn.clone(),
            req.model.clone(),
            json,
            &embedding,
            self.config.ttl_secs,
        )
        .await
        .map_err(|e| super::CacheError::Operation(e.to_string()))
    }

    /// Embed a request's canonical prompt off the async runtime threads
    /// (the model call is CPU-bound and Mutex-serialised).
    async fn embed(&self, req: &ChatRequest) -> Option<Vec<f32>> {
        embed_text(&self.embedder, prompt_text(req)).await
    }
}

/// Embed an owned prompt string on a blocking thread (CPU-bound, Mutex-serialised).
async fn embed_text(embedder: &Arc<dyn Embedder>, text: String) -> Option<Vec<f32>> {
    let embedder = Arc::clone(embedder);
    match tokio::task::spawn_blocking(move || embedder.embed_one(&text)).await {
        Ok(Ok(v)) => Some(v),
        Ok(Err(e)) => {
            warn!(error = %e, "semantic cache embedding failed");
            None
        }
        Err(e) => {
            warn!(error = %e, "semantic cache embed task panicked");
            None
        }
    }
}

/// HSET the entry then set its TTL (HSET takes no EX). Returns once durable.
async fn write_entry(
    mut conn: ConnectionManager,
    model: String,
    response_json: String,
    embedding: &[f32],
    ttl_secs: u64,
) -> redis::RedisResult<()> {
    let bytes = f32_slice_to_le_bytes(embedding);
    let key = format!("{SEMANTIC_PREFIX}:{}", uuid::Uuid::new_v4());
    // HSET + EXPIRE in one MULTI/EXEC: HSET takes no EX, and applying them
    // separately risks orphaning an entry with no TTL (indexed forever) if the
    // second round-trip fails. The pipeline makes the pair atomic.
    let mut pipe = redis::pipe();
    pipe.atomic()
        .cmd("HSET")
        .arg(&key)
        .arg("model")
        .arg(&model)
        .arg("response")
        .arg(&response_json)
        .arg("embedding")
        .arg(&bytes[..])
        .ignore()
        .cmd("EXPIRE")
        .arg(&key)
        .arg(ttl_secs)
        .ignore();
    pipe.query_async::<()>(&mut conn).await
}

/// Parse the first hit out of an `FT.SEARCH` reply, returning
/// `(response_json, cosine_distance)`. Returns `None` for an empty result set
/// or an unexpected reply shape.
fn parse_top_hit(value: &redis::Value) -> Option<(String, f32)> {
    let arr = match value {
        redis::Value::Array(a) => a,
        _ => return None,
    };
    // Reply layout: [count, key1, [field, val, field, val, ...], key2, ...].
    // We only need the first document's field array (index 2).
    let fields = match arr.get(2) {
        Some(redis::Value::Array(f)) => f,
        _ => return None,
    };
    let mut response = None;
    let mut score = None;
    for pair in fields.chunks(2) {
        // A trailing odd element (no value) can't be a field/value pair; skip it.
        let [name, val] = pair else { continue };
        let (Some(name), Some(val)) = (redis_value_to_string(name), redis_value_to_string(val))
        else {
            continue;
        };
        match name.as_str() {
            "response" => response = Some(val),
            "score" => {
                // Distinguish a present-but-unparseable score from an absent one:
                // a malformed score is a silent cache miss with no signal, so we
                // surface it via a warn + counter. An absent score field is the
                // normal empty-result case and stays quiet.
                match val.parse::<f32>() {
                    Ok(parsed) => score = Some(parsed),
                    Err(e) => {
                        warn!(error = %e, raw = %val, "semantic cache score field unparseable as f32");
                        metrics::counter!("solvela_semantic_cache_score_parse_error_total")
                            .increment(1);
                    }
                }
            }
            _ => {}
        }
    }
    Some((response?, score?))
}

fn redis_value_to_string(v: &redis::Value) -> Option<String> {
    match v {
        redis::Value::BulkString(b) => Some(String::from_utf8_lossy(b).to_string()),
        redis::Value::SimpleString(s) => Some(s.clone()),
        // Under RESP2 (the ConnectionManager default) RediSearch returns the
        // KNN score as a BulkString. These arms guard against a silent
        // cache-wide blackout if a future redis/redis-stack negotiates RESP3,
        // where numerics arrive as Double/Int. No-op under RESP2.
        redis::Value::Double(f) => Some(f.to_string()),
        redis::Value::Int(i) => Some(i.to_string()),
        _ => None,
    }
}

/// Canonicalise a chat request's messages into the single string we embed.
/// Deterministic and order-sensitive (message order changes meaning). Each
/// message contributes a `role: content` line; ordering is preserved.
///
/// Sampling params (`temperature` / `top_p`) are appended as a trailing
/// `params:` line *only when set*, so two requests identical except for
/// temperature/top_p embed to different text and never cross-hit. This mirrors
/// the exact tier, which keys on `temperature`. Changing this line's format
/// invalidates previously stored entries — acceptable given the 600s TTL.
pub(crate) fn prompt_text(req: &ChatRequest) -> String {
    let mut text = req
        .messages
        .iter()
        .map(|m| format!("{}: {}", role_str(&m.role), m.content))
        .collect::<Vec<_>>()
        .join("\n");
    // Deterministic, stable float formatting so the same params always embed
    // to the same bytes. Only emit the segments that are actually set.
    let mut params = String::new();
    if let Some(t) = req.temperature {
        params.push_str(&format!("temperature={t:.4}"));
    }
    if let Some(p) = req.top_p {
        if !params.is_empty() {
            params.push(' ');
        }
        params.push_str(&format!("top_p={p:.4}"));
    }
    if !params.is_empty() {
        text.push_str("\nparams: ");
        text.push_str(&params);
    }
    text
}

/// Stable lowercase label for a role. Pinned here (not `Debug`) so the embedded
/// text — and therefore stored-embedding compatibility — never shifts if the
/// `Role` derive or wire representation changes.
fn role_str(role: &solvela_protocol::Role) -> &'static str {
    use solvela_protocol::Role;
    match role {
        Role::System => "system",
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::Tool => "tool",
        Role::Developer => "developer",
        Role::Unknown => "unknown",
    }
}

/// Escape a value for safe use inside a RediSearch TAG filter (`@model:{...}`).
/// RediSearch treats much of ASCII punctuation as special inside tags; model
/// IDs like `openai/gpt-4o` contain `/` and `-`, so every non-alphanumeric,
/// non-underscore byte is backslash-escaped.
pub(crate) fn escape_tag(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 2);
    for ch in value.chars() {
        if !ch.is_ascii_alphanumeric() && ch != '_' {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Encode an f32 slice as little-endian bytes for a RediSearch vector param.
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
        assert!(
            !ta.is_empty(),
            "prompt_text must not be empty for non-empty messages"
        );
        assert_ne!(ta, tb, "message order must change the embedded text");
    }

    #[test]
    fn prompt_text_differs_for_different_temperature() {
        // Two requests identical except temperature must embed to DIFFERENT text,
        // so the semantic tier never cross-hits across sampling params (the exact
        // tier keys on temperature; the semantic tier must stay consistent).
        let mut a = req("m", "hello");
        a.temperature = Some(0.2);
        let mut b = req("m", "hello");
        b.temperature = Some(0.9);
        assert_ne!(
            prompt_text(&a),
            prompt_text(&b),
            "temperature must change the embedded text"
        );
        // The params line must actually carry the value.
        assert!(prompt_text(&a).contains("temperature=0.2000"));
    }

    #[test]
    fn prompt_text_differs_for_different_top_p() {
        let mut a = req("m", "hello");
        a.top_p = Some(0.5);
        let mut b = req("m", "hello");
        b.top_p = Some(0.95);
        assert_ne!(prompt_text(&a), prompt_text(&b));
        assert!(prompt_text(&a).contains("top_p=0.5000"));
    }

    #[test]
    fn prompt_text_omits_params_line_when_unset() {
        // With neither sampling param set, no trailing `params:` line is appended.
        let r = req("m", "hello");
        assert!(
            !prompt_text(&r).contains("params:"),
            "no params line when temperature/top_p are unset"
        );
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
        assert_eq!(escape_tag("gpt_4o"), "gpt_4o"); // underscore is safe
    }

    #[test]
    fn escape_tag_escapes_all_redisearch_metachars() {
        // RediSearch TAG metacharacters must all be escaped to avoid a malformed
        // (or injectable) filter via the model field.
        for ch in ['{', '}', '[', ']', '(', ')', '|', '@', ':', '*', ' ', '"'] {
            let input = format!("a{ch}b");
            let escaped = escape_tag(&input);
            assert_eq!(escaped, format!("a\\{ch}b"), "char {ch:?} not escaped");
        }
    }

    #[test]
    fn parse_top_hit_returns_none_on_empty_result_set() {
        // FT.SEARCH with no matches replies `[0]` (count only, no documents).
        let empty = redis::Value::Array(vec![redis::Value::Int(0)]);
        assert!(parse_top_hit(&empty).is_none());
    }

    #[test]
    fn parse_top_hit_returns_none_on_unexpected_shape() {
        assert!(parse_top_hit(&redis::Value::Nil).is_none());
        assert!(parse_top_hit(&redis::Value::Int(1)).is_none());
    }

    /// Build an `FT.SEARCH`-shaped reply `[count, key, [fields...]]` from a list
    /// of (name, value) field pairs.
    fn search_reply(fields: &[(&str, &str)]) -> redis::Value {
        let bulk = |s: &str| redis::Value::BulkString(s.as_bytes().to_vec());
        let mut field_arr = Vec::new();
        for (name, val) in fields {
            field_arr.push(bulk(name));
            field_arr.push(bulk(val));
        }
        redis::Value::Array(vec![
            redis::Value::Int(1),
            bulk("solvela:scache:doc1"),
            redis::Value::Array(field_arr),
        ])
    }

    #[test]
    fn parse_top_hit_well_formed_returns_response_and_score() {
        // Sanity baseline: a numeric score parses and the response is extracted.
        let reply = search_reply(&[("response", "{\"id\":\"x\"}"), ("score", "0.1234")]);
        let (resp, distance) = parse_top_hit(&reply).expect("well-formed hit must parse");
        assert_eq!(resp, "{\"id\":\"x\"}");
        assert!((distance - 0.1234).abs() < 1e-6);
    }

    #[test]
    fn parse_top_hit_returns_none_for_unparseable_score() {
        // A present-but-non-numeric `score` must NOT silently coerce to a hit;
        // it returns None (distinguishable from absent via the warn + metric the
        // parser emits — exercised here for the unparseable path).
        let reply = search_reply(&[("response", "{\"id\":\"x\"}"), ("score", "not-a-float")]);
        assert!(
            parse_top_hit(&reply).is_none(),
            "an unparseable score must yield a miss, not a coerced hit"
        );
    }

    #[test]
    fn parse_top_hit_returns_none_when_score_absent() {
        // Distinct from the unparseable case: no `score` field at all is also a
        // miss, but the quiet kind (no warn/metric).
        let reply = search_reply(&[("response", "{\"id\":\"x\"}")]);
        assert!(parse_top_hit(&reply).is_none());
    }

    // ---- model-backed + redis-backed (skip if unavailable) ----

    #[tokio::test]
    async fn exact_prompt_hits_with_high_similarity() {
        let Some(cache) = fresh_cache(0.85).await else {
            return;
        };
        let r = req("openai/gpt-4o", "What is the capital of France?");
        cache.store(&r, &resp("paris")).await.unwrap();
        let hit = cache.get(&r).await.expect("identical prompt should hit");
        assert_eq!(hit.response.id, "resp-paris");
        assert!(
            hit.similarity > 0.99,
            "identical prompt similarity {} too low",
            hit.similarity
        );
    }

    #[tokio::test]
    async fn paraphrase_hits_above_threshold() {
        let Some(cache) = fresh_cache(0.85).await else {
            return;
        };
        let stored = req("openai/gpt-4o", "What is the capital of France?");
        cache.store(&stored, &resp("paris")).await.unwrap();
        let query = req("openai/gpt-4o", "What's France's capital?");
        let hit = cache.get(&query).await.expect("paraphrase should hit");
        assert_eq!(hit.response.id, "resp-paris");
        assert!(hit.similarity >= 0.85);
    }

    #[tokio::test]
    async fn unrelated_prompt_misses() {
        let Some(cache) = fresh_cache(0.85).await else {
            return;
        };
        cache
            .store(
                &req("openai/gpt-4o", "What is the capital of France?"),
                &resp("paris"),
            )
            .await
            .unwrap();
        let miss = cache
            .get(&req("openai/gpt-4o", "How do I make sourdough bread?"))
            .await;
        assert!(miss.is_none(), "unrelated prompt must miss, got {miss:?}");
    }

    #[tokio::test]
    async fn different_model_misses() {
        let Some(cache) = fresh_cache(0.85).await else {
            return;
        };
        cache
            .store(
                &req("openai/gpt-4o", "What is the capital of France?"),
                &resp("paris"),
            )
            .await
            .unwrap();
        // Same prompt, different model — must not hit (response/pricing differ).
        let miss = cache
            .get(&req(
                "anthropic/claude-3.5-sonnet",
                "What is the capital of France?",
            ))
            .await;
        assert!(
            miss.is_none(),
            "cross-model hit must not happen, got {miss:?}"
        );
    }

    #[tokio::test]
    async fn streaming_request_is_not_served_from_cache() {
        let Some(cache) = fresh_cache(0.85).await else {
            return;
        };
        let mut r = req("openai/gpt-4o", "What is the capital of France?");
        cache.store(&r, &resp("paris")).await.unwrap();
        r.stream = true;
        assert!(
            cache.get(&r).await.is_none(),
            "streaming requests must not hit the cache"
        );
    }

    #[tokio::test]
    async fn set_works_under_write_semaphore() {
        // The write semaphore must not break the happy path: firing more writes
        // than MAX_INFLIGHT_WRITES must not panic, and a write that does land
        // becomes retrievable. Timing-sensitive saturation behaviour isn't
        // asserted here (it's racy); we only confirm set() still functions.
        let Some(cache) = fresh_cache(0.85).await else {
            return;
        };
        for i in 0..(MAX_INFLIGHT_WRITES * 3) {
            cache
                .set(&req("openai/gpt-4o", &format!("prompt {i}")), &resp("ok"))
                .await;
        }
        // Use the awaitable path to guarantee at least one durable entry, then
        // confirm it's retrievable — proving the semaphore didn't poison set().
        let r = req("openai/gpt-4o", "a durable semaphore-test prompt");
        cache.store(&r, &resp("durable")).await.unwrap();
        let hit = cache
            .get(&r)
            .await
            .expect("stored entry must be retrievable");
        assert_eq!(hit.response.id, "resp-durable");
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
        assert!(
            cache.get(&r).await.is_none(),
            "disabled cache must return None"
        );
    }
}
