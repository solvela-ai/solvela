//! Web-search tool provider adapters.
//!
//! Mirrors the `LLMProvider` pattern in [`crate::providers`]: a swappable
//! trait ([`SearchProvider`]) with concrete upstream adapters, registered only
//! when the upstream's API key is present in the environment. The gateway holds
//! the upstream key and calls the search API server-side; clients never see it.
//!
//! The normalized response shape ([`SearchResults`] / [`SearchResult`]) is the
//! stable contract returned to callers — adding Exa/Parallel later must map
//! into the same shape so existing callers do not break.

pub mod tavily;

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Error type for search-provider operations.
///
/// A small typed enum (rather than a boxed trait object) so the route handler
/// and future adapters can branch on the failure class. The handler maps every
/// variant to the same opaque 502 today (it must not leak upstream internals —
/// GHSA-cgqx-mg48-949v), but the typing lets a later change distinguish e.g. a
/// 4xx upstream `Status` from a transport failure without parsing strings.
#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    /// Transport-level failure reaching the upstream (DNS, connect, timeout).
    #[error("search upstream request failed: {0}")]
    Upstream(#[from] reqwest::Error),

    /// The upstream returned a non-success HTTP status. Carries only the numeric
    /// code — never the upstream body (which may echo the request or carry
    /// provider internals).
    #[error("search upstream returned status {0}")]
    Status(u16),

    /// The upstream response could not be parsed into the expected shape.
    #[error("search upstream response parse error: {0}")]
    Parse(String),
}

/// A single normalized search result.
///
/// Deliberately minimal and provider-agnostic: `title`, `url`, `snippet`.
/// Upstream-specific fields (Tavily `score`, `raw_content`, …) are dropped at
/// the adapter boundary so callers depend only on this stable shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResult {
    /// Human-readable title of the result.
    pub title: String,
    /// Canonical URL of the result.
    pub url: String,
    /// Short text snippet / description of the result.
    pub snippet: String,
}

/// Normalized web-search response returned by `POST /v1/search`.
///
/// Stable across providers: callers read `query` + `results` regardless of
/// which upstream served the request. The `provider` field is informational
/// (which adapter answered), not a contract callers should branch on.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchResults {
    /// The query that was executed (echoed from the request).
    pub query: String,
    /// The normalized result list (may be empty).
    pub results: Vec<SearchResult>,
    /// Which upstream adapter served this response (e.g. `"tavily"`).
    pub provider: String,
}

/// A normalized web-search query.
///
/// Only the fields the normalized tool exposes today. Provider-specific tuning
/// (search depth, topic, domains) is intentionally NOT surfaced in PR #1 so the
/// request contract stays portable across adapters; add it behind a stable knob
/// when a second provider needs it.
#[derive(Debug, Clone)]
pub struct SearchQuery {
    /// The search query string. Validated non-empty by the route before this
    /// reaches the adapter.
    pub query: String,
    /// Maximum number of results to return, if the caller specified one.
    /// `None` lets the adapter use its own sensible default.
    pub max_results: Option<u8>,
}

/// Trait for swappable web-search upstream adapters.
///
/// Each adapter translates the normalized [`SearchQuery`] into its upstream's
/// native request and maps the native response back into [`SearchResults`].
/// Implementations must be `Send + Sync` for use in Axum's concurrent handling.
#[async_trait]
pub trait SearchProvider: Send + Sync {
    /// Provider name (e.g. `"tavily"`).
    fn name(&self) -> &str;

    /// Execute a web search and return normalized results.
    async fn search(&self, query: SearchQuery) -> Result<SearchResults, SearchError>;
}

/// Build the configured search provider from environment API keys.
///
/// Returns `Some` only when a supported upstream's key is present and non-empty
/// — mirroring [`crate::providers::ProviderRegistry::from_env`]. With no key
/// set the crate still compiles and the `/v1/search` route returns a 503
/// "not configured" (never a free or stub-paid response).
///
/// Tavily is the only adapter today; when Exa/Parallel land they slot in here
/// (first-key-wins, or a `SOLVELA_SEARCH_PROVIDER` selector if multiple are set
/// — a later decision, out of PR #1 scope).
pub fn search_provider_from_env(client: reqwest::Client) -> Option<Arc<dyn SearchProvider>> {
    if let Ok(key) = std::env::var("TAVILY_API_KEY") {
        if !key.is_empty() {
            return Some(Arc::new(tavily::TavilyProvider::new(client, key)));
        }
    }
    None
}
