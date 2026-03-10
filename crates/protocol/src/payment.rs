use serde::{Deserialize, Serialize};

use crate::cost::CostBreakdown;

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
    /// Payment scheme (e.g., "exact", "escrow").
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
    /// Escrow program ID — only present for scheme="escrow".
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub escrow_program_id: Option<String>,
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

/// Solana-specific payment data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolanaPayload {
    /// Base64-encoded signed versioned transaction.
    pub transaction: String,
}

/// Escrow-specific payment payload (scheme = "escrow").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EscrowPayload {
    /// Base64-encoded signed deposit transaction (Solana versioned tx).
    pub deposit_tx: String,
    /// 32-byte request correlation ID — used as escrow PDA seed.
    /// Base64-encoded.
    pub service_id: String,
    /// Agent wallet pubkey (base58) — used to derive escrow PDA.
    pub agent_pubkey: String,
}

/// Union of direct-transfer and escrow payment payloads.
/// Uses untagged deserialization — EscrowPayload is tried first (it has
/// more fields), falling back to SolanaPayload for "exact" scheme clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PayloadData {
    Escrow(EscrowPayload),
    Direct(SolanaPayload),
}

/// The payment payload sent in the `PAYMENT-SIGNATURE` header.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentPayload {
    pub x402_version: u8,
    pub resource: Resource,
    pub accepted: PaymentAccept,
    pub payload: PayloadData,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::*;
    use crate::cost::CostBreakdown;

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
                escrow_program_id: None,
            }],
            cost_breakdown: CostBreakdown {
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

    #[test]
    fn test_payment_accept_escrow_serialization() {
        let accept = PaymentAccept {
            scheme: "escrow".to_string(),
            network: SOLANA_NETWORK.to_string(),
            amount: "5000".to_string(),
            asset: USDC_MINT.to_string(),
            pay_to: "RecipientWalletPubkeyHere".to_string(),
            max_timeout_seconds: MAX_TIMEOUT_SECONDS,
            escrow_program_id: Some("GTs7ik3NbW3xwSXq33jyVRGgmshNEyW1h9rxDNATiFLy".to_string()),
        };
        let json = serde_json::to_string(&accept).unwrap();
        assert!(json.contains("escrow_program_id"));
        assert!(json.contains("GTs7ik3NbW3xwSXq33jyVRGgmshNEyW1h9rxDNATiFLy"));
    }

    #[test]
    fn test_payment_accept_exact_no_escrow_field() {
        let accept = PaymentAccept {
            scheme: "exact".to_string(),
            network: SOLANA_NETWORK.to_string(),
            amount: "2625".to_string(),
            asset: USDC_MINT.to_string(),
            pay_to: "RecipientWalletPubkeyHere".to_string(),
            max_timeout_seconds: MAX_TIMEOUT_SECONDS,
            escrow_program_id: None,
        };
        let json = serde_json::to_string(&accept).unwrap();
        assert!(!json.contains("escrow_program_id"), "escrow_program_id should be absent when None");
    }

    #[test]
    fn test_payload_data_direct_roundtrip() {
        let direct = PayloadData::Direct(SolanaPayload { transaction: "dGVzdA==".to_string() });
        let json = serde_json::to_string(&direct).unwrap();
        let deserialized: PayloadData = serde_json::from_str(&json).unwrap();
        match deserialized {
            PayloadData::Direct(p) => assert_eq!(p.transaction, "dGVzdA=="),
            PayloadData::Escrow(_) => panic!("expected Direct variant"),
        }
    }

    #[test]
    fn test_payload_data_escrow_roundtrip() {
        let escrow = PayloadData::Escrow(EscrowPayload {
            deposit_tx: "dGVzdA==".to_string(),
            service_id: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_string(),
            agent_pubkey: "11111111111111111111111111111111".to_string(),
        });
        let json = serde_json::to_string(&escrow).unwrap();
        let deserialized: PayloadData = serde_json::from_str(&json).unwrap();
        match deserialized {
            PayloadData::Escrow(p) => {
                assert_eq!(p.deposit_tx, "dGVzdA==");
                assert_eq!(p.agent_pubkey, "11111111111111111111111111111111");
            }
            PayloadData::Direct(_) => panic!("expected Escrow variant"),
        }
    }

    #[test]
    fn test_escrow_payload_serde_roundtrip() {
        let ep = EscrowPayload {
            deposit_tx: "abc123".to_string(),
            service_id: "c2VydmljZTEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNA==".to_string(),
            agent_pubkey: "9noXzpXnkyEcKF3AeXqUHTdR59V5uvrRBUo9bwsHaByz".to_string(),
        };
        let json = serde_json::to_string(&ep).unwrap();
        let deserialized: EscrowPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.deposit_tx, ep.deposit_tx);
        assert_eq!(deserialized.service_id, ep.service_id);
        assert_eq!(deserialized.agent_pubkey, ep.agent_pubkey);
    }
}
