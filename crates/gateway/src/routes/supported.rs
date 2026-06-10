//! GET /v1/supported — x402 facilitator discovery endpoint.
//!
//! Returns the x402 payment schemes and networks this gateway supports.
//! Follows the OpenFacilitator `/supported` standard so Solvela
//! is discoverable by x402 ecosystem tooling and dashboards.

use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};

use solvela_x402::types::{SOLANA_NETWORK, X402_VERSION};

use crate::AppState;

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

/// Build the `/v1/supported` response for a given accepted USDC mint.
///
/// The mint is the CONFIGURED one (`config.solana.usdc_mint`) — the same mint
/// the payment verifiers enforce — never the compile-time constant, so a
/// deployment with a non-default mint advertises an asset it will actually
/// accept.
fn supported_response(usdc_mint: &str) -> SupportedResponse {
    SupportedResponse {
        kinds: vec![SupportedKind {
            x402_version: X402_VERSION,
            scheme: "exact".to_string(),
            network: SOLANA_NETWORK.to_string(),
            asset: usdc_mint.to_string(),
        }],
        gateway: "Solvela",
        pricing_url: "/v1/models",
    }
}

/// GET /v1/supported
///
/// Returns x402 payment schemes and networks supported by this gateway.
/// Compatible with the OpenFacilitator discovery standard.
pub async fn supported(State(state): State<Arc<AppState>>) -> Json<SupportedResponse> {
    Json(supported_response(&state.config.solana.usdc_mint))
}

#[cfg(test)]
mod tests {
    use super::*;
    use solvela_x402::types::USDC_MINT;

    #[test]
    fn test_supported_response_default_mint() {
        let resp = supported_response(USDC_MINT);

        assert_eq!(resp.kinds.len(), 1);
        assert_eq!(resp.kinds[0].x402_version, X402_VERSION);
        assert_eq!(resp.kinds[0].scheme, "exact");
        assert_eq!(resp.kinds[0].network, SOLANA_NETWORK);
        // Pin the LITERAL mainnet mint, not the constant against itself —
        // this is the wire-stability guarantee for default deployments.
        assert_eq!(
            resp.kinds[0].asset,
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
        );
        assert_eq!(resp.gateway, "Solvela");
    }

    #[test]
    fn test_supported_response_reports_configured_mint() {
        let devnet_mint = "4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU";
        let resp = supported_response(devnet_mint);
        assert_eq!(resp.kinds[0].asset, devnet_mint);
    }
}
