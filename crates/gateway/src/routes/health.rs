use std::sync::Arc;

use axum::extract::State;
use axum::http::HeaderMap;
use axum::Json;
use serde_json::{json, Value};

use crate::security;
use crate::AppState;

/// Check whether the request carries a valid admin bearer token.
fn is_admin_authenticated(headers: &HeaderMap, admin_token: Option<&str>) -> bool {
    let Some(expected) = admin_token else {
        return false;
    };
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|token| security::constant_time_eq(token.as_bytes(), expected.as_bytes()))
}

/// GET /health — gateway readiness check with dependency status.
///
/// Returns HTTP 200 always (load balancers need 2xx).
/// The `status` field indicates overall readiness:
/// - `"ok"` — at least one provider configured, all configured deps healthy
/// - `"degraded"` — at least one provider, but a configured dep is unavailable
/// - `"error"` — zero providers configured
///
/// Detailed `checks` (database, redis, providers, solana_rpc) are only included
/// when the request carries a valid `Authorization: Bearer <admin_token>` header.
/// Unauthenticated callers receive only `{"status":"..."}`.
pub async fn health(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Json<Value> {
    // ── Database check ──────────────────────────────────────────────────────
    let database_status = match &state.db_pool {
        Some(pool) => {
            let check = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(pool),
            )
            .await;
            match check {
                Ok(Ok(_)) => "connected",
                _ => "error",
            }
        }
        None => "not_configured",
    };

    // ── Redis check ─────────────────────────────────────────────────────────
    let redis_status = match &state.cache {
        Some(cache) => {
            let ping = tokio::time::timeout(std::time::Duration::from_secs(2), cache.ping()).await;
            match ping {
                Ok(true) => "connected",
                _ => "error",
            }
        }
        None => "not_configured",
    };

    // ── Providers ───────────────────────────────────────────────────────────
    let providers: Vec<&str> = state.providers.configured_providers();

    // ── Solana RPC ──────────────────────────────────────────────────────────
    let solana_rpc_status = if state.config.solana.rpc_url.is_empty() {
        "not_configured"
    } else {
        "configured"
    };

    // ── Overall status logic ────────────────────────────────────────────────
    let status = if providers.is_empty() {
        "error"
    } else {
        let db_degraded = database_status == "error";
        let redis_degraded = redis_status == "error";
        if db_degraded || redis_degraded {
            "degraded"
        } else {
            "ok"
        }
    };

    let authenticated = is_admin_authenticated(&headers, state.admin_token.as_deref());

    if authenticated {
        Json(json!({
            "status": status,
            "version": env!("CARGO_PKG_VERSION"),
            "checks": {
                "database": database_status,
                "redis": redis_status,
                "providers": providers,
                "solana_rpc": solana_rpc_status,
            }
        }))
    } else {
        Json(json!({
            "status": status,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use http_body_util::BodyExt;
    use tokio::sync::RwLock;
    use tower::ServiceExt;

    use crate::config::AppConfig;
    use crate::providers::health::{CircuitBreakerConfig, ProviderHealthTracker};
    use crate::providers::ProviderRegistry;
    use crate::routes::escrow::new_slot_cache;
    use crate::services::ServiceRegistry;
    use crate::usage::UsageTracker;
    use router::models::ModelRegistry;
    use x402::facilitator::Facilitator;

    fn test_state() -> Arc<AppState> {
        Arc::new(AppState {
            config: AppConfig::default(),
            model_registry: ModelRegistry::from_toml(
                r#"
[models.test-model]
provider = "test"
model_id = "test-model"
display_name = "Test"
input_cost_per_million = 1.0
output_cost_per_million = 2.0
context_window = 4096
supports_streaming = false
supports_tools = false
supports_vision = false
                "#,
            )
            .unwrap(),
            service_registry: RwLock::new(ServiceRegistry::empty()),
            providers: ProviderRegistry::from_env(reqwest::Client::new()),
            facilitator: Facilitator::new(vec![]),
            usage: UsageTracker::noop(),
            cache: None,
            provider_health: ProviderHealthTracker::new(CircuitBreakerConfig::default()),
            escrow_claimer: None,
            fee_payer_pool: None,
            nonce_pool: None,
            db_pool: None,
            session_secret: b"test-secret".to_vec(),
            http_client: reqwest::Client::new(),
            replay_set: AppState::new_replay_set(),
            slot_cache: new_slot_cache(),
            escrow_metrics: None,
            admin_token: None,
            prometheus_handle: None,
            dev_bypass_payment: false,
        })
    }

    fn test_router(state: Arc<AppState>) -> Router {
        Router::new()
            .route("/health", get(health))
            .with_state(state)
    }

    #[tokio::test]
    async fn test_health_unauthenticated_returns_only_status() {
        let state = test_state();
        let app = test_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // Unauthenticated: only status, no version or checks
        assert!(json["status"].is_string());
        assert!(json.get("checks").is_none() || json["checks"].is_null());
        assert!(json.get("version").is_none() || json["version"].is_null());
    }

    fn test_state_with_admin_token() -> Arc<AppState> {
        Arc::new(AppState {
            config: AppConfig::default(),
            model_registry: ModelRegistry::from_toml(
                r#"
[models.test-model]
provider = "test"
model_id = "test-model"
display_name = "Test"
input_cost_per_million = 1.0
output_cost_per_million = 2.0
context_window = 4096
supports_streaming = false
supports_tools = false
supports_vision = false
                "#,
            )
            .unwrap(),
            service_registry: RwLock::new(ServiceRegistry::empty()),
            providers: ProviderRegistry::from_env(reqwest::Client::new()),
            facilitator: Facilitator::new(vec![]),
            usage: UsageTracker::noop(),
            cache: None,
            provider_health: ProviderHealthTracker::new(CircuitBreakerConfig::default()),
            escrow_claimer: None,
            fee_payer_pool: None,
            nonce_pool: None,
            db_pool: None,
            session_secret: b"test-secret".to_vec(),
            http_client: reqwest::Client::new(),
            replay_set: AppState::new_replay_set(),
            slot_cache: new_slot_cache(),
            escrow_metrics: None,
            admin_token: Some("test-admin-token".to_string()),
            prometheus_handle: None,
            dev_bypass_payment: false,
        })
    }

    #[tokio::test]
    async fn test_health_authenticated_returns_checks_structure() {
        let state = test_state_with_admin_token();
        let app = test_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .header("Authorization", "Bearer test-admin-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // Authenticated: has status, version, checks
        assert!(json["status"].is_string());
        assert!(json["version"].is_string());
        assert!(json["checks"].is_object());
        assert!(json["checks"]["database"].is_string());
        assert!(json["checks"]["redis"].is_string());
        assert!(json["checks"]["providers"].is_array());
        assert!(json["checks"]["solana_rpc"].is_string());
    }

    #[tokio::test]
    async fn test_health_no_db_no_redis_reports_not_configured() {
        let state = test_state_with_admin_token();
        let app = test_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .header("Authorization", "Bearer test-admin-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(json["checks"]["database"], "not_configured");
        assert_eq!(json["checks"]["redis"], "not_configured");
    }

    #[tokio::test]
    async fn test_health_no_providers_returns_error_status() {
        // ProviderRegistry::from_env() with no API keys → empty providers
        let state = test_state();
        let app = test_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // HTTP status is always 200
        assert_eq!(response.status(), StatusCode::OK);

        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // No providers configured in test env → "error"
        assert_eq!(json["status"], "error");
    }
}
