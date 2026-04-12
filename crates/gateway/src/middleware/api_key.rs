use std::sync::Arc;

use axum::extract::{FromRequestParts, Request, State};
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use tracing::warn;
use uuid::Uuid;

use crate::orgs::models::OrgRole;
use crate::AppState;

/// Resolved organization context from API key authentication.
/// Inserted into request extensions by the api_key middleware.
#[derive(Debug, Clone)]
pub struct OrgContext {
    pub org_id: Uuid,
    pub api_key_id: Uuid,
    pub role: OrgRole,
}

/// Middleware: extract API key from Authorization header, verify, inject `OrgContext`.
///
/// This is additive — it never blocks requests. Routes decide whether to require
/// `OrgContext` via the [`RequireOrg`] or [`RequireOrgAdmin`] extractors.
pub async fn extract_api_key(
    State(state): State<Arc<AppState>>,
    mut request: Request,
    next: Next,
) -> Response {
    if let Some(auth) = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        if auth.starts_with("rcr_k_") {
            if let Some(pool) = &state.db_pool {
                match crate::orgs::queries::verify_api_key(pool, auth).await {
                    Ok(Some((api_key, org_id))) => {
                        request.extensions_mut().insert(OrgContext {
                            org_id,
                            api_key_id: api_key.id,
                            role: api_key.role,
                        });
                    }
                    Ok(None) => {
                        warn!("invalid or expired API key");
                    }
                    Err(e) => {
                        warn!(error = %e, "API key verification DB error — returning 503");
                        return (
                            StatusCode::SERVICE_UNAVAILABLE,
                            Json(serde_json::json!({
                                "error": {
                                    "type": "service_unavailable",
                                    "message": "Authentication service temporarily unavailable"
                                }
                            })),
                        )
                            .into_response();
                    }
                }
            }
        }
    }

    next.run(request).await
}

/// Extractor that requires a valid API key with org context.
/// Returns 401 if no valid API key is present.
#[derive(Debug)]
pub struct RequireOrg(pub OrgContext);

impl<S> FromRequestParts<S> for RequireOrg
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, Json<serde_json::Value>);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<OrgContext>()
            .cloned()
            .map(RequireOrg)
            .ok_or_else(|| {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({
                        "error": {
                            "type": "unauthorized",
                            "message": "Valid API key required"
                        }
                    })),
                )
            })
    }
}

/// Extractor that requires org admin or owner role.
/// Returns 401 if no API key, 403 if insufficient role.
#[derive(Debug)]
pub struct RequireOrgAdmin(pub OrgContext);

impl<S> FromRequestParts<S> for RequireOrgAdmin
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, Json<serde_json::Value>);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let ctx = parts
            .extensions
            .get::<OrgContext>()
            .cloned()
            .ok_or_else(|| {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(serde_json::json!({
                        "error": {
                            "type": "unauthorized",
                            "message": "Valid API key required"
                        }
                    })),
                )
            })?;

        if ctx.role.is_admin_or_owner() {
            Ok(RequireOrgAdmin(ctx))
        } else {
            Err((
                StatusCode::FORBIDDEN,
                Json(serde_json::json!({
                    "error": {
                        "type": "forbidden",
                        "message": "Admin or owner role required"
                    }
                })),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::Body;
    use axum::http;
    use tokio::sync::RwLock;
    use tower::ServiceExt;

    use crate::config::AppConfig;
    use crate::providers::health::{CircuitBreakerConfig, ProviderHealthTracker};
    use crate::providers::ProviderRegistry;
    use crate::routes::escrow::new_slot_cache;
    use crate::services::ServiceRegistry;
    use crate::usage::UsageTracker;
    use solvela_router::models::ModelRegistry;
    use x402::facilitator::Facilitator;

    /// Helper: build a minimal Router that runs `extract_api_key` middleware
    /// and returns 200 with the OrgContext debug string if present, or "none".
    fn test_router(state: Arc<AppState>) -> axum::Router {
        axum::Router::new()
            .route(
                "/test",
                axum::routing::get(|ext: Option<axum::Extension<OrgContext>>| async move {
                    match ext {
                        Some(axum::Extension(ctx)) => format!("org:{}", ctx.org_id),
                        None => "none".to_string(),
                    }
                }),
            )
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                extract_api_key,
            ))
            .with_state(state)
    }

    fn make_state() -> Arc<AppState> {
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
            .expect("valid test model TOML"),
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

    #[tokio::test]
    async fn test_no_auth_header_passes_through() {
        let state = make_state();
        let app = test_router(state);

        let req = http::Request::builder()
            .uri("/test")
            .body(Body::empty())
            .expect("valid request");

        let resp = app.oneshot(req).await.expect("request should succeed");
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024)
            .await
            .expect("read body");
        assert_eq!(&body[..], b"none", "no OrgContext should be inserted");
    }

    #[tokio::test]
    async fn test_non_rcr_key_ignored() {
        let state = make_state();
        let app = test_router(state);

        let req = http::Request::builder()
            .uri("/test")
            .header("authorization", "Bearer sk-some-openai-key")
            .body(Body::empty())
            .expect("valid request");

        let resp = app.oneshot(req).await.expect("request should succeed");
        assert_eq!(resp.status(), StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 1024)
            .await
            .expect("read body");
        assert_eq!(&body[..], b"none", "non-rcr_k_ tokens must be ignored");
    }

    #[tokio::test]
    async fn test_require_org_missing_context() {
        // Build Parts with no OrgContext in extensions
        let (mut parts, _body) = http::Request::builder()
            .uri("/test")
            .body(())
            .expect("valid request")
            .into_parts();

        let result = RequireOrg::from_request_parts(&mut parts, &()).await;
        assert!(result.is_err(), "should reject when no OrgContext");

        let (status, _json) = result.unwrap_err();
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_require_org_admin_member_role() {
        let (mut parts, _body) = http::Request::builder()
            .uri("/test")
            .body(())
            .expect("valid request")
            .into_parts();

        parts.extensions.insert(OrgContext {
            org_id: Uuid::new_v4(),
            api_key_id: Uuid::new_v4(),
            role: OrgRole::Member,
        });

        let result = RequireOrgAdmin::from_request_parts(&mut parts, &()).await;
        assert!(result.is_err(), "member role should be rejected");

        let (status, _json) = result.unwrap_err();
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn test_require_org_admin_owner_role() {
        let (mut parts, _body) = http::Request::builder()
            .uri("/test")
            .body(())
            .expect("valid request")
            .into_parts();

        parts.extensions.insert(OrgContext {
            org_id: Uuid::new_v4(),
            api_key_id: Uuid::new_v4(),
            role: OrgRole::Owner,
        });

        let result = RequireOrgAdmin::from_request_parts(&mut parts, &()).await;
        assert!(result.is_ok(), "owner role should be accepted");
    }

    #[tokio::test]
    async fn test_require_org_admin_admin_role() {
        let (mut parts, _body) = http::Request::builder()
            .uri("/test")
            .body(())
            .expect("valid request")
            .into_parts();

        parts.extensions.insert(OrgContext {
            org_id: Uuid::new_v4(),
            api_key_id: Uuid::new_v4(),
            role: OrgRole::Admin,
        });

        let result = RequireOrgAdmin::from_request_parts(&mut parts, &()).await;
        assert!(result.is_ok(), "admin role should be accepted");
    }
}
