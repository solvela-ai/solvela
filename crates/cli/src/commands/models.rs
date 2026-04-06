use anyhow::Result;

pub async fn list(api_url: &str) -> Result<()> {
    let resp = reqwest::get(format!("{}/v1/models", api_url)).await?;
    let body: serde_json::Value = resp.json().await?;

    if let Some(data) = body["data"].as_array() {
        println!(
            "{:<30} {:<12} {:<15} {:<15}",
            "MODEL", "PROVIDER", "INPUT $/1M", "OUTPUT $/1M"
        );
        println!("{}", "-".repeat(72));

        for model in data {
            let id = model["id"].as_str().unwrap_or("?");
            let provider = model["provider"].as_str().unwrap_or("?");
            let input = model["pricing"]["input_cost_per_million"]
                .as_f64()
                .map(|v| format!("${:.2}", v))
                .unwrap_or_else(|| "?".to_string());
            let output = model["pricing"]["output_cost_per_million"]
                .as_f64()
                .map(|v| format!("${:.2}", v))
                .unwrap_or_else(|| "?".to_string());

            println!("{:<30} {:<12} {:<15} {:<15}", id, provider, input, output);
        }

        println!(
            "\n{} models available. 5% platform fee included.",
            data.len()
        );
    } else {
        println!("No models available. Is the gateway running?");
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
    async fn test_models_list_ok() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [
                    {
                        "id": "openai/gpt-4o",
                        "provider": "openai",
                        "pricing": {
                            "input_cost_per_million": 2.50,
                            "output_cost_per_million": 10.00
                        }
                    },
                    {
                        "id": "anthropic/claude-sonnet-4-20250514",
                        "provider": "anthropic",
                        "pricing": {
                            "input_cost_per_million": 3.00,
                            "output_cost_per_million": 15.00
                        }
                    }
                ]
            })))
            .mount(&mock)
            .await;

        let result = list(&mock.uri()).await;
        assert!(result.is_ok(), "models list should succeed");
    }

    #[tokio::test]
    async fn test_models_list_empty() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": []})))
            .mount(&mock)
            .await;

        let result = list(&mock.uri()).await;
        assert!(result.is_ok(), "models list should handle empty response");
    }

    #[tokio::test]
    async fn test_models_list_no_data_field() {
        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"models": []})),
            )
            .mount(&mock)
            .await;

        let result = list(&mock.uri()).await;
        assert!(
            result.is_ok(),
            "models list should handle missing data field"
        );
    }

    #[tokio::test]
    async fn test_models_list_connection_error() {
        let result = list(&dead_url()).await;
        assert!(
            result.is_err(),
            "models list should error on connection failure"
        );
    }
}
