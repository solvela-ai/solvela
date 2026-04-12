use std::fmt;

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use secrecy::{ExposeSecret, SecretString};
use sha2::{Digest, Sha256};

use x402::types::{
    EscrowPayload, PayloadData, PaymentAccept, PaymentPayload, Resource, SolanaPayload,
    X402_VERSION,
};

use crate::commands::solana_tx::{build_escrow_deposit, build_usdc_transfer, fetch_current_slot};

/// Trait abstracting payment signing for load test workers.
///
/// Each mode (dev-bypass, exact, escrow) implements this trait.
/// The worker calls `prepare_payment` after receiving a 402 response
/// and uses the returned header value (if any) to retry the request.
#[async_trait::async_trait]
#[allow(dead_code)]
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
/// Relies on the gateway having `SOLVELA_DEV_BYPASS_PAYMENT=true` (or `RCR_DEV_BYPASS_PAYMENT=true`) set.
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

/// Generate a unique 32-byte service_id by hashing the request body + random nonce.
///
/// Mirrors the pattern used in `chat.rs` — SHA-256(body || 8-byte nonce).
fn generate_service_id(request_body: &[u8]) -> Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    hasher.update(request_body);
    let mut nonce = [0u8; 8];
    getrandom::getrandom(&mut nonce).context("getrandom failed to generate nonce")?;
    hasher.update(nonce);
    let hash = hasher.finalize();
    let mut id = [0u8; 32];
    id.copy_from_slice(&hash);
    Ok(id)
}

/// Derive the agent's base58 public key from a base58-encoded 64-byte keypair.
fn agent_pubkey_b58(keypair_b58: &str) -> Result<String> {
    let key_bytes = bs58::decode(keypair_b58)
        .into_vec()
        .context("failed to decode keypair from base58")?;
    if key_bytes.len() != 64 {
        anyhow::bail!(
            "keypair must be 64 bytes (seed || pubkey), got {}",
            key_bytes.len()
        );
    }
    let seed: [u8; 32] = key_bytes[..32]
        .try_into()
        .map_err(|_| anyhow::anyhow!("failed to extract seed from keypair"))?;
    let pubkey = ed25519_dalek::SigningKey::from_bytes(&seed).verifying_key();
    Ok(bs58::encode(pubkey.as_bytes()).into_string())
}

/// Real Anchor escrow deposit payment strategy for load testing.
///
/// Signs an escrow deposit transaction for each 402 response using the
/// provided keypair. The `rpc_client` is shared across all requests.
pub struct EscrowPaymentStrategy {
    keypair_b58: SecretString,
    rpc_client: reqwest::Client,
    rpc_url: String,
}

impl EscrowPaymentStrategy {
    /// Create a new `EscrowPaymentStrategy`.
    ///
    /// # Arguments
    /// * `keypair_b58` — 64-byte Solana keypair in base58, wrapped in `SecretString`
    /// * `rpc_client` — shared reqwest client for Solana RPC calls
    /// * `rpc_url` — Solana JSON-RPC endpoint URL
    pub fn new(keypair_b58: SecretString, rpc_client: reqwest::Client, rpc_url: String) -> Self {
        Self {
            keypair_b58,
            rpc_client,
            rpc_url,
        }
    }
}

impl fmt::Debug for EscrowPaymentStrategy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EscrowPaymentStrategy")
            .field("keypair_b58", &"[REDACTED]")
            .field("rpc_client", &self.rpc_client)
            .field("rpc_url", &self.rpc_url)
            .finish()
    }
}

#[async_trait::async_trait]
impl PaymentStrategy for EscrowPaymentStrategy {
    fn name(&self) -> &'static str {
        "escrow"
    }

    async fn prepare_payment(
        &self,
        _rpc_url: &str,
        request_body: &serde_json::Value,
        accepts: &[PaymentAccept],
    ) -> Result<Option<String>> {
        // Find the escrow-scheme entry from the 402 accepts list.
        let accepted = accepts
            .iter()
            .find(|a| a.scheme == "escrow" && a.escrow_program_id.is_some())
            .context("no 'escrow' payment scheme with program ID in 402 accepts")?
            .clone();

        let escrow_program_id = accepted
            .escrow_program_id
            .as_deref()
            .context("escrow scheme missing program ID")?;

        let amount: u64 = accepted
            .amount
            .parse()
            .context("invalid payment amount from gateway")?;

        // Generate a unique service_id for this request.
        let body_bytes =
            serde_json::to_vec(request_body).context("failed to serialize request body")?;
        let service_id = generate_service_id(&body_bytes)?;

        // Fetch current slot for expiry calculation.
        let current_slot = fetch_current_slot(&self.rpc_url, &self.rpc_client)
            .await
            .context("failed to fetch current slot for escrow expiry")?;
        let timeout_slots = (accepted.max_timeout_seconds * 1000) / 400;
        let expiry_slot = current_slot + timeout_slots;

        // Build and sign the escrow deposit transaction.
        let deposit_tx = build_escrow_deposit(
            self.keypair_b58.expose_secret(),
            &accepted.pay_to,
            escrow_program_id,
            amount,
            service_id,
            expiry_slot,
            &self.rpc_url,
            &self.rpc_client,
        )
        .await
        .context("failed to build escrow deposit transaction")?;

        // Derive the agent pubkey for the escrow payload.
        let agent_pubkey = agent_pubkey_b58(self.keypair_b58.expose_secret())
            .context("failed to derive agent pubkey from keypair")?;

        // Assemble the PaymentPayload with escrow variant.
        let payment_payload = PaymentPayload {
            x402_version: X402_VERSION,
            resource: Resource {
                url: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
            },
            accepted,
            payload: PayloadData::Escrow(EscrowPayload {
                deposit_tx,
                service_id: BASE64.encode(service_id),
                agent_pubkey,
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

    // --- Escrow payment strategy tests ---

    /// Helper to build a valid base58 keypair for tests.
    fn make_test_keypair_b58() -> String {
        let seed = [42u8; 32];
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
        let verifying_key = signing_key.verifying_key();
        let mut full_key = [0u8; 64];
        full_key[..32].copy_from_slice(&seed);
        full_key[32..].copy_from_slice(verifying_key.as_bytes());
        bs58::encode(&full_key).into_string()
    }

    #[test]
    fn test_escrow_payment_name() {
        let strategy = EscrowPaymentStrategy::new(
            SecretString::new("key".to_string()),
            reqwest::Client::new(),
            "http://localhost:8899".to_string(),
        );
        assert_eq!(strategy.name(), "escrow");
    }

    #[test]
    fn test_escrow_payment_debug_redacts_keypair() {
        let strategy = EscrowPaymentStrategy::new(
            SecretString::new("super-secret-escrow-key".to_string()),
            reqwest::Client::new(),
            "http://localhost:8899".to_string(),
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
        assert!(
            debug_output.contains("localhost:8899"),
            "Debug output should show rpc_url"
        );
    }

    #[tokio::test]
    async fn test_escrow_payment_no_escrow_scheme_returns_error() {
        let strategy = EscrowPaymentStrategy::new(
            SecretString::new("key".to_string()),
            reqwest::Client::new(),
            "http://localhost:8899".to_string(),
        );
        // Provide only an exact accept — no escrow scheme available.
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
            .prepare_payment("http://localhost:8899", &serde_json::json!({}), &accepts)
            .await;

        assert!(result.is_err(), "should error when no escrow scheme found");
        assert!(
            result.unwrap_err().to_string().contains("escrow"),
            "error should mention 'escrow'"
        );
    }

    #[tokio::test]
    async fn test_escrow_payment_missing_program_id_returns_error() {
        let strategy = EscrowPaymentStrategy::new(
            SecretString::new("key".to_string()),
            reqwest::Client::new(),
            "http://localhost:8899".to_string(),
        );
        // Escrow scheme but without program ID.
        let accepts = vec![PaymentAccept {
            scheme: "escrow".to_string(),
            network: "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp".to_string(),
            amount: "1000".to_string(),
            asset: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            pay_to: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM".to_string(),
            max_timeout_seconds: 300,
            escrow_program_id: None,
        }];

        let result = strategy
            .prepare_payment("http://localhost:8899", &serde_json::json!({}), &accepts)
            .await;

        assert!(
            result.is_err(),
            "should error when escrow scheme has no program ID"
        );
    }

    #[test]
    fn test_generate_service_id_length() {
        let id = generate_service_id(b"test request body").expect("should generate service_id");
        assert_eq!(id.len(), 32);
    }

    #[test]
    fn test_generate_service_id_unique_with_nonce() {
        let id1 = generate_service_id(b"same body").expect("first id");
        let id2 = generate_service_id(b"same body").expect("second id");
        assert_ne!(id1, id2, "service IDs should differ due to random nonce");
    }

    #[test]
    fn test_agent_pubkey_b58_valid_keypair() {
        let keypair = make_test_keypair_b58();
        let pubkey = agent_pubkey_b58(&keypair).expect("should derive pubkey");
        // Verify the pubkey matches the verifying key.
        let seed = [42u8; 32];
        let expected = ed25519_dalek::SigningKey::from_bytes(&seed).verifying_key();
        let expected_b58 = bs58::encode(expected.as_bytes()).into_string();
        assert_eq!(pubkey, expected_b58);
    }

    #[test]
    fn test_agent_pubkey_b58_invalid_keypair() {
        let short_key = bs58::encode(&[0u8; 32]).into_string();
        assert!(agent_pubkey_b58(&short_key).is_err());
    }

    #[tokio::test]
    async fn test_escrow_payment_builds_valid_header() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // Start a mock Solana RPC server that handles both getSlot and getLatestBlockhash.
        let mock_rpc = MockServer::start().await;

        // The escrow strategy makes two RPC calls: getSlot then getLatestBlockhash.
        // wiremock responds to all POST / with the same body, so we use a
        // respond_with closure that returns valid responses for both.
        // Since wiremock doesn't easily distinguish by JSON body, we provide
        // a response that satisfies both parsers (getSlot reads "result" as u64,
        // getLatestBlockhash reads "result.value.blockhash").
        // We mount them in sequence: getSlot first, then getLatestBlockhash.
        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": 100_u64
            })))
            .up_to_n_times(1)
            .expect(1)
            .mount(&mock_rpc)
            .await;

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
            .expect(1)
            .mount(&mock_rpc)
            .await;

        let keypair_b58 = make_test_keypair_b58();

        let strategy = EscrowPaymentStrategy::new(
            SecretString::new(keypair_b58.clone()),
            reqwest::Client::new(),
            mock_rpc.uri(),
        );

        let accepts = vec![PaymentAccept {
            scheme: "escrow".to_string(),
            network: "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp".to_string(),
            amount: "1000".to_string(),
            asset: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            pay_to: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM".to_string(),
            max_timeout_seconds: 300,
            escrow_program_id: Some("9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU".to_string()),
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
            "escrow payment should succeed: {:?}",
            result.err()
        );
        let header_value = result
            .expect("already checked is_ok")
            .expect("escrow strategy should return Some");

        // Decode the header to verify it's valid base64 -> JSON -> PaymentPayload.
        let decoded_bytes = BASE64
            .decode(&header_value)
            .expect("header should be valid base64");
        let payload: PaymentPayload = serde_json::from_slice(&decoded_bytes)
            .expect("decoded header should be valid PaymentPayload JSON");

        assert_eq!(payload.x402_version, X402_VERSION);
        assert_eq!(payload.resource.url, "/v1/chat/completions");
        assert_eq!(payload.resource.method, "POST");
        assert_eq!(payload.accepted.scheme, "escrow");
        assert_eq!(payload.accepted.amount, "1000");
        assert!(
            matches!(payload.payload, PayloadData::Escrow(_)),
            "payload should be Escrow variant"
        );

        // Verify the escrow payload fields.
        if let PayloadData::Escrow(ref ep) = payload.payload {
            assert!(!ep.deposit_tx.is_empty(), "deposit_tx should not be empty");
            assert!(!ep.service_id.is_empty(), "service_id should not be empty");
            assert!(
                !ep.agent_pubkey.is_empty(),
                "agent_pubkey should not be empty"
            );
            // Verify agent_pubkey matches our keypair.
            let expected_pubkey = agent_pubkey_b58(&keypair_b58).expect("should derive pubkey");
            assert_eq!(ep.agent_pubkey, expected_pubkey);
        }
    }
}
