use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use crate::AppState;

/// GET /v1/models — list all available models with pricing.
pub async fn list_models(State(state): State<Arc<AppState>>) -> Json<Value> {
    let models: Vec<_> = state
        .model_registry
        .all()
        .into_iter()
        .map(|m| {
            json!({
                "id": m.id,
                "object": "model",
                "provider": m.provider,
                "display_name": m.display_name,
                "context_window": m.context_window,
                "pricing": {
                    "input_per_million": m.input_cost_per_million,
                    "output_per_million": m.output_cost_per_million,
                    "currency": "USDC",
                    "fee_percent": rustyclaw_protocol::PLATFORM_FEE_PERCENT,
                },
                "capabilities": {
                    "streaming": m.supports_streaming,
                    "tools": m.supports_tools,
                    "vision": m.supports_vision,
                    "reasoning": m.reasoning,
                },
            })
        })
        .collect();

    Json(json!({
        "object": "list",
        "data": models,
    }))
}
