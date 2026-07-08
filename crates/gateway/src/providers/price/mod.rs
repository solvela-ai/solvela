//! Solana price tool provider adapters.
//!
//! Mirrors the [`crate::providers::search`] pattern (the web-search donor): a
//! swappable trait ([`PriceProvider`]) with concrete upstream adapters, so
//! Birdeye/Pyth can drop in later without changing the route or the response
//! contract. Jupiter is the only adapter today.
//!
//! ## Enablement divergence from the search donor
//!
//! The Jupiter Price API's lite host (`lite-api.jup.ag`) is KEYLESS, so unlike
//! search there is no API-key env gate: `main.rs` always constructs the
//! provider, and the route's enablement gate is the `solana-price` entry in
//! `config/services.toml` (entry missing/unpriced → 503). The `AppState` field
//! stays `Option` for test injection and defense in depth only.
//!
//! ## Normalized response shape
//!
//! [`PriceResults`] is the stable contract returned to callers: one
//! [`MintPrice`] per REQUESTED mint, in request order, with `price: null` for
//! a mint the upstream has no reliable price for (mirroring Jupiter's
//! omit-unknown semantics as an EXPLICIT null, never an error for the whole
//! batch). Numeric market-data fields are relayed as [`serde_json::Number`]s
//! at f64 precision — they are informational market data and are NEVER an
//! input to any billing computation (the route's flat per-call price comes
//! from the service registry alone).

pub mod jupiter;

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Error type for price-provider operations.
///
/// Same shape and rationale as [`crate::providers::search::SearchError`]: the
/// route maps every variant to the same opaque 502 (never leak upstream
/// internals — GHSA-cgqx-mg48-949v), but the typing lets a later change branch
/// on the failure class.
#[derive(Debug, thiserror::Error)]
pub enum PriceError {
    /// Transport-level failure reaching the upstream (DNS, connect, timeout).
    #[error("price upstream request failed: {0}")]
    Upstream(#[from] reqwest::Error),

    /// The upstream returned a non-success HTTP status. Carries only the
    /// numeric code — never the upstream body.
    #[error("price upstream returned status {0}")]
    Status(u16),

    /// The upstream response could not be parsed into the expected shape.
    #[error("price upstream response parse error: {0}")]
    Parse(String),
}

/// Price data for a single mint, as reported by the upstream.
///
/// `usd_price` is the load-bearing field; the rest are stable Jupiter-provided
/// recency/metadata fields (optional defensively — a missing metadata field
/// must not fail the whole batch). All numbers are relayed as JSON numbers at
/// f64 precision (informational market data); none of them ever feeds a
/// billing computation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PriceData {
    /// USD price of one whole token, as reported by the upstream.
    pub usd_price: serde_json::Number,
    /// Token decimals (display metadata).
    pub decimals: Option<u8>,
    /// Chain block/slot the price was derived at (recency indicator).
    pub block_id: Option<u64>,
    /// 24-hour percentage change (e.g. `-3.99` = −3.99%).
    pub price_change_24h: Option<serde_json::Number>,
}

/// One entry per REQUESTED mint, in request order.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MintPrice {
    /// The requested mint address (base58), echoed from the request.
    pub mint: String,
    /// The upstream's price data, or `null` when the upstream has no reliable
    /// price for this mint (Jupiter omits such mints from its response; the
    /// adapter normalizes the omission to an explicit `null` here so the
    /// caller sees a per-mint answer, never a whole-batch error).
    pub price: Option<PriceData>,
}

/// Normalized response returned by `POST /v1/solana/price`.
///
/// Stable across providers: callers read `prices` regardless of which upstream
/// served the request. `source` names the adapter (informational, not a
/// contract to branch on); `as_of` is the gateway-side RFC 3339 timestamp of
/// when the upstream answered (per-mint `block_id` carries chain recency).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PriceResults {
    /// One entry per requested mint, in request order.
    pub prices: Vec<MintPrice>,
    /// Which upstream adapter served this response (e.g. `"jupiter"`).
    pub source: String,
    /// RFC 3339 UTC timestamp of when the upstream response was received.
    pub as_of: String,
}

/// Trait for swappable price upstream adapters.
///
/// Each adapter translates the validated mint list into its upstream's native
/// request and maps the native response back into [`PriceResults`].
#[async_trait]
pub trait PriceProvider: Send + Sync {
    /// Provider name (e.g. `"jupiter"`).
    fn name(&self) -> &str;

    /// Fetch USD prices for the given mints (already validated by the route:
    /// 1..=50 entries, each base58-of-32-bytes) and return one normalized
    /// entry per requested mint, in request order.
    async fn prices(&self, mints: &[String]) -> Result<PriceResults, PriceError>;
}

/// Build the price provider.
///
/// ALWAYS returns a provider (contrast `search_provider_from_env`): the
/// Jupiter lite host is keyless, so there is no key to gate on. An optional
/// non-empty `JUPITER_API_KEY` switches to the keyed host
/// (`api.jup.ag`, same contract, higher rate limits).
pub fn price_provider_from_env(client: reqwest::Client) -> Arc<dyn PriceProvider> {
    let api_key = std::env::var("JUPITER_API_KEY")
        .ok()
        .filter(|k| !k.is_empty());
    Arc::new(jupiter::JupiterProvider::new(client, api_key))
}
