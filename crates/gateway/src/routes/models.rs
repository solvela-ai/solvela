use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use crate::AppState;

/// GET /v1/models — list all available models with pricing.
///
/// Each entry is serialized via `solvela_protocol::ModelInfo`'s `Serialize`
/// impl so the wire shape stays defined in exactly one place. A previous
/// inline `serde_json::json!{}` definition here created two parallel
/// definitions of the same payload; keeping a single source of truth makes
/// `ModelInfo`'s nested-wire-shape tests (`crates/protocol/src/model.rs`)
/// authoritative for this endpoint.
pub async fn list_models(State(state): State<Arc<AppState>>) -> Json<Value> {
    let models = state.model_registry.all();
    Json(json!({
        "object": "list",
        "data": models,
    }))
}
