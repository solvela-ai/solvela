use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};

use crate::commands::util::resolve_rpc_url as shared_resolve_rpc_url;

/// Lamports per SOL.
const LAMPORTS_PER_SOL: u64 = 1_000_000_000;

/// Maximum time we wait for an airdrop to confirm.
const AIRDROP_CONFIRM_TIMEOUT: Duration = Duration::from_secs(30);

/// Polling interval while waiting for airdrop confirmation.
const AIRDROP_POLL_INTERVAL: Duration = Duration::from_millis(1500);

fn wallet_dir() -> PathBuf {
    home_dir().join(".solvela")
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn wallet_file() -> PathBuf {
    wallet_dir().join("wallet.json")
}

/// Returns `true` if a wallet already exists at the default path.
pub(crate) fn wallet_exists() -> bool {
    wallet_file().exists()
}

/// Public path resolver — used by `init` for ready-summary output.
pub(crate) fn wallet_file_path() -> PathBuf {
    wallet_file()
}

/// Generate a new ed25519 keypair, persist it to the default wallet path with
/// 0o600 permissions on Unix, and return the base58 public address.
///
/// Returns `Err` if a wallet already exists. Used by both
/// `solvela wallet init` and `solvela init` to share key-generation logic.
pub(crate) fn generate_and_save_wallet() -> Result<String> {
    let dir = wallet_dir();
    fs::create_dir_all(&dir).context("failed to create wallet directory")?;

    if wallet_file().exists() {
        return Err(anyhow!(
            "wallet already exists at {}",
            wallet_file().display()
        ));
    }

    // Generate a real ed25519 keypair using the same library the gateway uses
    // for signature verification. The 32-byte secret scalar is the private
    // key; the corresponding 32-byte verifying key is the Solana public key.
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).context("failed to generate random seed")?;

    let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
    let verifying_key = signing_key.verifying_key();

    // Solana wallet convention: private key = seed || pubkey (64 bytes total).
    let mut full_key = [0u8; 64];
    full_key[..32].copy_from_slice(&seed);
    full_key[32..].copy_from_slice(verifying_key.as_bytes());
    let private_key_b58 = bs58::encode(&full_key).into_string();
    let address = bs58::encode(verifying_key.as_bytes()).into_string();

    let wallet_data = serde_json::json!({
        "private_key": private_key_b58,
        "address": address,
        "created_at": chrono::Utc::now().to_rfc3339(),
    });

    fs::write(wallet_file(), serde_json::to_string_pretty(&wallet_data)?)
        .context("failed to write wallet file")?;

    // Restrict file permissions to owner-only on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(wallet_file(), fs::Permissions::from_mode(0o600))?;
    }

    Ok(address)
}

pub async fn init() -> Result<()> {
    if wallet_exists() {
        println!("Wallet already exists at {}", wallet_file().display());
        println!("Use 'solvela wallet export' to view the private key.");
        return Ok(());
    }

    let address = generate_and_save_wallet()?;

    println!("Wallet created!");
    println!("Address:   {}", address);
    println!("Saved to:  {}", wallet_file().display());
    println!();
    println!("Fund this address with USDC-SPL on Solana to start using AI models.");
    println!("Solana devnet faucet: https://faucet.solana.com");
    Ok(())
}

pub async fn status(api_url: &str) -> Result<()> {
    let wallet = load_wallet()?;
    let address = wallet["address"].as_str().unwrap_or("unknown");

    println!("Wallet Address: {}", address);
    println!("Gateway:        {}", api_url);

    // Check gateway health
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;

    match client.get(format!("{}/health", api_url)).send().await {
        Ok(resp) if resp.status().is_success() => {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let gw_status = body["status"].as_str().unwrap_or("ok");
            let solana_rpc = body["solana_rpc"].as_str().unwrap_or("unknown");
            println!("Gateway Status: {}", gw_status);
            println!("Solana RPC:     {}", solana_rpc);
        }
        Ok(resp) => {
            println!("Gateway Status: error ({})", resp.status());
        }
        Err(e) => {
            println!("Gateway Status: unreachable ({})", e);
            println!("  Start the gateway with: cargo run -p gateway");
        }
    }

    println!();
    println!(
        "Tip: Check USDC balance at https://explorer.solana.com/address/{}",
        address
    );
    Ok(())
}

pub fn export() -> Result<()> {
    let wallet = load_wallet()?;
    let key = wallet["private_key"].as_str().unwrap_or("not found");
    eprintln!("WARNING: Keep this key secret! Anyone with this key controls your funds.");
    eprintln!();
    println!("{}", key);
    Ok(())
}

pub(crate) fn load_wallet() -> Result<serde_json::Value> {
    let path = wallet_file();
    if !path.exists() {
        anyhow::bail!(
            "No wallet found at {}.\nRun 'solvela wallet init' to create one.",
            path.display()
        );
    }
    let data = fs::read_to_string(&path).context("failed to read wallet file")?;
    serde_json::from_str(&data).context("wallet file is corrupted")
}

// ---------------------------------------------------------------------------
// Airdrop (devnet/testnet only)
// ---------------------------------------------------------------------------

/// Refuse airdrop unless the RPC URL is a known-safe environment.
///
/// A substring-block on "mainnet" is insufficient — third-party mainnet
/// endpoints (e.g. `rpc.ankr.com/solana`) would bypass it.  Instead we
/// maintain an explicit allow-list of safe-environment indicators and reject
/// anything that does not match.
fn assert_not_mainnet(rpc_url: &str) -> Result<()> {
    let lower = rpc_url.to_ascii_lowercase();
    let is_safe = lower.contains("devnet")
        || lower.contains("testnet")
        || lower.contains("localhost")
        || lower.contains("127.0.0.1")
        || lower.contains("0.0.0.0");
    if !is_safe {
        return Err(anyhow!(
            "refusing to request airdrop on an unrecognized RPC ({rpc_url}). \
             Only devnet/testnet/localhost endpoints are allowed."
        ));
    }
    Ok(())
}

/// JSON-RPC `requestAirdrop` — returns the transaction signature.
async fn request_airdrop(
    rpc_url: &str,
    wallet_b58: &str,
    lamports: u64,
    client: &reqwest::Client,
) -> Result<String> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "requestAirdrop",
        "params": [wallet_b58, lamports]
    });
    let resp = client
        .post(rpc_url)
        .json(&body)
        .send()
        .await
        .context("failed to connect to Solana RPC for requestAirdrop")?;
    let json: serde_json::Value = resp
        .json()
        .await
        .context("failed to parse requestAirdrop response")?;
    if let Some(err) = json.get("error") {
        return Err(anyhow!(
            "Solana RPC error: {}",
            serde_json::to_string(err).unwrap_or_default()
        ));
    }
    json["result"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("requestAirdrop response missing result signature"))
}

/// Poll `getSignatureStatuses` until the signature is confirmed/finalized
/// or a timeout elapses.
async fn confirm_signature(
    rpc_url: &str,
    signature: &str,
    client: &reqwest::Client,
) -> Result<bool> {
    let start = Instant::now();
    while start.elapsed() < AIRDROP_CONFIRM_TIMEOUT {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getSignatureStatuses",
            "params": [[signature], {"searchTransactionHistory": true}]
        });
        let resp = client.post(rpc_url).json(&body).send().await;
        if let Ok(r) = resp {
            if let Ok(json) = r.json::<serde_json::Value>().await {
                let status = &json["result"]["value"][0];
                if !status.is_null() {
                    let confirmation = status["confirmationStatus"].as_str().unwrap_or("");
                    if confirmation == "confirmed" || confirmation == "finalized" {
                        // Check that the tx didn't error.
                        if status["err"].is_null() {
                            return Ok(true);
                        } else {
                            return Err(anyhow!(
                                "airdrop transaction failed on-chain: {}",
                                status["err"]
                            ));
                        }
                    }
                }
            }
        }
        tokio::time::sleep(AIRDROP_POLL_INTERVAL).await;
    }
    Ok(false)
}

/// Fetch a wallet's SOL balance (lamports) via `getBalance`.
async fn fetch_sol_balance(
    rpc_url: &str,
    wallet_b58: &str,
    client: &reqwest::Client,
) -> Result<u64> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getBalance",
        "params": [wallet_b58, {"commitment": "confirmed"}]
    });
    let resp = client
        .post(rpc_url)
        .json(&body)
        .send()
        .await
        .context("failed to connect to Solana RPC for getBalance")?;
    let json: serde_json::Value = resp
        .json()
        .await
        .context("failed to parse getBalance response")?;
    json["result"]["value"]
        .as_u64()
        .ok_or_else(|| anyhow!("getBalance response missing result.value"))
}

/// Convert a SOL amount (f64) to lamports, rejecting negatives/NaN/overflow.
fn sol_to_lamports(sol: f64) -> Result<u64> {
    if !sol.is_finite() || sol <= 0.0 {
        return Err(anyhow!(
            "amount must be a positive number of SOL (got {sol})"
        ));
    }
    let lamports = sol * (LAMPORTS_PER_SOL as f64);
    if lamports > (u64::MAX as f64) {
        return Err(anyhow!("amount {sol} SOL overflows u64 lamports"));
    }
    Ok(lamports as u64)
}

/// Request a SOL airdrop on devnet/testnet, wait for confirmation, and print
/// the new balance.
///
/// TODO: USDC airdrop on devnet is not part of this command. Users need a
/// dedicated devnet USDC faucet (e.g. https://faucet.circle.com).
pub async fn airdrop(amount_sol: f64) -> Result<()> {
    let wallet = load_wallet()?;
    let address = wallet["address"]
        .as_str()
        .context("wallet missing address field")?;

    let rpc_url = shared_resolve_rpc_url();
    assert_not_mainnet(&rpc_url)?;

    let lamports = sol_to_lamports(amount_sol)?;

    println!("Requesting airdrop of {amount_sol} SOL ({lamports} lamports)...");
    println!("Wallet:    {address}");
    println!("RPC:       {rpc_url}");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?;

    let signature = request_airdrop(&rpc_url, address, lamports, &client).await?;
    println!("Signature: {signature}");
    println!("Waiting for confirmation (up to 30s)...");

    let confirmed = confirm_signature(&rpc_url, &signature, &client).await?;
    if !confirmed {
        return Err(anyhow!(
            "airdrop did not confirm within {}s — signature: {signature}",
            AIRDROP_CONFIRM_TIMEOUT.as_secs()
        ));
    }

    let new_lamports = fetch_sol_balance(&rpc_url, address, &client).await?;
    let new_sol = (new_lamports as f64) / (LAMPORTS_PER_SOL as f64);
    println!();
    println!("Airdrop confirmed!");
    println!("New balance: {new_sol} SOL ({new_lamports} lamports)");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// RAII guard that removes an env var on drop (panic-safe cleanup).
    struct EnvGuard(&'static str);
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            std::env::remove_var(self.0);
        }
    }

    /// Set HOME to a tempdir for the duration of a test.
    /// Returns the TempDir (must be kept alive for the test duration).
    fn with_temp_home() -> TempDir {
        let tmp = TempDir::new().expect("create tempdir");
        std::env::set_var("HOME", tmp.path());
        tmp
    }

    /// Create a temp home with a real wallet file.
    fn setup_wallet() -> TempDir {
        let tmp = with_temp_home();
        let dir = tmp.path().join(".solvela");
        fs::create_dir_all(&dir).expect("mkdir");
        let mut seed = [0u8; 32];
        seed[0] = 42;
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
        fs::write(
            dir.join("wallet.json"),
            serde_json::to_string_pretty(&wallet).expect("json"),
        )
        .expect("write wallet");
        tmp
    }

    #[tokio::test]
    async fn test_wallet_file_path_uses_home() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let _tmp = with_temp_home();
        let path = wallet_file();
        assert!(
            path.to_str().unwrap().contains(".solvela/wallet.json"),
            "wallet path should include .solvela/wallet.json"
        );
    }

    #[tokio::test]
    async fn test_load_wallet_missing_file_returns_error() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let _tmp = with_temp_home();
        let result = load_wallet();
        assert!(
            result.is_err(),
            "load_wallet should fail when no wallet exists"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("No wallet found"),
            "error should mention no wallet found, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_load_wallet_corrupted_file_returns_error() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let _tmp = with_temp_home();
        let dir = wallet_dir();
        fs::create_dir_all(&dir).expect("create dir");
        fs::write(wallet_file(), "not json").expect("write file");

        let result = load_wallet();
        assert!(result.is_err(), "load_wallet should fail on corrupt JSON");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("corrupted"),
            "error should mention corruption, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_load_wallet_valid_file() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let _tmp = with_temp_home();
        let dir = wallet_dir();
        fs::create_dir_all(&dir).expect("create dir");
        fs::write(
            wallet_file(),
            r#"{"private_key":"abc","address":"def","created_at":"2026-01-01T00:00:00Z"}"#,
        )
        .expect("write file");

        let wallet = load_wallet().expect("load should succeed");
        assert_eq!(wallet["address"], "def");
        assert_eq!(wallet["private_key"], "abc");
    }

    #[tokio::test]
    async fn test_wallet_init_creates_file() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let _tmp = with_temp_home();
        assert!(!wallet_file().exists(), "wallet should not exist yet");

        init().await.expect("init should succeed");

        assert!(
            wallet_file().exists(),
            "wallet file should exist after init"
        );

        let wallet = load_wallet().expect("should load created wallet");
        let address = wallet["address"].as_str().expect("address field");
        assert!(!address.is_empty(), "address should not be empty");
        assert!(
            wallet["private_key"].as_str().is_some(),
            "private_key should be present"
        );
    }

    #[tokio::test]
    async fn test_wallet_init_idempotent() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let _tmp = with_temp_home();
        init().await.expect("first init");
        let wallet1 = load_wallet().expect("load after first init");

        // Second init should not overwrite
        init().await.expect("second init");
        let wallet2 = load_wallet().expect("load after second init");

        assert_eq!(
            wallet1["address"], wallet2["address"],
            "second init should not overwrite existing wallet"
        );
    }

    #[tokio::test]
    async fn test_wallet_export_returns_private_key() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let _tmp = with_temp_home();
        let dir = wallet_dir();
        fs::create_dir_all(&dir).expect("create dir");
        fs::write(
            wallet_file(),
            r#"{"private_key":"test_secret_key_b58","address":"test_addr","created_at":"2026-01-01T00:00:00Z"}"#,
        )
        .expect("write file");

        // export() prints to stdout — we just verify it doesn't error
        export().expect("export should succeed");
    }

    #[tokio::test]
    async fn test_wallet_init_generates_valid_keypair() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let _tmp = with_temp_home();
        init().await.expect("init");

        let wallet = load_wallet().expect("load");
        let address = wallet["address"].as_str().expect("address");
        let private_key = wallet["private_key"].as_str().expect("private_key");

        // Address should be base58 (32-44 chars for Solana pubkeys)
        assert!(
            address.len() >= 32 && address.len() <= 44,
            "address should be valid base58 length"
        );

        // Private key should be base58 (64 bytes = ~87 chars)
        assert!(
            private_key.len() > 60,
            "private key should be base58-encoded 64 bytes"
        );

        // Verify the keypair is consistent: decode private key, derive pubkey, compare to address
        let key_bytes = bs58::decode(private_key)
            .into_vec()
            .expect("decode private key");
        assert_eq!(key_bytes.len(), 64, "private key should be 64 bytes");
        let derived_address = bs58::encode(&key_bytes[32..]).into_string();
        assert_eq!(
            derived_address, address,
            "address should match pubkey from private key"
        );
    }

    // --- Airdrop tests ---

    // --- assert_not_mainnet allow-list tests ---

    #[test]
    fn test_assert_not_mainnet_allows_devnet() {
        assert!(assert_not_mainnet("https://api.devnet.solana.com").is_ok());
    }

    #[test]
    fn test_assert_not_mainnet_allows_testnet() {
        assert!(assert_not_mainnet("https://api.testnet.solana.com").is_ok());
    }

    #[test]
    fn test_assert_not_mainnet_allows_localhost() {
        assert!(assert_not_mainnet("http://localhost:8899").is_ok());
    }

    #[test]
    fn test_assert_not_mainnet_allows_127_0_0_1() {
        assert!(assert_not_mainnet("http://127.0.0.1:8899").is_ok());
    }

    #[test]
    fn test_assert_not_mainnet_rejects_mainnet_beta() {
        let result = assert_not_mainnet("https://api.mainnet-beta.solana.com");
        assert!(result.is_err(), "mainnet-beta should be rejected");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unrecognized RPC"),
            "error message should mention unrecognized RPC, got: {err}"
        );
    }

    /// HIGH-1: third-party mainnet endpoints must be blocked even though the
    /// URL does not contain the word "mainnet".
    #[test]
    fn test_assert_not_mainnet_rejects_ankr_mainnet() {
        let result = assert_not_mainnet("https://rpc.ankr.com/solana");
        assert!(
            result.is_err(),
            "rpc.ankr.com/solana (mainnet) should be rejected"
        );
    }

    #[test]
    fn test_assert_not_mainnet_rejects_1rpc() {
        let result = assert_not_mainnet("https://1rpc.io/sol");
        assert!(result.is_err(), "1rpc.io/sol should be rejected");
    }

    #[test]
    fn test_assert_not_mainnet_rejects_empty_string() {
        let result = assert_not_mainnet("");
        assert!(result.is_err(), "empty string should be rejected");
    }

    /// A URL that contains "devnet" as a subdomain should still be allowed.
    #[test]
    fn test_assert_not_mainnet_allows_devnet_subdomain() {
        assert!(assert_not_mainnet("https://devnet.something.io/solana").is_ok());
    }

    #[test]
    fn test_sol_to_lamports_rejects_zero() {
        assert!(sol_to_lamports(0.0).is_err());
    }

    #[test]
    fn test_sol_to_lamports_rejects_negative() {
        assert!(sol_to_lamports(-1.0).is_err());
    }

    #[test]
    fn test_sol_to_lamports_rejects_nan() {
        assert!(sol_to_lamports(f64::NAN).is_err());
    }

    #[test]
    fn test_sol_to_lamports_rejects_infinity() {
        assert!(sol_to_lamports(f64::INFINITY).is_err());
    }

    #[test]
    fn test_sol_to_lamports_basic() {
        assert_eq!(sol_to_lamports(1.0).unwrap(), 1_000_000_000);
        assert_eq!(sol_to_lamports(0.5).unwrap(), 500_000_000);
        assert_eq!(sol_to_lamports(2.5).unwrap(), 2_500_000_000);
    }

    #[tokio::test]
    async fn test_airdrop_refuses_mainnet() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let _wallet = setup_wallet();
        std::env::set_var(
            "SOLVELA_SOLANA_RPC_URL",
            "https://api.mainnet-beta.solana.com",
        );
        let _guard = EnvGuard("SOLVELA_SOLANA_RPC_URL");
        let result = airdrop(1.0).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("mainnet"),
            "expected mainnet refusal, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_airdrop_no_wallet_fails() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let _tmp = with_temp_home();
        let result = airdrop(1.0).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No wallet found"));
    }

    #[tokio::test]
    async fn test_airdrop_success_flow() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let _wallet = setup_wallet();

        let mock = MockServer::start().await;

        // requestAirdrop returns a fake signature
        Mock::given(method("POST"))
            .and(path("/"))
            .and(body_string_contains("requestAirdrop"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": "fakeSignature123"
            })))
            .mount(&mock)
            .await;

        // getSignatureStatuses immediately reports confirmed status
        Mock::given(method("POST"))
            .and(path("/"))
            .and(body_string_contains("getSignatureStatuses"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "value": [{
                        "slot": 12345,
                        "confirmations": 1,
                        "err": null,
                        "confirmationStatus": "confirmed"
                    }]
                }
            })))
            .mount(&mock)
            .await;

        // getBalance reports the new balance
        Mock::given(method("POST"))
            .and(path("/"))
            .and(body_string_contains("getBalance"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {"value": 1_000_000_000u64}
            })))
            .mount(&mock)
            .await;

        std::env::set_var("SOLVELA_SOLANA_RPC_URL", mock.uri());
        let _guard = EnvGuard("SOLVELA_SOLANA_RPC_URL");

        let result = airdrop(1.0).await;
        assert!(result.is_ok(), "airdrop should succeed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_airdrop_rpc_error_propagates() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let _wallet = setup_wallet();

        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .and(body_string_contains("requestAirdrop"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "error": {"code": -32600, "message": "Airdrop quota exceeded"}
            })))
            .mount(&mock)
            .await;

        std::env::set_var("SOLVELA_SOLANA_RPC_URL", mock.uri());
        let _guard = EnvGuard("SOLVELA_SOLANA_RPC_URL");

        let result = airdrop(1.0).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("quota") || err.contains("RPC"),
            "expected RPC error, got: {err}"
        );
    }
}
