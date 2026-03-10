use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use rustyclaw_protocol::PLATFORM_FEE_PERCENT;

use crate::AppState;

/// GET /pricing — detailed per-model pricing with example request costs.
///
/// Returns provider cost, platform fee, and total for each model,
/// plus example costs for a typical 1 000-token request.
pub async fn pricing(State(state): State<Arc<AppState>>) -> Json<Value> {
    let fee_multiplier = 1.0 + PLATFORM_FEE_PERCENT as f64 / 100.0;

    let models: Vec<_> = state
        .model_registry
        .all()
        .into_iter()
        .map(|m| {
            // Cost for a representative 1 000-token request (500 in + 500 out)
            let example_input_tokens = 500u64;
            let example_output_tokens = 500u64;
            let provider_cost = (m.input_cost_per_million * example_input_tokens as f64
                + m.output_cost_per_million * example_output_tokens as f64)
                / 1_000_000.0;
            let total_cost = provider_cost * fee_multiplier;
            let platform_fee = total_cost - provider_cost;

            json!({
                "id": m.id,
                "display_name": m.display_name,
                "provider": m.provider,
                "pricing": {
                    "input_per_million_usdc": m.input_cost_per_million,
                    "output_per_million_usdc": m.output_cost_per_million,
                    "platform_fee_percent": PLATFORM_FEE_PERCENT,
                    "currency": "USDC",
                },
                "example_1k_token_request": {
                    "input_tokens": example_input_tokens,
                    "output_tokens": example_output_tokens,
                    "provider_cost_usdc": format!("{:.6}", provider_cost),
                    "platform_fee_usdc": format!("{:.6}", platform_fee),
                    "total_usdc": format!("{:.6}", total_cost),
                },
                "capabilities": {
                    "streaming": m.supports_streaming,
                    "tools": m.supports_tools,
                    "vision": m.supports_vision,
                    "reasoning": m.reasoning,
                    "context_window": m.context_window,
                },
            })
        })
        .collect();

    Json(json!({
        "platform": {
            "name": "RustyClawRouter",
            "chain": "solana",
            "token": "USDC-SPL",
            "usdc_mint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
            "fee_percent": PLATFORM_FEE_PERCENT,
            "fee_description": "5% platform fee is added on top of provider cost",
            "settlement": "Solana USDC-SPL TransferChecked (pre-signed versioned tx)",
            "min_tx_cost_sol": 0.000005,
        },
        "models": models,
    }))
}
