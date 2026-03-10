use serde::{Deserialize, Serialize};

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
    /// Verified deposit amount in atomic units (escrow scheme only).
    /// Used to cap the claim amount so it never exceeds the deposited amount.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub verified_amount: Option<u64>,
}
