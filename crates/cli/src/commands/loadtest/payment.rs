use anyhow::Result;

use x402::types::PaymentAccept;

/// Trait abstracting payment signing for load test workers.
///
/// Each mode (dev-bypass, exact, escrow) implements this trait.
/// The worker calls `prepare_payment` after receiving a 402 response
/// and uses the returned header value (if any) to retry the request.
#[async_trait::async_trait]
pub trait PaymentStrategy: Send + Sync {
    /// Human-readable name for reporting.
    fn name(&self) -> &'static str;

    /// Prepare the PAYMENT-SIGNATURE header value for a request.
    ///
    /// Returns `Ok(None)` if no payment is needed (dev-bypass mode).
    /// Returns `Ok(Some(header_value))` with the base64-encoded payment payload.
    /// The `accepts` slice comes from the 402 response's `PaymentRequired.accepts`.
    async fn prepare_payment(
        &self,
        rpc_url: &str,
        request_body: &serde_json::Value,
        accepts: &[PaymentAccept],
    ) -> Result<Option<String>>;
}

/// No-op payment strategy for dev-bypass mode.
///
/// Relies on the gateway having `RCR_DEV_BYPASS_PAYMENT=true` set.
/// No wallet needed, no Solana RPC calls.
pub struct DevBypassStrategy;

#[async_trait::async_trait]
impl PaymentStrategy for DevBypassStrategy {
    fn name(&self) -> &'static str {
        "dev-bypass"
    }

    async fn prepare_payment(
        &self,
        _rpc_url: &str,
        _request_body: &serde_json::Value,
        _accepts: &[PaymentAccept],
    ) -> Result<Option<String>> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_dev_bypass_returns_none() {
        let strategy = DevBypassStrategy;
        let result = strategy
            .prepare_payment("http://localhost:8402", &serde_json::json!({}), &[])
            .await
            .expect("dev bypass should not error");
        assert!(
            result.is_none(),
            "dev bypass should return no payment header"
        );
    }

    #[tokio::test]
    async fn test_dev_bypass_display_name() {
        let strategy = DevBypassStrategy;
        assert_eq!(strategy.name(), "dev-bypass");
    }
}
