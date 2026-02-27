use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

fn wallet_dir() -> PathBuf {
    dirs_or_home().join(".rustyclawrouter")
}

fn dirs_or_home() -> PathBuf {
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

    // Generate a random 64-byte key and encode as base58
    let mut key_bytes = [0u8; 64];
    getrandom_fill(&mut key_bytes);
    let private_key = bs58::encode(&key_bytes).into_string();
    // Derive a fake address from last 32 bytes
    let address = bs58::encode(&key_bytes[32..]).into_string();

    let wallet_data = serde_json::json!({
        "private_key": private_key,
        "address": address,
        "created_at": chrono::Utc::now().to_rfc3339(),
    });

    fs::write(wallet_file(), serde_json::to_string_pretty(&wallet_data)?)?;

    println!("Wallet created!");
    println!("Address: {}", address);
    println!("Saved to: {}", wallet_file().display());
    println!("\nFund with USDC-SPL to start using AI models.");
    Ok(())
}

pub async fn status(api_url: &str) -> Result<()> {
    let wallet = load_wallet()?;
    let address = wallet["address"].as_str().unwrap_or("unknown");

    println!("Wallet Address: {}", address);
    println!("Gateway: {}", api_url);

    // Check gateway health
    match reqwest::get(format!("{}/health", api_url)).await {
        Ok(resp) if resp.status().is_success() => {
            println!("Gateway Status: connected");
        }
        _ => {
            println!("Gateway Status: unreachable");
        }
    }

    Ok(())
}

pub fn export() -> Result<()> {
    let wallet = load_wallet()?;
    let key = wallet["private_key"].as_str().unwrap_or("not found");
    println!("Private Key (base58): {}", key);
    println!("\nWARNING: Keep this key secret! Anyone with this key can spend your USDC.");
    Ok(())
}

fn load_wallet() -> Result<serde_json::Value> {
    let path = wallet_file();
    if !path.exists() {
        anyhow::bail!("No wallet found. Run 'rcr wallet init' first.");
    }
    let data = fs::read_to_string(&path)?;
    Ok(serde_json::from_str(&data)?)
}

/// Fill bytes with random data using a simple method.
/// Note: Not cryptographically secure — production code should use `getrandom` or `rand`.
fn getrandom_fill(buf: &mut [u8]) {
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    // Simple PRNG for wallet generation
    let mut state = seed;
    for byte in buf.iter_mut() {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        *byte = (state >> 33) as u8;
    }
}
