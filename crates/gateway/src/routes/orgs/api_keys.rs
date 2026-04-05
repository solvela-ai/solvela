//! API key management handlers.

use super::*;

/// `POST /v1/orgs/:id/api-keys` — Create an API key for an organization.
pub async fn create_api_key(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(org_id): Path<Uuid>,
    Json(body): Json<CreateApiKeyRequest>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers) {
        return resp;
    }
    let pool = require_db!(state);

    match queries::create_api_key(pool, org_id, body).await {
        Ok(created) => {
            log_audit(
                pool,
                AuditEntry {
                    org_id: Some(org_id),
                    actor_wallet: None,
                    actor_api_key: None,
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
    Path(org_id): Path<Uuid>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers) {
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
    Path((org_id, key_id)): Path<(Uuid, Uuid)>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers) {
        return resp;
    }
    let pool = require_db!(state);

    match queries::revoke_api_key(pool, key_id, org_id).await {
        Ok(true) => {
            log_audit(
                pool,
                AuditEntry {
                    org_id: Some(org_id),
                    actor_wallet: None,
                    actor_api_key: None,
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
}
