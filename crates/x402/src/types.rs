use serde::{Deserialize, Serialize};

use rcr_common::types::CostBreakdown;

/// x402 protocol version.
pub const X402_VERSION: u8 = 2;

/// USDC-SPL mint address on Solana mainnet.
pub const USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

/// Solana mainnet network identifier for x402.
pub const SOLANA_NETWORK: &str = "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp";

/// Maximum timeout for payment authorization (5 minutes).
pub const MAX_TIMEOUT_SECONDS: u64 = 300;

// ---------------------------------------------------------------------------
// 402 Payment Required response types
// ---------------------------------------------------------------------------

/// Describes a resource that requires payment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resource {
    /// The URL path of the resource.
    pub url: String,
    /// HTTP method.
    pub method: String,
}

/// Describes an accepted payment method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentAccept {
    /// Payment scheme (e.g., "exact").
    pub scheme: String,
    /// Network identifier (e.g., "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp").
    pub network: String,
    /// Amount in atomic units (USDC has 6 decimals).
    pub amount: String,
    /// Token mint/contract address.
    pub asset: String,
    /// Recipient wallet address.
    pub pay_to: String,
    /// Maximum seconds the payment authorization is valid.
    pub max_timeout_seconds: u64,
}

/// The full 402 Payment Required response body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentRequired {
    pub x402_version: u8,
    pub resource: Resource,
    pub accepts: Vec<PaymentAccept>,
    pub cost_breakdown: CostBreakdown,
    pub error: String,
}

// ---------------------------------------------------------------------------
// Payment payload types (sent by client in PAYMENT-SIGNATURE header)
// ---------------------------------------------------------------------------

/// Solana-specific payment data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolanaPayload {
    /// Base64-encoded signed versioned transaction.
    pub transaction: String,
}

/// The payment payload sent in the `PAYMENT-SIGNATURE` header.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentPayload {
    pub x402_version: u8,
    pub resource: Resource,
    pub accepted: PaymentAccept,
    pub payload: SolanaPayload,
}

// ---------------------------------------------------------------------------
// Settlement types
// ---------------------------------------------------------------------------

/// Result of payment verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    /// Whether the payment is valid.
    pub valid: bool,
    /// Human-readable reason if invalid.
    pub reason: Option<String>,
    /// Verified amount in atomic units.
    pub verified_amount: Option<u64>,
}

/// Result of payment settlement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettlementResult {
    /// Whether settlement was successful.
    pub success: bool,
    /// Transaction signature (base58 for Solana).
    pub tx_signature: Option<String>,
    /// Network the settlement occurred on.
    pub network: String,
    /// Error message if settlement failed.
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_payment_required_serialization() {
        let pr = PaymentRequired {
            x402_version: X402_VERSION,
            resource: Resource {
                url: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
            },
            accepts: vec![PaymentAccept {
                scheme: "exact".to_string(),
                network: SOLANA_NETWORK.to_string(),
                amount: "2625".to_string(),
                asset: USDC_MINT.to_string(),
                pay_to: "RecipientWalletPubkeyHere".to_string(),
                max_timeout_seconds: MAX_TIMEOUT_SECONDS,
            }],
            cost_breakdown: rcr_common::types::CostBreakdown {
                provider_cost: "0.002500".to_string(),
                platform_fee: "0.000125".to_string(),
                total: "0.002625".to_string(),
                currency: "USDC".to_string(),
                fee_percent: 5,
            },
            error: "Payment required".to_string(),
        };

        let json = serde_json::to_string_pretty(&pr).unwrap();
        assert!(json.contains("x402_version"));
        assert!(json.contains("solana:"));
        assert!(json.contains("cost_breakdown"));
    }
}
