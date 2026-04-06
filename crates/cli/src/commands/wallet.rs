use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

fn wallet_dir() -> PathBuf {
    home_dir().join(".rustyclawrouter")
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn wallet_file() -> PathBuf {
    wallet_dir().join("wallet.json")
}

pub async fn init() -> Result<()> {
    let dir = wallet_dir();
    fs::create_dir_all(&dir).context("failed to create wallet directory")?;

    if wallet_file().exists() {
        println!("Wallet already exists at {}", wallet_file().display());
        println!("Use 'rcr wallet export' to view the private key.");
        return Ok(());
    }

    // Generate a real ed25519 keypair using the same library the gateway uses
    // for signature verification. The 32-byte secret scalar is the private key;
    // the corresponding 32-byte verifying key is the Solana public key.
    let mut seed = [0u8; 32];
    getrandom::getrandom(&mut seed).context("failed to generate random seed")?;

    let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
    let verifying_key = signing_key.verifying_key();

    // Solana wallet convention: private key = seed || pubkey bytes (64 bytes total), base58-encoded
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

    // Restrict file permissions to owner-only on Unix
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(wallet_file(), fs::Permissions::from_mode(0o600))?;
    }

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
        .timeout(std::time::Duration::from_secs(5))
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
            "No wallet found at {}.\nRun 'rcr wallet init' to create one.",
            path.display()
        );
    }
    let data = fs::read_to_string(&path).context("failed to read wallet file")?;
    serde_json::from_str(&data).context("wallet file is corrupted")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Set HOME to a tempdir for the duration of a test.
    /// Returns the TempDir (must be kept alive for the test duration).
    fn with_temp_home() -> TempDir {
        let tmp = TempDir::new().expect("create tempdir");
        std::env::set_var("HOME", tmp.path());
        tmp
    }

    #[tokio::test]
    async fn test_wallet_file_path_uses_home() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let _tmp = with_temp_home();
        let path = wallet_file();
        assert!(
            path.to_str()
                .unwrap()
                .contains(".rustyclawrouter/wallet.json"),
            "wallet path should include .rustyclawrouter/wallet.json"
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
}
