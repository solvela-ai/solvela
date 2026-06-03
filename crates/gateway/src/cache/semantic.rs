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
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

use solvela_protocol::{ChatRequest, ChatResponse};

use super::embedder::Embedder;

/// Redis key prefix for semantic cache entries (distinct from the exact tier's
/// `solvela:cache:` so the two never collide in the keyspace).
const SEMANTIC_PREFIX: &str = "solvela:scache";
/// RediSearch index name over the semantic entries.
///
/// **Versioned** (`_v2`): the schema gained `temperature`/`top_p` TAG fields so
/// sampling params hard-gate hits (a stored answer for `temperature=0` must
/// never be served for `temperature=2`). `ensure_index` short-circuits on an
/// existing index, so a schema change MUST bump this name — otherwise a
/// deployment that already created `_v1` would keep the old field-less schema
/// and the new TAG filters would match nothing. The old index/entries expire
/// via TTL and are simply never matched by the new query.
const SEMANTIC_INDEX: &str = "solvela_scache_idx_v2";

/// Upper bound on prompt characters we will embed. `bge-small-en-v1.5` silently
/// truncates inputs at 512 tokens; at the model's typical ~4 chars/token that's
/// roughly 2 KB, and we sit a hair below that at 2000 chars to leave headroom
/// for short tokens. A prompt past that limit embeds only its truncated prefix,
/// so two distinct long prompts that share a >512-token prefix (e.g. a long
/// RAG context with the real question last) collide — and the cache could
/// serve the *wrong* answer and bill for it. We refuse to cache or serve any
/// prompt above this budget; it falls through to a normal upstream call.
const MAX_EMBED_CHARS: usize = 2000;

/// Upper bound on concurrent fire-and-forget [`SemanticCache::set`] writes. Each
/// write runs a Mutex-serialised embedding (~75/s ceiling), so under a cache-miss
/// storm unbounded `tokio::spawn`s would pile up faster than they drain. We cap
/// in-flight writes here and drop the overflow — dropping a cache *write* (not a
/// response) is acceptable per the spirit of CLAUDE.md rule #9.
const MAX_INFLIGHT_WRITES: usize = 8;

/// Configuration for the semantic cache tier.
///
/// Vector dimension is intentionally NOT a field here: it is derived from
/// `embedder.dim()` inside [`SemanticCache::connect`]. A separate config-level
/// `dim` invited the failure mode where a caller passed a stale value (e.g.
/// from a previous embedder), `FT.CREATE` built the HNSW index at the wrong
/// width, and every subsequent `FT.SEARCH` failed at query time rather than
/// startup. Owning the dim on the embedder side makes that class structurally
/// impossible.
#[derive(Debug, Clone)]
pub struct SemanticConfig {
    /// Whether the semantic tier is enabled. Default off → zero behaviour change.
    pub enabled: bool,
    /// Minimum cosine similarity for a hit, in `[0, 1]`.
    pub threshold: f32,
    /// TTL (seconds) for stored semantic entries.
    pub ttl_secs: u64,
}

impl Default for SemanticConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold: 0.85,
            ttl_secs: 600,
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
    ///
    /// Vector dimension comes from `embedder.dim()` — there is no separate
    /// `config.dim` to drift from it.
    pub async fn connect(
        redis_url: &str,
        embedder: Arc<dyn Embedder>,
        config: SemanticConfig,
    ) -> Result<Self, super::CacheError> {
        // Fail fast on a bad threshold rather than degrade to a silent all-hit
        // (threshold ≤ 0: similarity ≥ 0 is always true → every query is a hit
        // → serves wrong cached response to paying agent) or all-miss
        // (threshold > 1) cache. Matches `SemanticSettings::validate` in
        // config.rs — the bounds MUST stay in sync; this guard exists because
        // `connect` is `pub` and can be called by tests or future code that
        // bypasses startup config validation.
        if !(config.threshold > 0.0 && config.threshold <= 1.0) {
            return Err(super::CacheError::Operation(format!(
                "semantic cache threshold {} is outside (0.0, 1.0]",
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
    ///
    /// **EAFP, not LBYL:** we skip the FT.INFO pre-check and rely on FT.CREATE's
    /// own "Index already exists" error. The check-then-create pattern races on
    /// simultaneous multi-replica startup against a fresh Redis — both replicas
    /// see FT.INFO miss, both call FT.CREATE, and the loser would silently run
    /// cache-disabled for its whole lifetime because the bubbled-up error
    /// triggers `build_semantic_cache` to return `None`. Treating the
    /// already-exists error as success closes that window.
    pub async fn ensure_index(&self) -> Result<(), super::CacheError> {
        let mut conn = self.conn.clone();
        let result: redis::RedisResult<()> = redis::cmd("FT.CREATE")
            .arg(SEMANTIC_INDEX)
            .arg("ON")
            .arg("HASH")
            .arg("PREFIX")
            .arg(1)
            .arg(format!("{SEMANTIC_PREFIX}:"))
            .arg("SCHEMA")
            .arg("model")
            .arg("TAG")
            // Sampling params are exact-match TAG predicates (see `param_tag`),
            // hard-gating hits so an answer sampled at one temperature/top_p is
            // never served for a request at another.
            .arg("temperature")
            .arg("TAG")
            .arg("top_p")
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
            .arg(self.embedder.dim() as i64)
            .arg("DISTANCE_METRIC")
            .arg("COSINE")
            .query_async(&mut conn)
            .await;
        match result {
            Ok(()) => Ok(()),
            // RediSearch returns "Index already exists" when the index is
            // present. The phrasing has drifted slightly across redis-stack
            // versions (capitalisation/whitespace), so we lowercase + check
            // the stable substring "already exists" — robust without false
            // positives, since no other FT.CREATE failure carries that
            // phrase. Anything else is a real failure (auth, OOM, unreachable
            // module, schema-incompatible re-create attempt) and propagates.
            Err(e) if e.to_string().to_lowercase().contains("already exists") => Ok(()),
            Err(e) => Err(super::CacheError::Operation(e.to_string())),
        }
    }

    /// Look up a semantically-similar cached response for this request.
    /// Returns `None` on miss, when disabled, or for streaming requests.
    pub async fn get(&self, req: &ChatRequest) -> Option<SemanticHit> {
        if !self.config.enabled || req.stream {
            return None;
        }

        let embedding = self.embed_checked(req).await?;
        let bytes = f32_slice_to_le_bytes(&embedding);

        // Hybrid query: filter by model + sampling-param TAGs, then KNN over the
        // embedding. The TAG predicates hard-gate the candidate set so a hit can
        // only come from the same model AND the same temperature/top_p — the
        // KNN never sees an entry from a different sampling regime.
        let query = format!(
            "(@model:{{{model}}} @temperature:{{{temp}}} @top_p:{{{top_p}}})\
             =>[KNN 1 @embedding $vec AS score]",
            model = escape_tag(&req.model),
            temp = escape_tag(&param_tag(req.temperature)),
            top_p = escape_tag(&param_tag(req.top_p)),
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
        // Mirror the exact-tier write guard (`cache/exact.rs::set`): refusing
        // to cache a usage-less response. The downstream
        // `semantic_hit_full_atomic` fallback estimates a cost on a usage-less
        // hit so the spend ledger can still settle, but every usage-less entry
        // accepted into this tier is a request that will bill the *estimate*
        // (typically the input bound + a 1000-token output assumption) rather
        // than the true token cost. The exact tier already refuses these to
        // keep the `log_spend` reconciliation honest; the semantic tier was
        // asymmetrically permissive, and a later semantic hit on such an
        // entry routes through the lowest-tested billing branch
        // (`mod.rs:743 +/- 30`). Skip-and-counter is fail-closed: the response
        // is still served, but the wallet does not store a future-billable
        // entry whose cost can only be estimated, not measured.
        if response.usage.is_none() {
            metrics::counter!("solvela_semantic_cache_write_skipped_no_usage_total").increment(1);
            debug!(
                model = %req.model,
                "semantic cache write skipped: response has no usage block; \
                 a future hit would bill the request's input estimate \
                 instead of the cached response's true token cost"
            );
            return;
        }
        // Don't store a prompt BGE would truncate (bug_002): its embedding would
        // reflect only the truncated prefix, risking a later wrong cross-hit.
        let text = prompt_text(req);
        if !within_embed_budget(&text) {
            debug!(
                model = %req.model,
                chars = text.chars().count(),
                "semantic cache write skipped: prompt exceeds embed budget"
            );
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
        let model = req.model.clone();
        let temperature = param_tag(req.temperature);
        let top_p = param_tag(req.top_p);
        let conn = self.conn.clone();
        let ttl = self.config.ttl_secs;
        tokio::spawn(async move {
            // Hold the permit for the task's lifetime; released on drop.
            let _permit = permit;
            let embedding = match embed_text(&embedder, text).await {
                Some(e) => e,
                None => return,
            };
            if let Err(e) =
                write_entry(conn, model, temperature, top_p, json, &embedding, ttl).await
            {
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
        let embedding = self.embed_checked(req).await.ok_or_else(|| {
            super::CacheError::Operation(
                "embedding skipped (oversized prompt) or failed".to_string(),
            )
        })?;
        write_entry(
            self.conn.clone(),
            req.model.clone(),
            param_tag(req.temperature),
            param_tag(req.top_p),
            json,
            &embedding,
            self.config.ttl_secs,
        )
        .await
        .map_err(|e| super::CacheError::Operation(e.to_string()))
    }

    /// Embed a request's canonical prompt off the async runtime threads (the
    /// model call is CPU-bound and Mutex-serialised). Returns `None` — a miss —
    /// when the prompt exceeds [`MAX_EMBED_CHARS`] (BGE would truncate it; see
    /// bug_002) or when embedding fails.
    async fn embed_checked(&self, req: &ChatRequest) -> Option<Vec<f32>> {
        let text = prompt_text(req);
        if !within_embed_budget(&text) {
            debug!(
                model = %req.model,
                chars = text.chars().count(),
                "semantic cache skip: prompt exceeds embed budget"
            );
            return None;
        }
        embed_text(&self.embedder, text).await
    }
}

/// Embed an owned prompt string on a blocking thread (CPU-bound, Mutex-serialised).
///
/// Increments distinct counters for the three failure modes so alerting can
/// separate them: `mutex_poisoned` is a permanent process-state corruption
/// (restart needed); `embed` is a transient runtime error; `task_panic` is a
/// `spawn_blocking` join failure (also indicates panic, but distinct from the
/// poison-recovery case which is on a subsequent call).
async fn embed_text(embedder: &Arc<dyn Embedder>, text: String) -> Option<Vec<f32>> {
    let embedder = Arc::clone(embedder);
    match tokio::task::spawn_blocking(move || embedder.embed_one(&text)).await {
        Ok(Ok(v)) => Some(v),
        Ok(Err(crate::cache::embedder::EmbedderError::MutexPoisoned)) => {
            metrics::counter!(
                "solvela_semantic_cache_embed_failure_total",
                "reason" => "mutex_poisoned"
            )
            .increment(1);
            // `error!`, not `warn!`: poison is permanent for the process.
            tracing::error!(
                "semantic cache embedder mutex poisoned — process needs restart \
                 to recover the model state"
            );
            None
        }
        Ok(Err(e)) => {
            metrics::counter!(
                "solvela_semantic_cache_embed_failure_total",
                "reason" => "embed"
            )
            .increment(1);
            warn!(error = %e, "semantic cache embedding failed");
            None
        }
        Err(e) => {
            metrics::counter!(
                "solvela_semantic_cache_embed_failure_total",
                "reason" => "task_panic"
            )
            .increment(1);
            // `error!`, not `warn!`: a panic inside the spawn_blocking task
            // means fastembed/ONNX state is suspect under load — at least as
            // severe as the startup `task_panic` arm in `build_semantic_cache`
            // (main.rs), which also logs `error!`. Operators reading logs
            // during an incident must see this at the same severity.
            tracing::error!(error = %e, "semantic cache embed task panicked");
            None
        }
    }
}

/// Build a deterministic, content-derived Redis key for a semantic-cache entry.
///
/// Hashing `model ‖ temperature ‖ top_p ‖ embedding` makes the write idempotent:
/// retries, fire-and-forget races, and storm-time duplicate writes all collapse
/// onto the same key, so `HSET` becomes an upsert and `EXPIRE` refreshes the TTL
/// without accreting near-duplicate HNSW nodes (index bloat → memory + KNN
/// latency over long-running deploys). Fields are null-separated to avoid
/// collisions between e.g. `model="a"` and a 1-char-shorter neighbour.
fn content_key(model: &str, temperature: &str, top_p: &str, embedding_bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(model.as_bytes());
    hasher.update(b"\0");
    hasher.update(temperature.as_bytes());
    hasher.update(b"\0");
    hasher.update(top_p.as_bytes());
    hasher.update(b"\0");
    hasher.update(embedding_bytes);
    format!("{SEMANTIC_PREFIX}:{}", hex::encode(hasher.finalize()))
}

/// HSET the entry then set its TTL (HSET takes no EX). Returns once durable.
async fn write_entry(
    mut conn: ConnectionManager,
    model: String,
    temperature: String,
    top_p: String,
    response_json: String,
    embedding: &[f32],
    ttl_secs: u64,
) -> redis::RedisResult<()> {
    let bytes = f32_slice_to_le_bytes(embedding);
    let key = content_key(&model, &temperature, &top_p, &bytes);
    // HSET + EXPIRE in one MULTI/EXEC: HSET takes no EX, and applying them
    // separately risks orphaning an entry with no TTL (indexed forever) if the
    // second round-trip fails. The pipeline makes the pair atomic.
    let mut pipe = redis::pipe();
    pipe.atomic()
        .cmd("HSET")
        .arg(&key)
        .arg("model")
        .arg(&model)
        .arg("temperature")
        .arg(&temperature)
        .arg("top_p")
        .arg(&top_p)
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
/// Sampling params (`temperature` / `top_p`) are **not** embedded here. An
/// earlier attempt appended them as a trailing `params:` line, but that does
/// not separate hits: two prompts identical except for temperature embed to
/// near-identical vectors (cosine ≈ 0.99 ≫ the 0.85 threshold), so the param
/// line is invisible to the KNN gate and the wrong sampling regime gets served.
/// Sampling params are instead enforced as exact RediSearch TAG predicates (see
/// [`param_tag`] and the `@temperature` / `@top_p` filters in [`SemanticCache::get`]).
pub(crate) fn prompt_text(req: &ChatRequest) -> String {
    req.messages
        .iter()
        .map(|m| {
            // Base line: `role: content`. For an assistant turn carrying
            // `tool_calls`, or a tool turn carrying `tool_call_id`, we append
            // a deterministic suffix so two conversation histories that
            // differ only in their tool-call payloads embed to distinct
            // vectors. Without this, an assistant turn `tool_calls=[get_weather("NYC")]`
            // and `tool_calls=[get_weather("Boston")]` produce identical
            // `prompt_text` (content is typically empty on tool-call turns)
            // and collide above the similarity threshold — wrong tool result
            // served from cache. The suffix is JSON to preserve structure;
            // missing fields are simply omitted (no `null` noise).
            let base = format!("{}: {}", role_str(&m.role), m.content.as_text());
            let tool_calls_suffix = m
                .tool_calls
                .as_ref()
                .and_then(|tc| serde_json::to_string(tc).ok())
                .filter(|s| s != "null")
                .map(|s| format!(" tool_calls={s}"))
                .unwrap_or_default();
            let tool_call_id_suffix = m
                .tool_call_id
                .as_ref()
                .map(|id| format!(" tool_call_id={id}"))
                .unwrap_or_default();
            format!("{base}{tool_calls_suffix}{tool_call_id_suffix}")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Whether a prompt is short enough to embed without BGE silently truncating it
/// (see [`MAX_EMBED_CHARS`]). Counts `chars`, not bytes, to match the
/// token-budget intent.
fn within_embed_budget(text: &str) -> bool {
    text.chars().count() <= MAX_EMBED_CHARS
}

/// Canonical RediSearch TAG value for a sampling parameter.
///
/// `None` (unset) and any set value map to distinct, stable strings, so the
/// semantic tier hard-gates hits on the sampling regime — mirroring the exact
/// tier, which keys on `temperature`. Set values are formatted to 4 decimal
/// places for stable, collision-free string matching; an unset param is the
/// literal `none`. The value is stored verbatim in the entry hash; the query
/// side escapes it via [`escape_tag`] (the `.` in a float is TAG-special).
fn param_tag(value: Option<f32>) -> String {
    match value {
        Some(v) => format!("{v:.4}"),
        None => "none".to_string(),
    }
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
    use crate::cache::embedder::{Embedder, EmbedderError, LocalBge, BGE_SMALL_DIM};
    use solvela_protocol::{ChatMessage, Role, Usage};
    use std::path::PathBuf;

    /// Embedder that always returns `Err` — used to verify `get`/`set` treat
    /// embedder failure as a miss + dropped write, never as a panic or a
    /// corrupt-hit. Closes the test gap that no test installed a deliberately
    /// failing embedder behind a real `SemanticCache`.
    struct FailingEmbedder;
    impl Embedder for FailingEmbedder {
        fn embed_one(&self, _text: &str) -> Result<Vec<f32>, EmbedderError> {
            Err(EmbedderError::Embed("forced failure".to_string()))
        }
        fn embed_batch(&self, _texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedderError> {
            Err(EmbedderError::Embed("forced failure".to_string()))
        }
        fn dim(&self) -> usize {
            BGE_SMALL_DIM
        }
    }

    fn model_cache_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.fastembed_cache")
    }

    fn redis_url() -> String {
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379/".to_string())
    }

    fn user_msg(content: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: content.into(),
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

    /// Serialises the redis-backed tests. They share one index name and key
    /// prefix, and [`fresh_cache`] drops + recreates the index on entry, so
    /// under cargo's default parallelism one test's `FT.DROPINDEX ... DD` can
    /// wipe another test's just-stored entries mid-run. Each redis test holds
    /// this lock (handed back by `fresh_cache`) for its whole body; the pure
    /// no-infra tests never touch it.
    fn semantic_test_lock() -> Arc<tokio::sync::Mutex<()>> {
        static LOCK: std::sync::OnceLock<Arc<tokio::sync::Mutex<()>>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// Build a SemanticCache against a fresh index, or skip if model/redis are
    /// unavailable (mirrors the redis-gated + model-gated patterns elsewhere).
    ///
    /// Returns the cache **and** a held lock guard: callers bind the guard
    /// (`let Some((cache, _guard)) = fresh_cache(..)`) so the shared index is
    /// owned exclusively until the test ends, regardless of `--test-threads`.
    async fn fresh_cache(
        threshold: f32,
    ) -> Option<(SemanticCache, tokio::sync::OwnedMutexGuard<()>)> {
        // Acquire BEFORE touching the shared index so the drop/recreate and the
        // test body are one critical section.
        let guard = semantic_test_lock().lock_owned().await;
        let embedder = LocalBge::with_cache_dir(model_cache_dir()).ok()?;
        let config = SemanticConfig {
            enabled: true,
            threshold,
            ttl_secs: 600,
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
        Some((cache, guard))
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
    fn prompt_text_ignores_sampling_params() {
        // Sampling params are NO LONGER embedded — they are enforced as exact
        // RediSearch TAG predicates instead (the embedded-suffix approach could
        // not separate near-identical vectors under the cosine threshold). Two
        // requests differing only in temperature/top_p must embed to the SAME
        // text; separation happens at the TAG layer, asserted behaviourally in
        // `different_temperature_misses`.
        let mut a = req("m", "hello");
        a.temperature = Some(0.2);
        a.top_p = Some(0.5);
        let mut b = req("m", "hello");
        b.temperature = Some(0.9);
        b.top_p = Some(0.95);
        assert_eq!(
            prompt_text(&a),
            prompt_text(&b),
            "sampling params must NOT change the embedded text"
        );
        assert!(!prompt_text(&a).contains("temperature"));
        assert!(!prompt_text(&a).contains("top_p"));
    }

    /// Two conversation histories differing only in their assistant
    /// `tool_calls` payloads must embed to distinct prompt texts; without this
    /// the embeddings collide above the similarity threshold and a tool-call
    /// for `get_weather("NYC")` can serve a cached completion that called
    /// `get_weather("Boston")` — wrong tool result returned to the agent.
    /// Falsifiability: removing the tool-calls suffix from `prompt_text` would
    /// make this test fail (the two texts collapse to `"assistant: "` lines).
    #[test]
    fn prompt_text_distinguishes_tool_calls() {
        use solvela_protocol::{FunctionCall, ToolCall};
        let make = |args: &str| {
            let mut r = req("m", "what's the weather?");
            r.messages.push(ChatMessage {
                role: Role::Assistant,
                content: solvela_protocol::MessageContent::Text(String::new()),
                name: None,
                tool_calls: Some(vec![ToolCall {
                    id: "call_1".to_string(),
                    r#type: "function".to_string(),
                    function: FunctionCall {
                        name: "get_weather".to_string(),
                        arguments: args.to_string(),
                    },
                }]),
                tool_call_id: None,
            });
            r
        };
        let nyc = make(r#"{"location":"NYC"}"#);
        let boston = make(r#"{"location":"Boston"}"#);
        assert_ne!(
            prompt_text(&nyc),
            prompt_text(&boston),
            "tool-call arguments must change the embedded text; otherwise a \
             cached response for NYC could serve a request for Boston"
        );
        // A message with no tool_calls must NOT carry a suffix (regression
        // guard against accidentally embedding `null`/empty markers that would
        // shift the vector of every plain conversation).
        let mut plain = req("m", "what's the weather?");
        plain.messages.push(ChatMessage {
            role: Role::Assistant,
            content: "It's sunny.".into(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        });
        let text = prompt_text(&plain);
        assert!(
            !text.contains("tool_calls"),
            "plain (no tool_calls) message must not embed a tool_calls suffix; got: {text:?}"
        );
        assert!(
            !text.contains("tool_call_id"),
            "plain (no tool_call_id) message must not embed a tool_call_id suffix; got: {text:?}"
        );
    }

    #[test]
    fn param_tag_is_stable_and_separates_values() {
        // Unset → "none"; set → 4-dp formatted, distinct per value.
        assert_eq!(param_tag(None), "none");
        assert_eq!(param_tag(Some(0.0)), "0.0000");
        assert_eq!(param_tag(Some(0.2)), "0.2000");
        assert_ne!(param_tag(Some(0.2)), param_tag(Some(0.9)));
        // Unset and a set value must never collide.
        assert_ne!(param_tag(None), param_tag(Some(0.0)));
    }

    #[test]
    fn within_embed_budget_guards_oversized_prompts() {
        assert!(within_embed_budget("short prompt"));
        assert!(within_embed_budget(&"x".repeat(MAX_EMBED_CHARS)));
        assert!(!within_embed_budget(&"x".repeat(MAX_EMBED_CHARS + 1)));
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
        let Some((cache, _guard)) = fresh_cache(0.85).await else {
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
        let Some((cache, _guard)) = fresh_cache(0.85).await else {
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
        let Some((cache, _guard)) = fresh_cache(0.85).await else {
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
        let Some((cache, _guard)) = fresh_cache(0.85).await else {
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
    async fn different_temperature_misses() {
        // Behavioural regression guard: an earlier attempt appended sampling
        // params as a trailing line in the embedded text, which only changed
        // the string and not the cosine score (the suffix is invisible to the
        // gate), so a hit at a different temperature was still served. This
        // test drives the REAL lookup: store at temperature 0.0, query the
        // identical prompt at temperature 2.0, and require a MISS. It fails
        // if the temperature TAG predicate is dropped.
        let Some((cache, _guard)) = fresh_cache(0.85).await else {
            return;
        };
        let mut stored = req("openai/gpt-4o", "What is the capital of France?");
        stored.temperature = Some(0.0);
        cache.store(&stored, &resp("paris")).await.unwrap();

        let mut query = req("openai/gpt-4o", "What is the capital of France?");
        query.temperature = Some(2.0);
        let miss = cache.get(&query).await;
        assert!(
            miss.is_none(),
            "a hit at a different temperature must MISS, got {miss:?}"
        );

        // Sanity: the same temperature still hits.
        let hit = cache
            .get(&stored)
            .await
            .expect("same temperature must still hit");
        assert_eq!(hit.response.id, "resp-paris");
    }

    #[tokio::test]
    async fn different_top_p_misses() {
        let Some((cache, _guard)) = fresh_cache(0.85).await else {
            return;
        };
        let mut stored = req("openai/gpt-4o", "What is the capital of France?");
        stored.top_p = Some(0.1);
        cache.store(&stored, &resp("paris")).await.unwrap();

        let mut query = req("openai/gpt-4o", "What is the capital of France?");
        query.top_p = Some(0.99);
        assert!(
            cache.get(&query).await.is_none(),
            "a hit at a different top_p must MISS"
        );
    }

    #[tokio::test]
    async fn oversized_prompt_is_not_cached_or_served() {
        // bug_002: a prompt past the embed budget must neither be stored nor
        // matched — it always falls through to a normal upstream call so BGE
        // truncation can never serve a wrong cross-hit.
        let Some((cache, _guard)) = fresh_cache(0.85).await else {
            return;
        };
        let huge = "a ".repeat(MAX_EMBED_CHARS); // ~2x the char budget
        let r = req("openai/gpt-4o", &huge);
        // store() refuses an oversized prompt rather than embedding a truncation.
        assert!(
            cache.store(&r, &resp("huge")).await.is_err(),
            "store must reject an oversized prompt"
        );
        // And a lookup of the same oversized prompt is a guaranteed miss.
        assert!(
            cache.get(&r).await.is_none(),
            "oversized prompt lookup must miss"
        );
    }

    #[tokio::test]
    async fn streaming_request_is_not_served_from_cache() {
        let Some((cache, _guard)) = fresh_cache(0.85).await else {
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

    use crate::cache::test_metrics::{counter_value, install_test_recorder};

    /// Mirror of `exact::set_refuses_to_cache_response_without_usage`: the
    /// semantic tier must refuse to cache a `usage: None` response, parallel
    /// to the exact tier. Without this guard the spend ledger on a later
    /// usage-less hit routes through the lowest-tested billing branch
    /// (`mod.rs:743` ± 30), billing the request's input estimate instead of
    /// the cached response's true token cost.
    ///
    /// Falsifiability: asserts `solvela_semantic_cache_write_skipped_no_usage_total`
    /// incremented. Removing the guard would let execution continue past the
    /// counter call into the (embed + Redis) path — the counter would not
    /// fire and this test would fail.
    #[tokio::test]
    async fn set_refuses_to_cache_response_without_usage() {
        let Some((cache, _guard)) = fresh_cache(0.85).await else {
            return;
        };
        let handle = install_test_recorder();
        let before = counter_value(
            &handle,
            "solvela_semantic_cache_write_skipped_no_usage_total",
        );

        let r = req("openai/gpt-4o", "usage-less guard probe");
        let usage_less = ChatResponse {
            id: "no-usage".to_string(),
            object: "chat.completion".to_string(),
            created: 0,
            model: "test".to_string(),
            choices: vec![],
            usage: None,
        };
        cache.set(&r, &usage_less).await;

        let after = counter_value(
            &handle,
            "solvela_semantic_cache_write_skipped_no_usage_total",
        );
        assert_eq!(
            after,
            before + 1,
            "semantic tier must refuse to cache usage-less responses; \
             without the guard a later hit would bill the wallet's input \
             estimate rather than the response's true cost"
        );
        // And the entry must not be retrievable — the guard short-circuits
        // before embedding/writing.
        assert!(
            cache.get(&r).await.is_none(),
            "guarded write must not produce a retrievable entry"
        );
    }

    #[tokio::test]
    async fn set_works_under_write_semaphore() {
        // The write semaphore must not break the happy path: firing more writes
        // than MAX_INFLIGHT_WRITES must not panic, and a write that does land
        // becomes retrievable. Timing-sensitive saturation behaviour isn't
        // asserted here (it's racy); we only confirm set() still functions.
        let Some((cache, _guard)) = fresh_cache(0.85).await else {
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

    /// Failing-embedder behavior at the `SemanticCache` layer: `get` must
    /// return `None` (miss → fall through to provider) and `store` must
    /// return `Err` — never panic, never return a corrupt-hit. This is the
    /// general `EmbedderError` propagation contract.
    ///
    /// SCOPE: this test does NOT exercise the F2 fix's actual Mutex-poison
    /// recovery path (a `FailingEmbedder` never holds a Mutex). For that, see
    /// `cache::embedder::tests::mutex_poison_in_localbge_returns_mutex_poisoned_variant`
    /// which pokes a real thread panic to poison a `LocalBge`'s lock and
    /// asserts the dedicated `EmbedderError::MutexPoisoned` variant.
    ///
    /// This test also intentionally does NOT exercise the fire-and-forget
    /// `set` path — only the synchronous `store` and `get` entry points.
    /// Holds the shared-index lock because `connect` calls `ensure_index`.
    #[tokio::test]
    async fn failing_embedder_yields_miss_not_panic() {
        // Honour the shared-redis-index lock (mirror of fresh_cache's pattern).
        let _guard = semantic_test_lock().lock_owned().await;
        let config = SemanticConfig {
            enabled: true,
            threshold: 0.85,
            ttl_secs: 600,
        };
        let cache =
            match SemanticCache::connect(&redis_url(), Arc::new(FailingEmbedder), config).await {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("skipping: redis-stack unreachable ({e})");
                    return;
                }
            };
        let r = req("openai/gpt-4o", "What is the capital of France?");

        // `store` MUST surface the embedder failure as Err — the documented
        // contract that callers (the provider write-back path) use to count
        // dropped writes and avoid cache pollution.
        let store_result = cache.store(&r, &resp("paris")).await;
        assert!(
            store_result.is_err(),
            "store with a failing embedder must return Err, not silently succeed"
        );

        // `get` MUST return None on embedder failure — fall through to upstream
        // is the documented behavior, NOT a panic or a corrupt SemanticHit.
        let hit = cache.get(&r).await;
        assert!(
            hit.is_none(),
            "get with a failing embedder must return None (miss), not a corrupt hit"
        );
    }
}
