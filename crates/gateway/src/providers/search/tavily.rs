//! Tavily web-search adapter.
//!
//! Upstream: `POST https://api.tavily.com/search`, authenticated with
//! `Authorization: Bearer <TAVILY_API_KEY>`. Request/response shape verified
//! against <https://docs.tavily.com/documentation/api-reference/endpoint/search>
//! (2026-06-17). Result objects carry `title`, `url`, `content`, `score`; we map
//! `content → snippet` and drop the rest at this boundary so callers depend only
//! on the normalized [`SearchResults`] shape.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::{SearchError, SearchProvider, SearchQuery, SearchResult, SearchResults};

/// Tavily search API endpoint.
const TAVILY_SEARCH_URL: &str = "https://api.tavily.com/search";

/// Effective per-request timeout for the upstream Tavily call.
///
/// Set explicitly on the request so it is the SINGLE intended bound regardless
/// of the shared client's default timeout: reqwest's `RequestBuilder::timeout`
/// OVERRIDES the client-level `Client::timeout` for that request (reqwest
/// 0.12). The gateway's shared `http_client` carries a 10s default
/// (`main.rs`), but this Tavily call runs AFTER on-chain settlement, so a hung
/// upstream must still be bounded — 30s caps the charge-without-result window
/// (the agent already paid; an unbounded wait would strand the request).
const TAVILY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Tavily web-search provider.
///
/// Holds the gateway's Tavily API key. The key is secret: the custom [`Debug`]
/// impl redacts it so it can never leak through `{:?}` / tracing.
pub struct TavilyProvider {
    client: reqwest::Client,
    api_key: String,
}

impl std::fmt::Debug for TavilyProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TavilyProvider")
            .field("api_key", &"<redacted>")
            .finish()
    }
}

impl TavilyProvider {
    /// Construct a Tavily provider with the given HTTP client and API key.
    pub fn new(client: reqwest::Client, api_key: String) -> Self {
        Self { client, api_key }
    }
}

/// Tavily request body. Only the fields we send today.
#[derive(Debug, Serialize)]
struct TavilyRequest<'a> {
    query: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_results: Option<u8>,
}

/// Tavily response body. Untrusted external data — we deserialize only the
/// fields we map, ignoring everything else. We deliberately do NOT read the
/// upstream's echoed `query`: the caller's own validated query is authoritative
/// (see `search` below), so an untrusted upstream string can never replace it.
#[derive(Debug, Deserialize)]
struct TavilyResponse {
    #[serde(default)]
    results: Vec<TavilyResult>,
}

/// A single Tavily result object. `content` is Tavily's snippet field.
#[derive(Debug, Deserialize)]
struct TavilyResult {
    #[serde(default)]
    title: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    content: String,
}

#[async_trait]
impl SearchProvider for TavilyProvider {
    fn name(&self) -> &str {
        "tavily"
    }

    async fn search(&self, query: SearchQuery) -> Result<SearchResults, SearchError> {
        let body = TavilyRequest {
            query: &query.query,
            max_results: query.max_results,
        };

        let response = self
            .client
            .post(TAVILY_SEARCH_URL)
            .bearer_auth(&self.api_key)
            .timeout(TAVILY_TIMEOUT)
            .json(&body)
            .send()
            .await?;

        let status = response.status();
        if !status.is_success() {
            // Do NOT forward the upstream body verbatim — it may echo the
            // request or carry provider internals. Surface only the status code.
            return Err(SearchError::Status(status.as_u16()));
        }

        let parsed: TavilyResponse = response
            .json()
            .await
            .map_err(|e| SearchError::Parse(e.to_string()))?;

        let results = parsed
            .results
            .into_iter()
            .map(|r| SearchResult {
                title: r.title,
                url: r.url,
                snippet: r.content,
            })
            .collect();

        // Always echo the caller's OWN validated query, never the
        // upstream-returned `parsed.query` — the upstream response is untrusted
        // data and must not become authoritative over the client's request
        // (reflecting it back would let a compromised/buggy upstream inject a
        // different string into the caller's result envelope).
        Ok(SearchResults {
            query: query.query,
            results,
            provider: "tavily".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_redacts_api_key() {
        let provider = TavilyProvider::new(reqwest::Client::new(), "tvly-supersecret".to_string());
        let debug = format!("{provider:?}");
        assert!(
            !debug.contains("tvly-supersecret"),
            "Debug must not leak the API key, got: {debug}"
        );
        assert!(debug.contains("<redacted>"), "got: {debug}");
    }

    #[test]
    fn name_is_tavily() {
        let provider = TavilyProvider::new(reqwest::Client::new(), "k".to_string());
        assert_eq!(provider.name(), "tavily");
    }

    #[test]
    fn request_omits_max_results_when_none() {
        let body = TavilyRequest {
            query: "rust async",
            max_results: None,
        };
        let json = serde_json::to_string(&body).unwrap();
        assert_eq!(json, r#"{"query":"rust async"}"#);
    }

    #[test]
    fn request_includes_max_results_when_some() {
        let body = TavilyRequest {
            query: "rust async",
            max_results: Some(3),
        };
        let json = serde_json::to_string(&body).unwrap();
        assert!(json.contains(r#""max_results":3"#), "got: {json}");
    }

    #[test]
    fn response_maps_content_to_snippet() {
        // Pin the field mapping: Tavily `content` → normalized `snippet`,
        // and that unknown fields (score, raw_content) are ignored.
        let raw = r#"{
            "query": "rust",
            "results": [
                {"title": "Rust Lang", "url": "https://rust-lang.org",
                 "content": "A systems language", "score": 0.98,
                 "raw_content": "ignored"}
            ],
            "response_time": 0.4
        }"#;
        let parsed: TavilyResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(parsed.results.len(), 1);
        assert_eq!(parsed.results[0].title, "Rust Lang");
        assert_eq!(parsed.results[0].url, "https://rust-lang.org");
        assert_eq!(parsed.results[0].content, "A systems language");
    }
}
