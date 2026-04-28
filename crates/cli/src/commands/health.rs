use anyhow::Result;

pub async fn check(api_url: &str) -> Result<()> {
    println!("Checking gateway at {}...", api_url);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;

    match client.get(format!("{}/health", api_url)).send().await {
        Ok(resp) => {
            if resp.status().is_success() {
                let body: serde_json::Value = resp.json().await?;
                let status = body["status"].as_str().unwrap_or("unknown");
                println!("Status: {}", status);
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
    async fn test_health_check_ok() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"status": "ok"})),
            )
            .mount(&mock)
            .await;

        let result = check(&mock.uri()).await;
        assert!(result.is_ok(), "health check should succeed");
    }

    #[tokio::test]
    async fn test_health_check_server_error() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&mock)
            .await;

        let result = check(&mock.uri()).await;
        assert!(result.is_ok(), "health check should handle 500 gracefully");
    }

    #[tokio::test]
    async fn test_health_check_unreachable() {
        let result = check(&dead_url()).await;
        assert!(
            result.is_ok(),
            "health check should handle connection failure gracefully"
        );
    }
}
