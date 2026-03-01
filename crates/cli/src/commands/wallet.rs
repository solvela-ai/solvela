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
