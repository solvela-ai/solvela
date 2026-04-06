use std::path::Path;

use anyhow::Result;

pub async fn run(api_url: &str) -> Result<()> {
    println!("Running diagnostics...\n");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build()?;

    // Check 1: Gateway connectivity
    print!("Gateway ({})... ", api_url);
    match client.get(format!("{}/health", api_url)).send().await {
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
    match client.get(format!("{}/v1/models", api_url)).send().await {
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

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Bind a TCP listener to get an OS-assigned port, then drop it.
    /// The returned URL will be connection-refused immediately (ECONNREFUSED).
    fn dead_url() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        drop(listener);
        format!("http://127.0.0.1:{port}")
    }

    #[tokio::test]
    async fn test_doctor_all_checks_pass() {
        // Hold the async mutex for the full test to prevent HOME from being
        // clobbered by another test while run() reads it.
        let _lock = crate::ENV_MUTEX.lock().await;
        let tmp = tempfile::TempDir::new().expect("tempdir");
        std::env::set_var("HOME", tmp.path());
        let wallet_dir = tmp.path().join(".rustyclawrouter");
        std::fs::create_dir_all(&wallet_dir).expect("mkdir");
        std::fs::write(
            wallet_dir.join("wallet.json"),
            r#"{"address":"test","private_key":"test"}"#,
        )
        .expect("write wallet");

        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"status": "ok"})),
            )
            .mount(&mock)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"id": "test-model"}]
            })))
            .mount(&mock)
            .await;

        let result = run(&mock.uri()).await;
        assert!(
            result.is_ok(),
            "doctor should succeed even with mixed results"
        );
        drop(tmp);
    }

    #[tokio::test]
    async fn test_doctor_gateway_unreachable() {
        let result = run(&dead_url()).await;
        assert!(
            result.is_ok(),
            "doctor should not error when gateway is unreachable"
        );
    }

    #[tokio::test]
    async fn test_doctor_no_wallet() {
        // Hold the async mutex for the full test to prevent HOME from being
        // clobbered by another test while run() reads it.
        let _lock = crate::ENV_MUTEX.lock().await;
        let tmp = tempfile::TempDir::new().expect("tempdir");
        std::env::set_var("HOME", tmp.path());

        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"status": "ok"})),
            )
            .mount(&mock)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": []})))
            .mount(&mock)
            .await;

        let result = run(&mock.uri()).await;
        assert!(
            result.is_ok(),
            "doctor should handle missing wallet gracefully"
        );
    }
}
