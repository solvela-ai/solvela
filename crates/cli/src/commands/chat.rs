use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use x402::types::{PaymentPayload, PaymentRequired, Resource, SolanaPayload};

use crate::commands::wallet::load_wallet;

pub async fn run(api_url: &str, model: &str, prompt: &str, yes: bool) -> Result<()> {
    let client = reqwest::Client::new();

    let body = serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
    });

    let endpoint_url = format!("{}/v1/chat/completions", api_url);

    // First try without payment.
    let resp = client.post(&endpoint_url).json(&body).send().await?;

    if resp.status().is_success() {
        let resp_body: serde_json::Value = resp.json().await?;
        if let Some(content) = resp_body["choices"][0]["message"]["content"].as_str() {
            println!("{}", content);
        }
        return Ok(());
    }

    if resp.status().as_u16() != 402 {
        let status = resp.status();
        let text = resp.text().await?;
        println!("Error {}: {}", status, text);
        return Ok(());
    }

    // --- 402 Payment Required ---
    let error_body: serde_json::Value = resp.json().await?;
    let error_msg = error_body["error"]["message"].as_str().unwrap_or("");

    let payment_required: PaymentRequired = serde_json::from_str(error_msg)
        .context("failed to parse PaymentRequired from 402 response")?;

    // Show cost breakdown.
    let cb = &payment_required.cost_breakdown;
    println!("Cost breakdown:");
    println!("  Provider cost : {} {}", cb.provider_cost, cb.currency);
    println!(
        "  Platform fee  : {} {} ({}%)",
        cb.platform_fee, cb.currency, cb.fee_percent
    );
    println!("  Total         : {} {}", cb.total, cb.currency);

    // Confirm unless --yes was passed.
    if !yes {
        print!("Proceed? [y/N] ");
        use std::io::Write;
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !matches!(input.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Aborted.");
            return Ok(());
        }
    }

    // Load wallet (needed to identify the payer; actual tx signing is stubbed).
    let wallet = load_wallet()?;
    let _address = wallet["address"].as_str().unwrap_or("unknown");

    // Take the first accepted payment method.
    let accepted = payment_required
        .accepts
        .into_iter()
        .next()
        .context("gateway returned no accepted payment methods")?;

    // Build the PaymentPayload.
    let payment_payload = PaymentPayload {
        x402_version: x402::types::X402_VERSION,
        resource: Resource {
            url: endpoint_url.clone(),
            method: "POST".to_string(),
        },
        accepted,
        payload: x402::types::PayloadData::Direct(SolanaPayload {
            // Real versioned-transaction construction is out of scope here.
            transaction: "STUB_BASE64_TX".to_string(),
        }),
    };

    // Encode as base64(JSON(payload)).
    let payload_json = serde_json::to_string(&payment_payload)?;
    let payment_header = BASE64.encode(payload_json.as_bytes());

    // Retry with the payment header.
    let retry_resp = client
        .post(&endpoint_url)
        .header("PAYMENT-SIGNATURE", &payment_header)
        .json(&body)
        .send()
        .await?;

    if retry_resp.status().is_success() {
        let resp_body: serde_json::Value = retry_resp.json().await?;
        if let Some(content) = resp_body["choices"][0]["message"]["content"].as_str() {
            println!("{}", content);
        }
    } else {
        let status = retry_resp.status();
        let text = retry_resp.text().await?;
        println!("Error {}: {}", status, text);
    }

    Ok(())
}
