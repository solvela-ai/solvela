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
