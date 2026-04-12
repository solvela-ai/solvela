//! API key management handlers.

use super::*;
use crate::orgs::models::OrgRole;

/// `POST /v1/orgs/:id/api-keys` — Create an API key for an organization.
pub async fn create_api_key(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    org_ctx: Option<Extension<OrgContext>>,
    Path(org_id): Path<Uuid>,
    Json(body): Json<CreateApiKeyRequest>,
) -> Response {
    let auth = match require_auth(&state, &headers, org_ctx.as_ref().map(|e| &e.0)) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_org_admin_access(&auth, org_id) {
        return resp;
    }

    // Prevent role escalation: callers cannot assign a role higher than their own
    if let AuthContext::OrgKey(ref ctx) = auth {
        if let Some(ref requested_role) = body.role {
            if *requested_role == OrgRole::Owner && ctx.role != OrgRole::Owner {
                return (
                    StatusCode::FORBIDDEN,
                    Json(json!({
                        "error": "only owners can assign the owner role"
                    })),
                )
                    .into_response();
            }
        }
    }

    if let Err((status, err)) = validate_name(&body.name, "name") {
        return (status, err).into_response();
    }

    let pool = require_db!(state);

    let actor_api_key = match &auth {
        AuthContext::OrgKey(ctx) => Some(ctx.api_key_id),
        AuthContext::Admin => None,
    };

    match queries::create_api_key(pool, org_id, body).await {
        Ok(created) => {
            log_audit(
                pool,
                AuditEntry {
                    org_id: Some(org_id),
                    actor_wallet: None,
                    actor_api_key,
                    action: "api_key.created".to_string(),
                    resource_type: "api_key".to_string(),
                    resource_id: Some(created.id.to_string()),
                    details: None,
                    ip_address: None,
                },
            );
            (StatusCode::CREATED, Json(created)).into_response()
        }
        Err(e) => {
            tracing::warn!(org_id = %org_id, error = %e, "failed to create api key");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to create API key" })),
            )
                .into_response()
        }
    }
}

/// `GET /v1/orgs/:id/api-keys` — List (non-revoked) API keys for an organization.
pub async fn list_api_keys(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    org_ctx: Option<Extension<OrgContext>>,
    Path(org_id): Path<Uuid>,
) -> Response {
    let auth = match require_auth(&state, &headers, org_ctx.as_ref().map(|e| &e.0)) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_org_access(&auth, org_id) {
        return resp;
    }
    let pool = require_db!(state);

    match queries::list_api_keys(pool, org_id).await {
        Ok(keys) => (StatusCode::OK, Json(keys)).into_response(),
        Err(e) => {
            tracing::warn!(org_id = %org_id, error = %e, "failed to list api keys");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to list API keys" })),
            )
                .into_response()
        }
    }
}

/// `DELETE /v1/orgs/:id/api-keys/:kid` — Revoke an API key.
pub async fn revoke_api_key(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    org_ctx: Option<Extension<OrgContext>>,
    Path((org_id, key_id)): Path<(Uuid, Uuid)>,
) -> Response {
    let auth = match require_auth(&state, &headers, org_ctx.as_ref().map(|e| &e.0)) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_org_admin_access(&auth, org_id) {
        return resp;
    }
    let pool = require_db!(state);

    let actor_api_key = match &auth {
        AuthContext::OrgKey(ctx) => Some(ctx.api_key_id),
        AuthContext::Admin => None,
    };

    match queries::revoke_api_key(pool, key_id, org_id).await {
        Ok(true) => {
            log_audit(
                pool,
                AuditEntry {
                    org_id: Some(org_id),
                    actor_wallet: None,
                    actor_api_key,
                    action: "api_key.revoked".to_string(),
                    resource_type: "api_key".to_string(),
                    resource_id: Some(key_id.to_string()),
                    details: None,
                    ip_address: None,
                },
            );
            (StatusCode::OK, Json(json!({ "revoked": true }))).into_response()
        }
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "API key not found or already revoked" })),
        )
            .into_response(),
        Err(e) => {
            tracing::warn!(org_id = %org_id, key_id = %key_id, error = %e, "failed to revoke api key");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to revoke API key" })),
            )
                .into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::super::test_helpers::test_router;

    #[tokio::test]
    async fn revoke_api_key_requires_auth() {
        let app = test_router(Some("tok"));
        let org_id = Uuid::new_v4();
        let key_id = Uuid::new_v4();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/v1/orgs/{org_id}/api-keys/{key_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn create_api_key_no_db_returns_503() {
        let app = test_router(Some("mytoken"));
        let org_id = Uuid::new_v4();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/orgs/{org_id}/api-keys"))
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer mytoken")
                    .body(Body::from(r#"{"name":"My Key","scopes":[]}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        // db_pool is None -> 503
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn create_api_key_admin_key_cannot_assign_owner_role() {
        use axum::routing::post;
        use axum::Router;
        use std::sync::Arc;

        use crate::middleware::api_key::OrgContext;
        use crate::orgs::models::OrgRole;
        use crate::providers::health::{CircuitBreakerConfig, ProviderHealthTracker};
        use crate::services::ServiceRegistry;
        use solvela_router::models::ModelRegistry;
        use tokio::sync::RwLock;

        let org_id = Uuid::new_v4();

        let model_registry = ModelRegistry::from_toml(super::test_helpers::TEST_MODELS_TOML)
            .expect("test models toml must be valid");
        let service_registry = ServiceRegistry::empty();
        let facilitator = x402::facilitator::Facilitator::new(vec![]);

        let state = Arc::new(crate::AppState {
            config: crate::config::AppConfig::default(),
            model_registry,
            service_registry: RwLock::new(service_registry),
            providers: crate::providers::ProviderRegistry::from_env(reqwest::Client::new()),
            facilitator,
            usage: crate::usage::UsageTracker::noop(),
            cache: None,
            provider_health: ProviderHealthTracker::new(CircuitBreakerConfig::default()),
            escrow_claimer: None,
            fee_payer_pool: None,
            nonce_pool: None,
            db_pool: None,
            session_secret: vec![0u8; 32],
            replay_set: crate::AppState::new_replay_set(),
            http_client: reqwest::Client::new(),
            slot_cache: crate::routes::escrow::new_slot_cache(),
            escrow_metrics: None,
            admin_token: None, // no admin token — forces API key auth path
            prometheus_handle: None,
            dev_bypass_payment: false,
        });

        let ctx = OrgContext {
            org_id,
            api_key_id: Uuid::new_v4(),
            role: OrgRole::Admin, // Admin, not Owner
        };

        let app = Router::new()
            .route("/v1/orgs/{id}/api-keys", post(super::super::create_api_key))
            .layer(axum::middleware::from_fn(
                move |mut req: axum::extract::Request, next: axum::middleware::Next| {
                    let ctx = ctx.clone();
                    async move {
                        req.extensions_mut().insert(ctx);
                        next.run(req).await
                    }
                },
            ))
            .with_state(state);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/orgs/{org_id}/api-keys"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"Escalated Key","role":"owner"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn create_api_key_name_too_long_returns_400() {
        let app = test_router(Some("mytoken"));
        let org_id = Uuid::new_v4();
        let long_name = "a".repeat(257);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/orgs/{org_id}/api-keys"))
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer mytoken")
                    .body(Body::from(
                        serde_json::json!({ "name": long_name }).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }
}
