use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use secrecy::ExposeSecret;
use sha2::{Digest, Sha256};
use solvela_x402::types::{
    PaymentAccept, PaymentPayload, PaymentRequired, Resource, SolanaPayload, SOLANA_NETWORK,
    USDC_MINT,
};
use zeroize::Zeroizing;

use crate::commands::wallet::load_wallet;

/// Returns true if an accept entry is payable by this CLI: the canonical
/// Solana network AND the canonical USDC mint.
///
/// The CLI pins both client-side (matching all four SDK signers) so a
/// malicious or misconfigured gateway advertising a foreign asset or network
/// can never steer the wallet into signing a transfer of the wrong token —
/// fail closed, never fall back to "first advertised entry".
fn is_payable_accept(accept: &PaymentAccept) -> bool {
    accept.network == SOLANA_NETWORK && accept.asset == USDC_MINT
}

/// Select the preferred payment scheme from the accepts list.
///
/// Only entries on the canonical Solana network with the canonical USDC mint
/// are eligible (see [`is_payable_accept`]). If `override_scheme` is `Some`,
/// an eligible entry with that scheme name must be present or an error is
/// returned. Otherwise the default preference is used: "escrow" (when a
/// program ID is advertised) over "exact".
fn select_payment_scheme<'a>(
    accepts: &'a [PaymentAccept],
    override_scheme: Option<&str>,
) -> Result<&'a PaymentAccept> {
    if let Some(scheme) = override_scheme {
        let mut candidates = accepts.iter().filter(|a| a.scheme == scheme).peekable();
        if candidates.peek().is_none() {
            anyhow::bail!("requested scheme '{scheme}' not advertised by gateway");
        }
        return candidates.find(|a| is_payable_accept(a)).with_context(|| {
            format!(
                "requested scheme '{scheme}' is only advertised with an unsupported \
                 network/asset (expected Solana USDC mint {USDC_MINT}); refusing to sign"
            )
        });
    }
    accepts
        .iter()
        .find(|a| is_payable_accept(a) && a.scheme == "escrow" && a.escrow_program_id.is_some())
        .or_else(|| accepts.iter().find(|a| is_payable_accept(a)))
        .with_context(|| {
            format!(
                "gateway advertised no payment method on the Solana USDC mint {USDC_MINT}; \
                 refusing to sign"
            )
        })
}

/// Generate a unique 32-byte service_id by hashing the request body + random nonce.
///
/// Issue #118 security invariant: the nonce is load-bearing. Without it,
/// two identical request bodies would produce the same service_id →
/// same escrow PDA → same vault ATA, all derivable off-chain *before*
/// the deposit broadcasts (front-running ATA creation, confidentiality
/// leak via on-chain correlation of vault address to specific prompts).
/// The on-chain program treats `service_id` as opaque bytes, so this is
/// a purely off-chain discipline — do NOT remove the nonce mix here.
/// Regression test: `tests::test_generate_service_id_unique_with_nonce`.
/// See `crates/x402/src/escrow/AGENTS.md` "service_id MUST mix a
/// per-request CSPRNG nonce" for the parallel TS-side implementation.
fn generate_service_id(request_body: &[u8]) -> Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    hasher.update(request_body);
    let mut nonce = [0u8; 8];
    getrandom::fill(&mut nonce).context("getrandom failed to generate nonce")?;
    hasher.update(nonce);
    let hash = hasher.finalize();
    let mut id = [0u8; 32];
    id.copy_from_slice(&hash);
    Ok(id)
}

pub async fn run(
    api_url: &str,
    model: &str,
    prompt: &str,
    yes: bool,
    scheme: Option<&str>,
) -> Result<()> {
    // Single client serves the gateway round-trip (which proxies LLM
    // completions, ~30-90 s typical) and the Solana RPC calls
    // (`solana_tx::*` helpers; ~1-5 s typical). The 180 s safety-net
    // covers the slowest legitimate path. Without a timeout, a stalled
    // gateway or RPC hangs the process indefinitely after the keypair
    // has already been loaded and the transaction signed.
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(180))
        .build()
        .context("failed to build HTTP client")?;

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
    //
    // Accept BOTH 402 body shapes the gateway may emit (issue #217):
    //
    //   1. Envelope (legacy, current default):
    //      { "error": { "type": "invalid_payment",
    //                   "message": "<JSON PaymentRequired>" } }
    //
    //   2. Direct (x402-spec compliant; target shape post-#217):
    //      { "x402_version": 2, "resource": {...}, "accepts": [...],
    //        "cost_breakdown": {...}, "error": "Payment required" }
    //
    // Shape detection is duck-typed: a top-level `error.message` string
    // triggers the envelope path; otherwise we attempt direct deserialization.
    // This dual-shape support lets the gateway flip unilaterally without a
    // coordinated CLI release.
    let error_body: serde_json::Value = resp.json().await?;

    let payment_required: PaymentRequired = if let Some(error_msg) =
        error_body["error"]["message"].as_str()
    {
        // Shape 1: envelope.
        serde_json::from_str(error_msg)
            .context("failed to parse PaymentRequired from 402 envelope.message")?
    } else {
        // Shape 2: direct PaymentRequired at top level. The serde_json error
        // here is surfaced via `with_context` so a body that matches neither
        // shape gets a single clear "neither shape matched" message instead
        // of two cascading errors.
        serde_json::from_value(error_body.clone()).with_context(|| {
            format!(
                "gateway 402 body matched neither envelope shape (`error.message` with stringified JSON) \
                 nor direct PaymentRequired shape (top-level `x402_version` + `accepts`); raw body: {error_body}"
            )
        })?
    };

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

    // Load wallet. `expose_secret()` is the documented escape hatch for
    // SecretString — every site that pulls the raw key is greppable for
    // future reviewers.
    let wallet = load_wallet()?;
    let private_key_b58: &str = wallet.private_key.expose_secret();

    // Select the preferred payment scheme (escrow > exact, or override).
    let accepted = select_payment_scheme(&payment_required.accepts, scheme)?.clone();

    // Resolve the Solana RPC URL from the environment.
    let rpc_url = std::env::var("SOLANA_RPC_URL")
        .or_else(|_| std::env::var("SOLVELA_SOLANA_RPC_URL"))
        .or_else(|_| std::env::var("RCR_SOLANA_RPC_URL"))
        .map_err(|_| {
            anyhow::anyhow!(
                "SOLANA_RPC_URL required for payment signing. \
                 Set it to your Solana RPC endpoint (e.g. https://api.mainnet-beta.solana.com)."
            )
        })?;

    let body_bytes = serde_json::to_vec(&body).context("failed to serialize request body")?;

    // Build the PaymentPayload based on scheme.
    let payment_payload = match accepted.scheme.as_str() {
        "escrow" => {
            let escrow_program_id = accepted
                .escrow_program_id
                .as_deref()
                .context("escrow scheme missing program ID")?;
            let service_id = generate_service_id(&body_bytes)?;
            let current_slot = crate::commands::solana_tx::fetch_current_slot(&rpc_url, &client)
                .await
                .context("failed to fetch current slot for escrow expiry")?;
            let expiry_slot = crate::commands::util::escrow_expiry_slot(
                current_slot,
                accepted.max_timeout_seconds,
            );
            let deposit_tx = crate::commands::solana_tx::build_escrow_deposit(
                private_key_b58,
                &accepted.pay_to,
                escrow_program_id,
                accepted
                    .amount
                    .parse::<u64>()
                    .context("invalid payment amount from gateway")?,
                service_id,
                expiry_slot,
                &rpc_url,
                &client,
            )
            .await
            .context("failed to build escrow deposit transaction")?;

            // Derive agent pubkey from keypair. Both `key_bytes` (Vec<u8>
            // holding seed||pubkey) and `seed` ([u8; 32], the raw ed25519
            // secret scalar) are wrapped in `Zeroizing` so the buffers are
            // scrubbed when this scope exits. `agent_pubkey` is the
            // public verifying key — non-secret, no wrap needed.
            let key_bytes = Zeroizing::new(
                bs58::decode(private_key_b58)
                    .into_vec()
                    .context("keypair decode")?,
            );
            let mut seed = Zeroizing::new([0u8; 32]);
            seed.copy_from_slice(
                key_bytes
                    .get(..32)
                    .ok_or_else(|| anyhow::anyhow!("bad seed"))?,
            );
            let agent_pubkey = ed25519_dalek::SigningKey::from_bytes(&seed).verifying_key();
            let agent_pubkey_b58 = bs58::encode(agent_pubkey.as_bytes()).into_string();

            PaymentPayload {
                x402_version: solvela_x402::types::X402_VERSION,
                resource: Resource {
                    url: "/v1/chat/completions".to_string(),
                    method: "POST".to_string(),
                },
                accepted: accepted.clone(),
                payload: solvela_x402::types::PayloadData::Escrow(
                    solvela_x402::types::EscrowPayload {
                        deposit_tx,
                        service_id: BASE64.encode(service_id),
                        agent_pubkey: agent_pubkey_b58,
                    },
                ),
            }
        }
        _ => {
            // Exact / direct payment scheme.
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

            PaymentPayload {
                x402_version: solvela_x402::types::X402_VERSION,
                resource: Resource {
                    url: "/v1/chat/completions".to_string(),
                    method: "POST".to_string(),
                },
                accepted,
                payload: solvela_x402::types::PayloadData::Direct(SolanaPayload {
                    transaction: signed_tx,
                }),
            }
        }
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
    use wiremock::matchers::{body_string_contains, header_exists, method, path};
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
        let dir = tmp.path().join(".solvela");
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

        let result = run(&mock.uri(), "auto", "What is Solana?", true, None).await;
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

        let result = run(&mock.uri(), "auto", "test", true, None).await;
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
        std::env::set_var("SOLANA_RPC_URL", mock.uri());
        let _env_guard = EnvGuard("SOLANA_RPC_URL");

        let result = run(&mock.uri(), "auto", "What is Solana?", true, None).await;

        assert!(
            result.is_ok(),
            "chat payment flow should succeed with --yes"
        );
    }

    /// Issue #217: dual-shape 402 parser. The CLI must decode a 402 body
    /// emitted in the x402-spec-compliant direct shape
    /// `{ x402_version, accepts, cost_breakdown, … }` exactly the same as
    /// the legacy `{ error: { message: "<JSON>" } }` envelope. Mirror of
    /// `test_chat_402_payment_flow` with only the gateway's 402 body shape
    /// changed.
    #[tokio::test]
    async fn test_chat_402_payment_flow_direct_shape() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let _wallet = setup_wallet();

        let mock = MockServer::start().await;

        // Solana RPC getLatestBlockhash mock (same as envelope test).
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

        // Mount 402 with the DIRECT shape — body is the PaymentRequired
        // payload at the top level, no envelope wrapping.
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(402).set_body_json(payment_required))
            .up_to_n_times(1)
            .mount(&mock)
            .await;

        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header_exists("PAYMENT-SIGNATURE"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": "Paid response!"}}]
            })))
            .mount(&mock)
            .await;

        std::env::set_var("SOLANA_RPC_URL", mock.uri());
        let _env_guard = EnvGuard("SOLANA_RPC_URL");

        let result = run(&mock.uri(), "auto", "What is Solana?", true, None).await;

        assert!(
            result.is_ok(),
            "chat payment flow should succeed against a direct-shape 402: {:?}",
            result.err()
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

        let result = run(&mock.uri(), "auto", "test", true, None).await;
        assert!(
            result.is_err(),
            "chat should fail when wallet is missing for payment"
        );
    }

    #[tokio::test]
    async fn test_chat_connection_error() {
        let result = run(&dead_url(), "auto", "test", true, None).await;
        assert!(result.is_err(), "chat should error on connection failure");
    }

    // --- select_payment_scheme tests ---

    fn make_exact_accept() -> PaymentAccept {
        PaymentAccept {
            scheme: "exact".to_string(),
            network: "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp".to_string(),
            amount: "1000".to_string(),
            asset: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            pay_to: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM".to_string(),
            max_timeout_seconds: 300,
            escrow_program_id: None,
        }
    }

    fn make_escrow_accept() -> PaymentAccept {
        PaymentAccept {
            scheme: "escrow".to_string(),
            network: "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp".to_string(),
            amount: "1000".to_string(),
            asset: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            pay_to: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM".to_string(),
            max_timeout_seconds: 300,
            escrow_program_id: Some("9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU".to_string()),
        }
    }

    #[test]
    fn test_select_escrow_scheme_prefers_escrow() {
        let accepts = vec![make_exact_accept(), make_escrow_accept()];
        // Safe: non-empty accepts list always yields Ok
        let selected = select_payment_scheme(&accepts, None).unwrap();
        assert_eq!(selected.scheme, "escrow");
    }

    #[test]
    fn test_select_escrow_scheme_falls_back_to_exact() {
        let accepts = vec![make_exact_accept()];
        // Safe: non-empty accepts list always yields Ok
        let selected = select_payment_scheme(&accepts, None).unwrap();
        assert_eq!(selected.scheme, "exact");
    }

    #[test]
    fn test_select_escrow_scheme_skips_escrow_without_program_id() {
        let mut escrow_no_id = make_escrow_accept();
        escrow_no_id.escrow_program_id = None;
        let accepts = vec![make_exact_accept(), escrow_no_id];
        // Safe: non-empty accepts list always yields Ok
        let selected = select_payment_scheme(&accepts, None).unwrap();
        assert_eq!(
            selected.scheme, "exact",
            "escrow without program ID should be skipped"
        );
    }

    #[test]
    fn test_select_payment_scheme_with_override() {
        let accepts = vec![make_exact_accept(), make_escrow_accept()];
        // --scheme exact should select exact even when escrow is available
        let selected = select_payment_scheme(&accepts, Some("exact")).unwrap();
        assert_eq!(
            selected.scheme, "exact",
            "--scheme exact should override default escrow preference"
        );
    }

    #[test]
    fn test_select_payment_scheme_override_escrow() {
        let accepts = vec![make_exact_accept(), make_escrow_accept()];
        let selected = select_payment_scheme(&accepts, Some("escrow")).unwrap();
        assert_eq!(selected.scheme, "escrow");
    }

    #[test]
    fn test_select_payment_scheme_override_not_advertised() {
        let accepts = vec![make_exact_accept()];
        let result = select_payment_scheme(&accepts, Some("escrow"));
        assert!(
            result.is_err(),
            "requesting an unavailable scheme should return an error"
        );
        assert!(
            result.unwrap_err().to_string().contains("escrow"),
            "error should mention the requested scheme"
        );
    }

    /// An exact accept whose `asset` is NOT the canonical USDC mint (wSOL here).
    /// A malicious or buggy gateway advertising this first must never have it
    /// selected — the CLI pins the canonical Solana network + USDC mint
    /// client-side, matching the four SDK signers.
    fn make_foreign_asset_accept() -> PaymentAccept {
        PaymentAccept {
            asset: "So11111111111111111111111111111111111111112".to_string(),
            ..make_exact_accept()
        }
    }

    /// An exact accept on a foreign network (Ethereum mainnet CAIP-2 id).
    fn make_foreign_network_accept() -> PaymentAccept {
        PaymentAccept {
            network: "eip155:1".to_string(),
            ..make_exact_accept()
        }
    }

    #[test]
    fn test_select_payment_scheme_skips_foreign_mint_first_entry() {
        let accepts = vec![make_foreign_asset_accept(), make_exact_accept()];
        let selected = select_payment_scheme(&accepts, None).expect("valid USDC entry exists");
        assert_eq!(
            selected.asset, USDC_MINT,
            "foreign-mint entry listed first must be skipped in favor of the USDC entry"
        );
    }

    #[test]
    fn test_select_payment_scheme_skips_foreign_network_first_entry() {
        let accepts = vec![make_foreign_network_accept(), make_exact_accept()];
        let selected = select_payment_scheme(&accepts, None).expect("valid USDC entry exists");
        assert_eq!(selected.network, SOLANA_NETWORK);
    }

    #[test]
    fn test_select_payment_scheme_prefers_escrow_among_valid_entries() {
        // Escrow preference is kept, but only among canonical-mint entries.
        let accepts = vec![
            make_foreign_asset_accept(),
            make_exact_accept(),
            make_escrow_accept(),
        ];
        let selected = select_payment_scheme(&accepts, None).expect("valid entries exist");
        assert_eq!(selected.scheme, "escrow");
        assert_eq!(selected.asset, USDC_MINT);
    }

    #[test]
    fn test_select_payment_scheme_all_foreign_errors() {
        let mut foreign_escrow = make_escrow_accept();
        foreign_escrow.asset = "So11111111111111111111111111111111111111112".to_string();
        let accepts = vec![make_foreign_asset_accept(), foreign_escrow];
        let result = select_payment_scheme(&accepts, None);
        assert!(
            result.is_err(),
            "no canonical USDC entry must be a hard error, never a fallback selection"
        );
        assert!(
            result.unwrap_err().to_string().contains(USDC_MINT),
            "error should name the expected USDC mint"
        );
    }

    #[test]
    fn test_select_payment_scheme_override_foreign_mint_errors() {
        // --scheme exact, but the only exact entry carries a foreign mint.
        let accepts = vec![make_foreign_asset_accept(), make_escrow_accept()];
        let result = select_payment_scheme(&accepts, Some("exact"));
        assert!(
            result.is_err(),
            "--scheme override must not select a foreign-mint entry"
        );
    }

    #[test]
    fn test_generate_service_id_length() {
        // Safe: getrandom is available in the test environment
        let id = generate_service_id(b"test request body").unwrap();
        assert_eq!(id.len(), 32);
    }

    #[test]
    fn test_generate_service_id_unique_with_nonce() {
        // Safe: getrandom is available in the test environment
        let id1 = generate_service_id(b"same body").unwrap();
        let id2 = generate_service_id(b"same body").unwrap();
        assert_ne!(id1, id2, "service IDs should differ due to random nonce");
    }

    #[tokio::test]
    async fn test_chat_402_escrow_payment_flow() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let _wallet = setup_wallet();

        let mock = MockServer::start().await;

        // Mock getSlot RPC call.
        Mock::given(method("POST"))
            .and(path("/"))
            .and(body_string_contains("getSlot"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": 12345
            })))
            .mount(&mock)
            .await;

        // Mock getLatestBlockhash RPC call.
        Mock::given(method("POST"))
            .and(path("/"))
            .and(body_string_contains("getLatestBlockhash"))
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
                "scheme": "escrow",
                "network": "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
                "amount": "1000",
                "asset": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                "pay_to": "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM",
                "max_timeout_seconds": 300,
                "escrow_program_id": "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU"
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

        // Mount 402 first (lower priority).
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

        // Mount paid response last (higher priority).
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header_exists("PAYMENT-SIGNATURE"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": "Escrow paid response!"}}]
            })))
            .mount(&mock)
            .await;

        std::env::set_var("SOLANA_RPC_URL", mock.uri());
        let _env_guard = EnvGuard("SOLANA_RPC_URL");

        let result = run(&mock.uri(), "auto", "What is Solana?", true, None).await;

        assert!(
            result.is_ok(),
            "escrow payment flow should succeed: {:?}",
            result.err()
        );
    }
}
