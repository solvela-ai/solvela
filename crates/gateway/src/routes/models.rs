use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use solvela_protocol::ModelInfo;

use crate::AppState;

/// GET /v1/models — list all available models with pricing.
///
/// The registry holds `ModelRegistration` (the full gateway-internal record);
/// each is projected to the wire-only `solvela_protocol::ModelInfo` via
/// `ModelInfo::from`, which drops the internal-only fields the gateway never
/// emits. Serialization then goes through `ModelInfo`'s `Serialize` impl so
/// the nested wire shape stays defined in exactly one place. A previous inline
/// `serde_json::json!{}` definition here created two parallel definitions of
/// the same payload; keeping a single source of truth makes `ModelInfo`'s
/// nested-wire-shape tests (`crates/protocol/src/model.rs`) authoritative for
/// this endpoint.
pub async fn list_models(State(state): State<Arc<AppState>>) -> Json<Value> {
    let models: Vec<ModelInfo> = state
        .model_registry
        .all()
        .into_iter()
        .map(ModelInfo::from)
        .collect();
    Json(json!({
        "object": "list",
        "data": models,
    }))
}
