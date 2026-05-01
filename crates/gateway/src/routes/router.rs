use axum::{extract::State, Json};
use std::sync::Arc;

use solvela_protocol::ChatRequest;
use solvela_router::analyzer::{analyze_request, RouterAnalysis};

use crate::{error::GatewayError, AppState};

/// `POST /v1/router/analyze`
///
/// Diagnostic endpoint that classifies an inbound `ChatRequest` through the
/// 15-dimension smart router and returns a structured breakdown — tier, score,
/// per-dimension signals, and per-profile model recommendations — without
/// performing payment verification or proxying to a provider.
///
/// Intended for development tooling, SDK authors, and debugging.
pub async fn analyze(
    State(_state): State<Arc<AppState>>,
    Json(req): Json<ChatRequest>,
) -> Result<Json<RouterAnalysis>, GatewayError> {
    if req.messages.is_empty() {
        return Err(GatewayError::BadRequest(
            "messages must not be empty".to_string(),
        ));
    }
    Ok(Json(analyze_request(&req)))
}
