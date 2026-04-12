//! GET /v1/supported — x402 facilitator discovery endpoint.
//!
//! Returns the x402 payment schemes and networks this gateway supports.
//! Follows the OpenFacilitator `/supported` standard so Solvela
//! is discoverable by x402 ecosystem tooling and dashboards.

use axum::Json;
use serde::{Deserialize, Serialize};

use x402::types::{SOLANA_NETWORK, USDC_MINT, X402_VERSION};

/// A supported payment kind (scheme + network combination).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupportedKind {
    /// x402 protocol version.
    pub x402_version: u8,
    /// Payment scheme (e.g., "exact").
    pub scheme: String,
    /// Network identifier (e.g., "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp").
    pub network: String,
    /// The USDC-SPL mint address accepted for payments.
    pub asset: String,
}

/// Response body for GET /v1/supported.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SupportedResponse {
    /// All payment kinds this gateway can accept.
    pub kinds: Vec<SupportedKind>,
    /// Human-readable gateway name.
    pub gateway: &'static str,
    /// Link to pricing endpoint.
    pub pricing_url: &'static str,
}

/// GET /v1/supported
///
/// Returns x402 payment schemes and networks supported by this gateway.
/// Compatible with the OpenFacilitator discovery standard.
pub async fn supported() -> Json<SupportedResponse> {
    Json(SupportedResponse {
        kinds: vec![SupportedKind {
            x402_version: X402_VERSION,
            scheme: "exact".to_string(),
            network: SOLANA_NETWORK.to_string(),
            asset: USDC_MINT.to_string(),
        }],
        gateway: "Solvela",
        pricing_url: "/v1/models",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_supported_response() {
        let Json(resp) = supported().await;

        assert_eq!(resp.kinds.len(), 1);
        assert_eq!(resp.kinds[0].x402_version, X402_VERSION);
        assert_eq!(resp.kinds[0].scheme, "exact");
        assert_eq!(resp.kinds[0].network, SOLANA_NETWORK);
        assert_eq!(resp.kinds[0].asset, USDC_MINT);
        assert_eq!(resp.gateway, "Solvela");
    }
}
