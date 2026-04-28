//! GET /metrics — Prometheus metrics endpoint.
//!
//! Returns Prometheus text exposition format. Gated behind the admin token
//! stored in `AppState.admin_token` (read from `SOLVELA_ADMIN_TOKEN` at startup).
//! Returns 404 when the admin token is not configured (hides the endpoint
//! entirely). Returns 503 when the Prometheus recorder failed to install.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;

use crate::security;
use crate::AppState;

/// Prometheus text content type per the exposition format spec.
const PROMETHEUS_CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// GET /metrics
///
/// Returns:
/// - 200 with Prometheus text format body when authorized
/// - 401 when the Authorization header is missing or invalid
/// - 404 when the admin token is not configured (endpoint hidden)
/// - 503 when the Prometheus recorder is unavailable
pub async fn get_metrics(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // If no admin token is configured, hide the endpoint entirely
    let admin_token = match &state.admin_token {
        Some(t) => t,
        None => {
            return (StatusCode::NOT_FOUND, "not found").into_response();
        }
    };

    // Validate Bearer token using constant-time comparison
    let authorized = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|token| security::constant_time_eq(token.as_bytes(), admin_token.as_bytes()));

    if !authorized {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    }

    let handle = match &state.prometheus_handle {
        Some(h) => h,
        None => {
            return (StatusCode::SERVICE_UNAVAILABLE, "metrics unavailable").into_response();
        }
    };

    let body = handle.render();
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, PROMETHEUS_CONTENT_TYPE)],
        body,
    )
        .into_response()
}
