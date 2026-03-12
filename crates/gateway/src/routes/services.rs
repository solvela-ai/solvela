use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::security;
use crate::services::{RegistrationError, ServiceEntry};
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
    let registry = state.service_registry.read().await;
    let all = registry.all();

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
                "source": svc.source,
                "healthy": svc.healthy,
                "price_per_request_usdc": svc.price_per_request_usdc,
            })
        })
        .collect();

    Json(json!({
        "object": "list",
        "data": services,
        "total": services.len(),
    }))
}

// ---------------------------------------------------------------------------
// POST /v1/services/register — runtime service registration
// ---------------------------------------------------------------------------

/// Request body for `POST /v1/services/register`.
#[derive(Debug, Deserialize)]
pub struct RegisterServiceRequest {
    pub id: String,
    pub name: String,
    pub endpoint: String,
    pub category: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub pricing_label: Option<String>,
    pub price_per_request_usdc: Option<f64>,
}

/// POST /v1/services/register — register a new external service at runtime.
///
/// Protected by `RCR_ADMIN_TOKEN` env var. Returns:
/// - 201 Created with the full `ServiceEntry` on success
/// - 400 Bad Request for validation errors
/// - 401 Unauthorized if token is wrong or missing
/// - 404 Not Found if `RCR_ADMIN_TOKEN` is not set
/// - 409 Conflict if the service ID already exists
pub async fn register_service(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<RegisterServiceRequest>,
) -> impl IntoResponse {
    // Gate behind RCR_ADMIN_TOKEN — if not configured, hide the endpoint entirely
    let admin_token = match std::env::var("RCR_ADMIN_TOKEN") {
        Ok(t) if !t.is_empty() => t,
        _ => {
            return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response();
        }
    };

    // Validate Bearer token
    let authorized = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|token| security::constant_time_eq(token.as_bytes(), admin_token.as_bytes()));

    if !authorized {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    }

    // Validate endpoint is not a private/internal network address (SSRF prevention)
    match security::is_private_endpoint(&body.endpoint).await {
        Ok(true) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "service endpoint must not resolve to a private or internal network address" })),
            )
                .into_response();
        }
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("service endpoint validation failed: {e}") })),
            )
                .into_response();
        }
        Ok(false) => { /* public address — proceed */ }
    }

    // Build the ServiceEntry
    let pricing_label = body
        .pricing_label
        .unwrap_or_else(|| match body.price_per_request_usdc {
            Some(price) => format!("${price}/request"),
            None => "per-request (see /pricing)".to_string(),
        });

    let entry = ServiceEntry {
        id: body.id,
        name: body.name,
        category: body.category,
        endpoint: body.endpoint,
        x402_enabled: true,
        internal: false,
        description: body.description,
        pricing_label,
        chains: vec!["solana".to_string()],
        source: "api".to_string(),
        healthy: None,
        price_per_request_usdc: body.price_per_request_usdc,
    };

    // Acquire write lock and register
    let mut registry = state.service_registry.write().await;
    match registry.register(entry.clone()) {
        Ok(()) => (StatusCode::CREATED, Json(json!(entry))).into_response(),
        Err(RegistrationError::DuplicateId(id)) => (
            StatusCode::CONFLICT,
            Json(json!({ "error": format!("service with id '{id}' already exists") })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}
