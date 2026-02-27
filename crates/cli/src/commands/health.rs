use anyhow::Result;

pub async fn check(api_url: &str) -> Result<()> {
    println!("Checking gateway at {}...", api_url);

    match reqwest::get(format!("{}/health", api_url)).await {
        Ok(resp) => {
            if resp.status().is_success() {
                let body: serde_json::Value = resp.json().await?;
                let status = body["status"].as_str().unwrap_or("unknown");
                let version = body["version"].as_str().unwrap_or("unknown");
                println!("Status: {}", status);
                println!("Version: {}", version);
            } else {
                println!("Gateway returned status: {}", resp.status());
            }
        }
        Err(e) => {
            println!("Failed to connect: {}", e);
            println!("\nIs the gateway running? Start with: cargo run -p gateway");
        }
    }

    Ok(())
}
