use std::fmt;

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use secrecy::{ExposeSecret, SecretString};

use x402::types::{
    PayloadData, PaymentAccept, PaymentPayload, Resource, SolanaPayload, X402_VERSION,
};

use crate::commands::solana_tx::build_usdc_transfer;

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

/// Real SPL TransferChecked payment strategy for exact-scheme load testing.
///
/// Signs a USDC-SPL transfer for each 402 response using the provided keypair.
/// The `rpc_client` is shared across all requests to avoid per-request allocation.
pub struct ExactPaymentStrategy {
    keypair_b58: SecretString,
    rpc_client: reqwest::Client,
}

impl ExactPaymentStrategy {
    /// Create a new `ExactPaymentStrategy`.
    ///
    /// # Arguments
    /// * `keypair_b58` — 64-byte Solana keypair in base58, wrapped in `SecretString`
    /// * `rpc_client` — shared reqwest client for Solana RPC calls
    pub fn new(keypair_b58: SecretString, rpc_client: reqwest::Client) -> Self {
        Self {
            keypair_b58,
            rpc_client,
        }
    }
}

impl fmt::Debug for ExactPaymentStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExactPaymentStrategy")
            .field("keypair_b58", &"[REDACTED]")
            .field("rpc_client", &self.rpc_client)
            .finish()
    }
}

#[async_trait::async_trait]
impl PaymentStrategy for ExactPaymentStrategy {
    fn name(&self) -> &'static str {
        "exact"
    }

    async fn prepare_payment(
        &self,
        rpc_url: &str,
        _request_body: &serde_json::Value,
        accepts: &[PaymentAccept],
    ) -> Result<Option<String>> {
        // Find the exact-scheme entry from the 402 accepts list.
        let accepted = accepts
            .iter()
            .find(|a| a.scheme == "exact")
            .context("no 'exact' payment scheme in 402 accepts")?
            .clone();

        let amount: u64 = accepted
            .amount
            .parse()
            .context("invalid payment amount from gateway")?;

        // Build and sign the USDC-SPL TransferChecked transaction.
        let signed_tx = build_usdc_transfer(
            self.keypair_b58.expose_secret(),
            &accepted.pay_to,
            amount,
            rpc_url,
            &self.rpc_client,
        )
        .await
        .context("failed to build USDC transfer for exact payment")?;

        // Assemble the PaymentPayload the gateway expects.
        let payment_payload = PaymentPayload {
            x402_version: X402_VERSION,
            resource: Resource {
                url: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
            },
            accepted,
            payload: PayloadData::Direct(SolanaPayload {
                transaction: signed_tx,
            }),
        };

        // Encode as base64(JSON(payload)) — same format as the chat command.
        let payload_json = serde_json::to_string(&payment_payload)
            .context("failed to serialize payment payload")?;
        let header_value = BASE64.encode(payload_json.as_bytes());

        Ok(Some(header_value))
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

    #[test]
    fn test_exact_payment_debug_redacts_keypair() {
        let strategy = ExactPaymentStrategy::new(
            SecretString::new("super-secret-keypair".to_string()),
            reqwest::Client::new(),
        );
        let debug_output = format!("{:?}", strategy);
        assert!(
            debug_output.contains("[REDACTED]"),
            "Debug output should redact the keypair"
        );
        assert!(
            !debug_output.contains("super-secret"),
            "Debug output must not leak the keypair"
        );
    }

    #[test]
    fn test_exact_payment_name() {
        let strategy =
            ExactPaymentStrategy::new(SecretString::new("key".to_string()), reqwest::Client::new());
        assert_eq!(strategy.name(), "exact");
    }

    #[tokio::test]
    async fn test_exact_payment_no_exact_scheme_returns_error() {
        let strategy =
            ExactPaymentStrategy::new(SecretString::new("key".to_string()), reqwest::Client::new());
        // Provide only an escrow accept — no exact scheme available.
        let accepts = vec![PaymentAccept {
            scheme: "escrow".to_string(),
            network: "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp".to_string(),
            amount: "1000".to_string(),
            asset: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            pay_to: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM".to_string(),
            max_timeout_seconds: 300,
            escrow_program_id: Some("program-id".to_string()),
        }];

        let result = strategy
            .prepare_payment("http://localhost:8899", &serde_json::json!({}), &accepts)
            .await;

        assert!(result.is_err(), "should error when no exact scheme found");
        assert!(
            result.unwrap_err().to_string().contains("exact"),
            "error should mention 'exact'"
        );
    }

    #[tokio::test]
    async fn test_exact_payment_builds_valid_header() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // Start a mock Solana RPC server.
        let mock_rpc = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "value": {
                        "blockhash": "11111111111111111111111111111111",
                        "lastValidBlockHeight": 9999
                    }
                }
            })))
            .mount(&mock_rpc)
            .await;

        // Generate a valid 64-byte keypair.
        let seed = [42u8; 32];
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
        let verifying_key = signing_key.verifying_key();
        let mut full_key = [0u8; 64];
        full_key[..32].copy_from_slice(&seed);
        full_key[32..].copy_from_slice(verifying_key.as_bytes());
        let keypair_b58 = bs58::encode(&full_key).into_string();

        let strategy =
            ExactPaymentStrategy::new(SecretString::new(keypair_b58), reqwest::Client::new());

        let accepts = vec![PaymentAccept {
            scheme: "exact".to_string(),
            network: "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp".to_string(),
            amount: "1000".to_string(),
            asset: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            pay_to: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM".to_string(),
            max_timeout_seconds: 300,
            escrow_program_id: None,
        }];

        let result = strategy
            .prepare_payment(
                &mock_rpc.uri(),
                &serde_json::json!({"model": "auto"}),
                &accepts,
            )
            .await;

        assert!(
            result.is_ok(),
            "exact payment should succeed: {:?}",
            result.err()
        );
        let header_value = result
            .expect("already checked is_ok")
            .expect("exact strategy should return Some");

        // Decode the header to verify it's valid base64 -> JSON -> PaymentPayload.
        let decoded_bytes = BASE64
            .decode(&header_value)
            .expect("header should be valid base64");
        let payload: PaymentPayload = serde_json::from_slice(&decoded_bytes)
            .expect("decoded header should be valid PaymentPayload JSON");

        assert_eq!(payload.x402_version, X402_VERSION);
        assert_eq!(payload.resource.url, "/v1/chat/completions");
        assert_eq!(payload.resource.method, "POST");
        assert_eq!(payload.accepted.scheme, "exact");
        assert_eq!(payload.accepted.amount, "1000");
        assert!(
            matches!(payload.payload, PayloadData::Direct(_)),
            "payload should be Direct variant"
        );
    }
}
