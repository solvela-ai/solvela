//! Prompt embedding for the semantic cache tier.
//!
//! The [`Embedder`] trait abstracts the embedding backend so the gateway can
//! swap the local model for an API-backed one (e.g. OpenAI `text-embedding-3-small`)
//! without touching the cache logic. The default [`LocalBge`] runs
//! `bge-small-en-v1.5` (33M params, 384-dim) locally via the `fastembed` crate
//! (ONNX runtime) — no per-request API cost, ~13 ms p50 on CPU.
//!
//! BGE outputs are L2-normalized so cosine reduces to a dot product, but
//! [`cosine`] does the full computation defensively: if the model is ever
//! swapped for a non-normalized one, callers still get a valid similarity.
//!
//! `fastembed`'s `embed` takes `&mut self`, so [`LocalBge`] wraps the model in a
//! `Mutex` to keep the trait `&self`-shareable behind `Arc<dyn Embedder>`. Lock
//! contention is negligible against the embedding cost; a model pool is a future
//! optimisation (see the plan's Risk #9 — ~75 emb/sec ceiling per process).

use std::path::PathBuf;
use std::sync::Mutex;

use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};

/// Embedding dimension of `bge-small-en-v1.5`.
pub const BGE_SMALL_DIM: usize = 384;

/// Errors from constructing or running an embedder.
#[derive(Debug, thiserror::Error)]
pub enum EmbedderError {
    /// Model initialisation failed (download, missing cache, corrupt model).
    #[error("embedder init failed: {0}")]
    Init(String),
    /// An embedding call failed at runtime.
    #[error("embedding failed: {0}")]
    Embed(String),
}

/// Abstraction over an embedding backend. `Send + Sync` so it can live in
/// `AppState` behind `Arc<dyn Embedder>`.
pub trait Embedder: Send + Sync {
    /// Embed a single text into a dense vector.
    fn embed_one(&self, text: &str) -> Result<Vec<f32>, EmbedderError>;
    /// Embed many texts in one batch (cheaper than N `embed_one` calls).
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedderError>;
    /// Output dimension of this embedder's vectors.
    fn dim(&self) -> usize;
}

/// Local `bge-small-en-v1.5` embedder backed by `fastembed`.
pub struct LocalBge {
    model: Mutex<TextEmbedding>,
    dim: usize,
}

impl LocalBge {
    /// Initialise the model using `fastembed`'s default cache directory
    /// (`./.fastembed_cache` relative to the process working directory). The
    /// first call downloads ~133 MB; subsequent constructions reuse the cache.
    pub fn new() -> Result<Self, EmbedderError> {
        Self::init(InitOptions::new(EmbeddingModel::BGESmallENV15).with_show_download_progress(false))
    }

    /// Initialise the model from an explicit cache directory. Used by tests
    /// (to point at a pre-downloaded model) and by operators who want to pin
    /// the model location rather than rely on the process working directory.
    pub fn with_cache_dir(dir: impl Into<PathBuf>) -> Result<Self, EmbedderError> {
        Self::init(
            InitOptions::new(EmbeddingModel::BGESmallENV15)
                .with_show_download_progress(false)
                .with_cache_dir(dir.into()),
        )
    }

    fn init(opts: InitOptions) -> Result<Self, EmbedderError> {
        let model = TextEmbedding::try_new(opts).map_err(|e| EmbedderError::Init(e.to_string()))?;
        Ok(Self {
            model: Mutex::new(model),
            dim: BGE_SMALL_DIM,
        })
    }
}

impl Embedder for LocalBge {
    fn embed_one(&self, text: &str) -> Result<Vec<f32>, EmbedderError> {
        let mut model = self
            .model
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut out = model
            .embed(vec![text.to_string()], None)
            .map_err(|e| EmbedderError::Embed(e.to_string()))?;
        out.pop()
            .ok_or_else(|| EmbedderError::Embed("fastembed returned no vectors".to_string()))
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, EmbedderError> {
        let mut model = self
            .model
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let owned: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
        model
            .embed(owned, None)
            .map_err(|e| EmbedderError::Embed(e.to_string()))
    }

    fn dim(&self) -> usize {
        self.dim
    }
}

/// Cosine similarity between two equal-length vectors. Returns a value in
/// `[-1, 1]`; higher means more similar. Returns `0.0` if either vector has
/// zero magnitude (avoids NaN from division by zero).
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    debug_assert_eq!(a.len(), b.len(), "embedding dimension mismatch");
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    let denom = na.sqrt() * nb.sqrt();
    if denom == 0.0 {
        return 0.0;
    }
    dot / denom
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Instant;

    /// Workspace-root `.fastembed_cache` (cargo runs tests with CWD = crate dir,
    /// so we resolve relative to the manifest dir to find the shared model).
    fn model_cache_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.fastembed_cache")
    }

    /// Build an embedder, or skip the test if the model can't be loaded (no
    /// cached model and no network). Mirrors the redis-gated pattern used by
    /// the semantic store tests — model-dependent tests must not fail CI envs
    /// that lack the ~133 MB model, but must really run where it's available.
    fn embedder() -> Option<LocalBge> {
        match LocalBge::with_cache_dir(model_cache_dir()) {
            Ok(e) => Some(e),
            Err(e) => {
                eprintln!(
                    "skipping: LocalBge init failed ({e}). Cache the bge-small model \
                     under .fastembed_cache/ or allow network for fastembed to download it."
                );
                None
            }
        }
    }

    // ---- pure cosine (no model needed) ----

    #[test]
    fn cosine_identical_unit_vectors_is_one() {
        let v = vec![0.6, 0.8]; // unit length
        assert!((cosine(&v, &v) - 1.0).abs() < 1e-6, "got {}", cosine(&v, &v));
    }

    #[test]
    fn cosine_orthogonal_is_zero() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        assert!(cosine(&a, &b).abs() < 1e-6, "got {}", cosine(&a, &b));
    }

    #[test]
    fn cosine_opposite_is_negative_one() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        assert!((cosine(&a, &b) + 1.0).abs() < 1e-6, "got {}", cosine(&a, &b));
    }

    #[test]
    fn cosine_zero_magnitude_returns_zero_not_nan() {
        let a = vec![0.0, 0.0];
        let b = vec![1.0, 1.0];
        let s = cosine(&a, &b);
        assert!(s.is_finite(), "cosine with zero vector must not be NaN/inf");
        assert_eq!(s, 0.0);
    }

    // ---- model-backed (skip if model unavailable) ----

    #[test]
    fn dim_is_384() {
        let Some(e) = embedder() else { return };
        assert_eq!(e.dim(), BGE_SMALL_DIM);
        let v = e.embed_one("hello").unwrap();
        assert_eq!(v.len(), BGE_SMALL_DIM);
    }

    #[test]
    fn deterministic() {
        let Some(e) = embedder() else { return };
        let v1 = e.embed_one("what is the capital of France?").unwrap();
        let v2 = e.embed_one("what is the capital of France?").unwrap();
        assert_eq!(v1, v2, "identical input must produce identical embedding");
    }

    #[test]
    fn paraphrase_similarity_high() {
        let Some(e) = embedder() else { return };
        let v1 = e.embed_one("What is the capital of France?").unwrap();
        let v2 = e.embed_one("What's France's capital?").unwrap();
        let sim = cosine(&v1, &v2);
        assert!(sim > 0.80, "paraphrase cosine too low: {sim}");
    }

    #[test]
    fn unrelated_similarity_low() {
        let Some(e) = embedder() else { return };
        let v1 = e.embed_one("What is the capital of France?").unwrap();
        let v2 = e.embed_one("How do I make sourdough bread?").unwrap();
        let sim = cosine(&v1, &v2);
        assert!(sim < 0.65, "unrelated cosine too high: {sim}");
    }

    #[test]
    fn batch_matches_one_by_one() {
        let Some(e) = embedder() else { return };
        let texts = ["hello world", "the capital of Spain is Madrid"];
        let batch = e.embed_batch(&texts).unwrap();
        let one = e.embed_one(texts[0]).unwrap();
        let two = e.embed_one(texts[1]).unwrap();
        assert_eq!(batch.len(), 2);
        assert_eq!(batch[0], one, "batch[0] != embed_one");
        assert_eq!(batch[1], two, "batch[1] != embed_one");
    }

    #[test]
    fn latency_p50_under_50ms() {
        let Some(e) = embedder() else { return };
        let _ = e.embed_one("warmup");
        let n = 20;
        let mut samples = Vec::with_capacity(n);
        for i in 0..n {
            let prompt = format!("test prompt number {i}: explain a concept briefly");
            let t = Instant::now();
            let _ = e.embed_one(&prompt).unwrap();
            samples.push(t.elapsed());
        }
        samples.sort();
        let p50 = samples[n / 2];
        assert!(p50.as_millis() < 50, "p50 latency too high: {p50:?}");
    }
}
