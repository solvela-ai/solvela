use std::path::Path;

use anyhow::Result;

pub async fn run(api_url: &str) -> Result<()> {
    println!("Running diagnostics...\n");

    // Check 1: Gateway connectivity
    print!("Gateway ({})... ", api_url);
    match reqwest::get(format!("{}/health", api_url)).await {
        Ok(resp) if resp.status().is_success() => println!("OK"),
        Ok(resp) => println!("ERROR (status {})", resp.status()),
        Err(e) => println!("FAIL ({})", e),
    }

    // Check 2: Wallet
    let wallet_path = std::env::var("HOME")
        .map(|h| format!("{}/.rustyclawrouter/wallet.json", h))
        .unwrap_or_else(|_| ".rustyclawrouter/wallet.json".to_string());
    print!("Wallet ({})... ", wallet_path);
    if Path::new(&wallet_path).exists() {
        println!("FOUND");
    } else {
        println!("NOT FOUND (run 'rcr wallet init')");
    }

    // Check 3: Models endpoint
    print!("Models endpoint... ");
    match reqwest::get(format!("{}/v1/models", api_url)).await {
        Ok(resp) if resp.status().is_success() => {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let count = body["data"].as_array().map(|a| a.len()).unwrap_or(0);
            println!("OK ({} models)", count);
        }
        _ => println!("UNAVAILABLE"),
    }

    // Check 4: Environment variables
    print!("SOLANA_WALLET_KEY... ");
    if std::env::var("SOLANA_WALLET_KEY").is_ok() {
        println!("SET");
    } else {
        println!("NOT SET");
    }

    println!("\nDiagnostics complete.");
    Ok(())
}
