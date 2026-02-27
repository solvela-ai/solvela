use anyhow::Result;

pub async fn run(api_url: &str, model: &str, prompt: &str) -> Result<()> {
    let client = reqwest::Client::new();

    let body = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
    });

    // First try without payment (will get 402)
    let resp = client
        .post(format!("{}/v1/chat/completions", api_url))
        .json(&body)
        .send()
        .await?;

    if resp.status().as_u16() == 402 {
        let error_body: serde_json::Value = resp.json().await?;
        let error_msg = error_body["error"]["message"].as_str().unwrap_or("");

        if let Ok(payment_info) = serde_json::from_str::<serde_json::Value>(error_msg) {
            let total = payment_info["cost_breakdown"]["total"]
                .as_str()
                .unwrap_or("?");
            println!("Cost: {} USDC", total);
            println!("(Payment signing not yet implemented in CLI — use SDK)");
        } else {
            println!("Payment required. Use an SDK with a funded wallet.");
        }
        return Ok(());
    }

    if resp.status().is_success() {
        let body: serde_json::Value = resp.json().await?;
        if let Some(content) = body["choices"][0]["message"]["content"].as_str() {
            println!("{}", content);
        }
    } else {
        let status = resp.status();
        let body = resp.text().await?;
        println!("Error {}: {}", status, body);
    }

    Ok(())
}
