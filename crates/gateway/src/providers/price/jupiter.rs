//! Jupiter Price API adapter.
//!
//! Upstream: `GET https://lite-api.jup.ag/price/v3?ids=<mint>,<mint>,...`
//! (keyless "Lite" host) or `GET https://api.jup.ag/price/v3?...` with an
//! `x-api-key` header when `JUPITER_API_KEY` is configured — same contract on
//! both hosts. Request/response shape verified against the live lite endpoint
//! and <https://developers.jup.ag/docs/price> (2026-07-08):
//!
//! - `ids` is a comma-separated list of mint addresses, **max 50 per request**
//!   (the route enforces the same cap before payment).
//! - The response is a JSON object keyed by mint; each entry carries
//!   `usdPrice`, `blockId`, `decimals`, `priceChange24h` (plus fields we drop,
//!   e.g. `liquidity`).
//! - Mints without a reliable price are **omitted entirely** (no key, no
//!   per-mint error). [`normalize`] maps the omission to an explicit
//!   `price: null` entry so callers get one answer per requested mint.
//! - Free/keyless tier rate limit: ~60 requests/minute against the shared
//!   default bucket (Jupiter portal docs, 2026-07); the keyed host raises this
//!   (Developer 600/min and up). The per-agent x402 price plus the gateway's
//!   outer per-IP limiter keep organic traffic well under it.

use async_trait::async_trait;
use serde::Deserialize;

use super::{MintPrice, PriceData, PriceError, PriceProvider, PriceResults};

/// Keyless ("Lite") Jupiter host — no API key required.
const JUPITER_LITE_BASE: &str = "https://lite-api.jup.ag";

/// Keyed Jupiter host — same contract, higher rate limits, `x-api-key` auth.
const JUPITER_KEYED_BASE: &str = "https://api.jup.ag";

/// Price API v3 path, shared by both hosts.
const PRICE_V3_PATH: &str = "/price/v3";

/// Effective per-request timeout for the upstream Jupiter call.
///
/// Same rationale as the Tavily adapter: set explicitly on the request so it
/// is the SINGLE intended bound regardless of the shared client's default
/// (reqwest's `RequestBuilder::timeout` overrides the client-level timeout for
/// that request). This call runs AFTER on-chain settlement on the exact path,
/// so a hung upstream must still be bounded — 30s caps the
/// charge-without-result window.
const JUPITER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Jupiter Price API provider.
///
/// Holds an OPTIONAL API key (the lite host needs none). The key is secret:
/// the custom [`Debug`] impl redacts it so it can never leak through `{:?}` /
/// tracing.
pub struct JupiterProvider {
    client: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
}

impl std::fmt::Debug for JupiterProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JupiterProvider")
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

impl JupiterProvider {
    /// Construct a Jupiter provider. `api_key = None` uses the keyless lite
    /// host; `Some` uses the keyed host with the same wire contract.
    pub fn new(client: reqwest::Client, api_key: Option<String>) -> Self {
        let base_url = if api_key.is_some() {
            JUPITER_KEYED_BASE.to_string()
        } else {
            JUPITER_LITE_BASE.to_string()
        };
        Self {
            client,
            base_url,
            api_key,
        }
    }
}

/// A single Jupiter price entry. Untrusted external data — we deserialize only
/// the fields we map, ignoring everything else (`liquidity`, `createdAt`, …).
/// Only `usdPrice` is required; the metadata fields are defensive `Option`s so
/// a missing field on one mint cannot fail the whole batch.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct JupiterEntry {
    usd_price: serde_json::Number,
    #[serde(default)]
    decimals: Option<u8>,
    #[serde(default)]
    block_id: Option<u64>,
    #[serde(default)]
    price_change_24h: Option<serde_json::Number>,
}

/// Map the upstream mint-keyed object back onto the REQUESTED mint list, in
/// request order, with an explicit `None` for every mint the upstream omitted
/// (its documented no-reliable-price semantics). Pure — unit-tested below.
fn normalize(
    mints: &[String],
    mut parsed: std::collections::HashMap<String, JupiterEntry>,
    as_of: String,
) -> PriceResults {
    let prices = mints
        .iter()
        .map(|mint| MintPrice {
            mint: mint.clone(),
            // `remove` (not `get`) so each upstream entry is consumed once; a
            // duplicated request mint yields data for the first occurrence and
            // an explicit null for the repeat — the route's docs tell callers
            // to send each mint once.
            price: parsed.remove(mint).map(|e| PriceData {
                usd_price: e.usd_price,
                decimals: e.decimals,
                block_id: e.block_id,
                price_change_24h: e.price_change_24h,
            }),
        })
        .collect();
    PriceResults {
        prices,
        source: "jupiter".to_string(),
        as_of,
    }
}

#[async_trait]
impl PriceProvider for JupiterProvider {
    fn name(&self) -> &str {
        "jupiter"
    }

    async fn prices(&self, mints: &[String]) -> Result<PriceResults, PriceError> {
        // The route validates every mint as base58-of-32-bytes before payment,
        // so the joined `ids` value is URL-safe by construction.
        let ids = mints.join(",");
        let url = format!("{}{}", self.base_url, PRICE_V3_PATH);

        let mut request = self
            .client
            .get(&url)
            .query(&[("ids", ids.as_str())])
            .timeout(JUPITER_TIMEOUT);
        if let Some(key) = &self.api_key {
            request = request.header("x-api-key", key);
        }

        let response = request.send().await?;

        let status = response.status();
        if !status.is_success() {
            // Do NOT forward the upstream body verbatim — surface only the
            // status code (GHSA-cgqx-mg48-949v posture, mirrors Tavily).
            return Err(PriceError::Status(status.as_u16()));
        }

        let parsed: std::collections::HashMap<String, JupiterEntry> = response
            .json()
            .await
            .map_err(|e| PriceError::Parse(e.to_string()))?;

        let as_of = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        Ok(normalize(mints, parsed, as_of))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact response shape observed live from
    /// `lite-api.jup.ag/price/v3?ids=EPjF...,So111...2` on 2026-07-08.
    const LIVE_SHAPE: &str = r#"{
        "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v": {
            "createdAt": "2024-06-05T08:55:25.527Z",
            "liquidity": 374344298.2809153,
            "usdPrice": 0.9998736983419257,
            "blockId": 431537809,
            "decimals": 6,
            "priceChange24h": -0.0024366454583391443
        },
        "So11111111111111111111111111111111111111112": {
            "usdPrice": 78.00526773041928,
            "blockId": 431537809,
            "decimals": 9,
            "priceChange24h": -3.9907973603850517
        }
    }"#;

    #[test]
    fn debug_redacts_api_key() {
        let provider =
            JupiterProvider::new(reqwest::Client::new(), Some("jup-supersecret".to_string()));
        let debug = format!("{provider:?}");
        assert!(
            !debug.contains("jup-supersecret"),
            "Debug must not leak the API key, got: {debug}"
        );
        assert!(debug.contains("<redacted>"), "got: {debug}");
    }

    #[test]
    fn name_is_jupiter() {
        let provider = JupiterProvider::new(reqwest::Client::new(), None);
        assert_eq!(provider.name(), "jupiter");
    }

    #[test]
    fn keyless_uses_lite_host_and_key_uses_keyed_host() {
        let lite = JupiterProvider::new(reqwest::Client::new(), None);
        assert_eq!(lite.base_url, JUPITER_LITE_BASE);
        let keyed = JupiterProvider::new(reqwest::Client::new(), Some("k".to_string()));
        assert_eq!(keyed.base_url, JUPITER_KEYED_BASE);
    }

    #[test]
    fn parses_live_response_shape_and_ignores_extra_fields() {
        let parsed: std::collections::HashMap<String, JupiterEntry> =
            serde_json::from_str(LIVE_SHAPE).unwrap();
        let usdc = &parsed["EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"];
        // Approximate compare: serde_json's default float parse (no
        // `float_roundtrip` feature) can differ from std's by 1 ULP, so an
        // exact assertion is flaky by construction. This field is
        // informational market data — never a billing input — so ULP-level
        // precision is not load-bearing anywhere.
        let usd = usdc.usd_price.as_f64().unwrap();
        assert!(
            (usd - 0.9998736983419257).abs() < 1e-12,
            "usdPrice must parse to ~0.99987…, got {usd}"
        );
        assert_eq!(usdc.decimals, Some(6));
        assert_eq!(usdc.block_id, Some(431_537_809));
        assert!(usdc.price_change_24h.is_some());
    }

    #[test]
    fn normalize_maps_omitted_mint_to_explicit_null() {
        let parsed: std::collections::HashMap<String, JupiterEntry> =
            serde_json::from_str(LIVE_SHAPE).unwrap();
        let mints = vec![
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            // NOT in the upstream response — Jupiter's omit-unknown semantics.
            "4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU".to_string(),
            "So11111111111111111111111111111111111111112".to_string(),
        ];
        let results = normalize(&mints, parsed, "2026-07-08T00:00:00Z".to_string());

        assert_eq!(results.source, "jupiter");
        assert_eq!(results.prices.len(), 3, "one entry per requested mint");
        // Request order preserved.
        assert_eq!(results.prices[0].mint, mints[0]);
        assert_eq!(results.prices[1].mint, mints[1]);
        assert_eq!(results.prices[2].mint, mints[2]);
        assert!(results.prices[0].price.is_some());
        assert!(
            results.prices[1].price.is_none(),
            "an upstream-omitted mint must normalize to an explicit None/null"
        );
        assert!(results.prices[2].price.is_some());
    }

    #[test]
    fn normalize_serializes_null_price_on_the_wire() {
        let parsed: std::collections::HashMap<String, JupiterEntry> =
            serde_json::from_str("{}").unwrap();
        let mints = vec!["4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU".to_string()];
        let results = normalize(&mints, parsed, "2026-07-08T00:00:00Z".to_string());
        let json = serde_json::to_value(&results).unwrap();
        assert!(
            json["prices"][0]["price"].is_null(),
            "the wire shape must carry an explicit null, got: {json}"
        );
    }

    #[test]
    fn entry_requires_usd_price() {
        // An entry with no usdPrice is a parse error — never a silent zero.
        let err = serde_json::from_str::<JupiterEntry>(r#"{"decimals": 6}"#);
        assert!(err.is_err(), "usdPrice must be required");
    }
}
