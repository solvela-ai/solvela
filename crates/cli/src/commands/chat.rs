use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use x402::types::{PaymentPayload, PaymentRequired, Resource, SolanaPayload};

use crate::commands::wallet::load_wallet;

pub async fn run(api_url: &str, model: &str, prompt: &str, yes: bool) -> Result<()> {
    let client = reqwest::Client::new();

    let body = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
    });

    let endpoint_url = format!("{}/v1/chat/completions", api_url);

    // First try without payment.
    let resp = client.post(&endpoint_url).json(&body).send().await?;

    if resp.status().is_success() {
        let resp_body: serde_json::Value = resp.json().await?;
        if let Some(content) = resp_body["choices"][0]["message"]["content"].as_str() {
            println!("{}", content);
        } else {
            eprintln!("Warning: response contained no text content");
            eprintln!(
                "Raw response: {}",
                serde_json::to_string_pretty(&resp_body)
                    .unwrap_or_else(|e| format!("<serialization failed: {e}>"))
            );
        }
        return Ok(());
    }

    if resp.status().as_u16() != 402 {
        let status = resp.status();
        let text = resp.text().await?;
        return Err(anyhow::anyhow!("Gateway error {}: {}", status, text));
    }

    // --- 402 Payment Required ---
    let error_body: serde_json::Value = resp.json().await?;
    let error_msg = error_body["error"]["message"].as_str().unwrap_or("");

    let payment_required: PaymentRequired = serde_json::from_str(error_msg)
        .context("failed to parse PaymentRequired from 402 response")?;

    // Show cost breakdown.
    let cb = &payment_required.cost_breakdown;
    println!("Cost breakdown:");
    println!("  Provider cost : {} {}", cb.provider_cost, cb.currency);
    println!(
        "  Platform fee  : {} {} ({}%)",
        cb.platform_fee, cb.currency, cb.fee_percent
    );
    println!("  Total         : {} {}", cb.total, cb.currency);

    // Confirm unless --yes was passed.
    if !yes {
        print!("Proceed? [y/N] ");
        use std::io::Write;
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !matches!(input.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Aborted.");
            return Ok(());
        }
    }

    // Load wallet.
    let wallet = load_wallet()?;
    let private_key_b58 = wallet["private_key"]
        .as_str()
        .context("wallet missing private_key field")?;

    // Take the first accepted payment method.
    let accepted = payment_required
        .accepts
        .into_iter()
        .next()
        .context("gateway returned no accepted payment methods")?;

    // Resolve the Solana RPC URL from the environment.
    let rpc_url = std::env::var("SOLANA_RPC_URL")
        .or_else(|_| std::env::var("RCR_SOLANA_RPC_URL"))
        .map_err(|_| {
            anyhow::anyhow!(
                "SOLANA_RPC_URL required for payment signing. \
                 Set it to your Solana RPC endpoint (e.g. https://api.mainnet-beta.solana.com)."
            )
        })?;

    // Build and sign a real USDC-SPL TransferChecked transaction.
    let signed_tx = crate::commands::solana_tx::build_usdc_transfer(
        private_key_b58,
        &accepted.pay_to,
        accepted
            .amount
            .parse::<u64>()
            .context("invalid payment amount from gateway")?,
        &rpc_url,
        &client,
    )
    .await
    .context("failed to build Solana payment transaction")?;

    // Build the PaymentPayload.
    let payment_payload = PaymentPayload {
        x402_version: x402::types::X402_VERSION,
        resource: Resource {
            url: endpoint_url.clone(),
            method: "POST".to_string(),
        },
        accepted,
        payload: x402::types::PayloadData::Direct(SolanaPayload {
            transaction: signed_tx,
        }),
    };

    // Encode as base64(JSON(payload)).
    let payload_json = serde_json::to_string(&payment_payload)?;
    let payment_header = BASE64.encode(payload_json.as_bytes());

    // Retry with the payment header.
    let retry_resp = client
        .post(&endpoint_url)
        .header("PAYMENT-SIGNATURE", &payment_header)
        .json(&body)
        .send()
        .await?;

    if retry_resp.status().is_success() {
        let resp_body: serde_json::Value = retry_resp.json().await?;
        if let Some(content) = resp_body["choices"][0]["message"]["content"].as_str() {
            println!("{}", content);
        } else {
            eprintln!("Warning: response contained no text content");
            eprintln!(
                "Raw response: {}",
                serde_json::to_string_pretty(&resp_body)
                    .unwrap_or_else(|e| format!("<serialization failed: {e}>"))
            );
        }
    } else {
        let status = retry_resp.status();
        let text = retry_resp.text().await?;
        return Err(anyhow::anyhow!(
            "Payment submitted but gateway returned error {}: {}",
            status,
            text
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header_exists, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// RAII guard that removes an env var on drop (panic-safe cleanup).
    struct EnvGuard(&'static str);
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            std::env::remove_var(self.0);
        }
    }

    /// Bind a TCP listener to get an OS-assigned port, then drop it.
    /// The returned URL will be connection-refused immediately (ECONNREFUSED).
    fn dead_url() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        drop(listener);
        format!("http://127.0.0.1:{port}")
    }

    /// Create a temp home with a valid wallet for payment tests.
    fn setup_wallet() -> tempfile::TempDir {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        std::env::set_var("HOME", tmp.path());
        let dir = tmp.path().join(".rustyclawrouter");
        std::fs::create_dir_all(&dir).expect("mkdir");

        // Generate a real keypair for the wallet
        let mut seed = [0u8; 32];
        seed[0] = 42; // deterministic for tests
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
        let verifying_key = signing_key.verifying_key();
        let mut full_key = [0u8; 64];
        full_key[..32].copy_from_slice(&seed);
        full_key[32..].copy_from_slice(verifying_key.as_bytes());

        let wallet = serde_json::json!({
            "private_key": bs58::encode(&full_key).into_string(),
            "address": bs58::encode(verifying_key.as_bytes()).into_string(),
            "created_at": "2026-01-01T00:00:00Z"
        });
        std::fs::write(
            dir.join("wallet.json"),
            serde_json::to_string_pretty(&wallet).expect("json"),
        )
        .expect("write wallet");
        tmp
    }

    #[tokio::test]
    async fn test_chat_free_response() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": "Hello! Solana is a blockchain."}}]
            })))
            .mount(&mock)
            .await;

        let result = run(&mock.uri(), "auto", "What is Solana?", true).await;
        assert!(result.is_ok(), "chat should succeed on 200 response");
    }

    #[tokio::test]
    async fn test_chat_server_error() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(500).set_body_string("internal error"))
            .mount(&mock)
            .await;

        let result = run(&mock.uri(), "auto", "test", true).await;
        assert!(result.is_err(), "chat should return error on 500 response");
        assert!(
            result.unwrap_err().to_string().contains("Gateway error"),
            "error message should mention gateway error"
        );
    }

    #[tokio::test]
    async fn test_chat_402_payment_flow() {
        // Hold the async mutex for the full test to prevent HOME from being
        // clobbered by another test while load_wallet() reads it.
        let _lock = crate::ENV_MUTEX.lock().await;
        let _wallet = setup_wallet();

        // One mock server handles both the gateway and the Solana RPC
        // (all distinguished by path or method+body).
        let mock = MockServer::start().await;

        // Mock the Solana RPC getLatestBlockhash call.
        // The system blockhash (all zeros) base58-encodes to 32 '1' characters.
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
            .mount(&mock)
            .await;

        let payment_required = serde_json::json!({
            "x402_version": 2,
            "resource": {"url": "/v1/chat/completions", "method": "POST"},
            "accepts": [{
                "scheme": "exact",
                "network": "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
                "amount": "1000",
                "asset": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                "pay_to": "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM",
                "max_timeout_seconds": 300
            }],
            "cost_breakdown": {
                "provider_cost": "0.001000",
                "platform_fee": "0.000050",
                "fee_percent": 5,
                "total": "0.001050",
                "currency": "USDC"
            },
            "error": "Payment required"
        });

        // Mount 402 first (lower priority in wiremock — last mounted wins)
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(402).set_body_json(serde_json::json!({
                "error": {
                    "message": serde_json::to_string(&payment_required).expect("serialize PR")
                }
            })))
            .up_to_n_times(1)
            .mount(&mock)
            .await;

        // Mount paid response last (higher priority — last mounted wins in wiremock 0.6)
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header_exists("PAYMENT-SIGNATURE"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": "Paid response!"}}]
            })))
            .mount(&mock)
            .await;

        // Point SOLANA_RPC_URL at the mock server root (same server, path "/").
        std::env::set_var("SOLANA_RPC_URL", &mock.uri());
        let _env_guard = EnvGuard("SOLANA_RPC_URL");

        let result = run(&mock.uri(), "auto", "What is Solana?", true).await;

        assert!(
            result.is_ok(),
            "chat payment flow should succeed with --yes"
        );
    }

    #[tokio::test]
    async fn test_chat_402_no_wallet_returns_error() {
        // Hold the async mutex for the full test to prevent HOME from being
        // clobbered by another test while load_wallet() reads it.
        let _lock = crate::ENV_MUTEX.lock().await;
        let tmp = tempfile::TempDir::new().expect("tempdir");
        std::env::set_var("HOME", tmp.path());

        let mock = MockServer::start().await;
        let payment_required = serde_json::json!({
            "x402_version": 2,
            "resource": {"url": "/v1/chat/completions", "method": "POST"},
            "accepts": [{
                "scheme": "exact",
                "network": "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
                "amount": "1000",
                "asset": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                "pay_to": "TestRecipient11111111111111111111111111111",
                "max_timeout_seconds": 300
            }],
            "cost_breakdown": {
                "provider_cost": "0.001000",
                "platform_fee": "0.000050",
                "fee_percent": 5,
                "total": "0.001050",
                "currency": "USDC"
            },
            "error": "Payment required"
        });

        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(402).set_body_json(serde_json::json!({
                "error": {
                    "message": serde_json::to_string(&payment_required).expect("serialize")
                }
            })))
            .mount(&mock)
            .await;

        let result = run(&mock.uri(), "auto", "test", true).await;
        assert!(
            result.is_err(),
            "chat should fail when wallet is missing for payment"
        );
    }

    #[tokio::test]
    async fn test_chat_connection_error() {
        let result = run(&dead_url(), "auto", "test", true).await;
        assert!(result.is_err(), "chat should error on connection failure");
    }
}
