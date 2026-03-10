use serde::{Deserialize, Serialize};

/// Cost breakdown returned in 402 responses and receipts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostBreakdown {
    /// Raw provider cost in USDC.
    pub provider_cost: String,
    /// Platform fee in USDC (5%).
    pub platform_fee: String,
    /// Total cost to the agent in USDC.
    pub total: String,
    /// Always "USDC".
    pub currency: String,
    /// Platform fee percentage (5).
    pub fee_percent: u8,
}
