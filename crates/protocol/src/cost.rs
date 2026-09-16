use serde::{Deserialize, Serialize};

/// Cost breakdown returned in 402 responses and receipts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostBreakdown {
    /// Raw provider cost in USDC.
    pub provider_cost: String,
    /// Platform fee in USDC (at `fee_percent`).
    pub platform_fee: String,
    /// Total cost to the agent in USDC.
    pub total: String,
    /// Always "USDC".
    pub currency: String,
    /// Platform fee percentage. The LIVE value
    /// ([`crate::platform_fee_percent`]), not the compile-time constant —
    /// producers must never hard-code it.
    pub fee_percent: u8,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::PLATFORM_FEE_PERCENT;

    #[test]
    fn test_cost_breakdown_serialization() {
        let cost = CostBreakdown {
            provider_cost: "0.002500".to_string(),
            platform_fee: "0.000125".to_string(),
            total: "0.002625".to_string(),
            currency: "USDC".to_string(),
            fee_percent: PLATFORM_FEE_PERCENT,
        };
        let json = serde_json::to_value(&cost).unwrap();
        assert_eq!(json["fee_percent"], 5);
        assert_eq!(json["currency"], "USDC");
    }
}
