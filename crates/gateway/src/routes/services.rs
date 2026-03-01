use std::sync::Arc;

use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::AppState;

/// Optional query parameters for `GET /v1/services`.
#[derive(Debug, Deserialize)]
pub struct ServicesQuery {
    /// Filter by category (e.g. `?category=intelligence`).
    #[serde(default)]
    pub category: Option<String>,
    /// Filter to only internal (`true`) or external (`false`) services.
    pub internal: Option<bool>,
}

/// GET /v1/services — list all registered x402-compatible services.
///
/// Returns the service marketplace registry. Supports optional filtering by
/// category and internal/external status. Internal services are hosted by the
/// gateway itself; external services are third-party x402 endpoints.
///
/// # Query parameters
/// - `?category=<cat>` — filter by service category
/// - `?internal=true|false` — filter to internal or external services only
pub async fn list_services(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ServicesQuery>,
) -> Json<Value> {
    let all = state.service_registry.all();

    let services: Vec<Value> = all
        .iter()
        .filter(|svc| {
            // Apply category filter if provided
            if let Some(ref cat) = params.category {
                if !svc.category.eq_ignore_ascii_case(cat) {
                    return false;
                }
            }
            // Apply internal/external filter if provided
            if let Some(internal_only) = params.internal {
                if svc.internal != internal_only {
                    return false;
                }
            }
            true
        })
        .map(|svc| {
            json!({
                "id": svc.id,
                "name": svc.name,
                "category": svc.category,
                "endpoint": svc.endpoint,
                "x402_enabled": svc.x402_enabled,
                "internal": svc.internal,
                "description": svc.description,
                "pricing": svc.pricing_label,
                "chains": svc.chains,
            })
        })
        .collect();

    Json(json!({
        "object": "list",
        "data": services,
        "total": services.len(),
    }))
}
