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
