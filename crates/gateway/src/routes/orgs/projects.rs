//! Per-project budget cap CRUD handlers.
//!
//! Endpoints (all org-scoped, dual auth admin token OR org API key):
//! - `POST /v1/orgs/:id/projects`               — create
//! - `GET  /v1/orgs/:id/projects`               — list
//! - `GET  /v1/orgs/:id/projects/:pid`          — fetch one
//! - `PUT  /v1/orgs/:id/projects/:pid`          — update name/cap
//! - `DELETE /v1/orgs/:id/projects/:pid`        — delete
//!
//! Wiring `try_charge` into the chat hot path is a follow-up — see
//! `crates/gateway/src/orgs/budget_projects.rs`.

use super::*;

use crate::orgs::budget_projects::{
    BudgetProjectError, BudgetProjectRepo, CreateBudgetProjectRequest, UpdateBudgetProjectRequest,
};

fn map_repo_error(err: BudgetProjectError) -> Response {
    match err {
        BudgetProjectError::NotFound => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "budget project not found" })),
        )
            .into_response(),
        BudgetProjectError::Invalid(reason) => {
            (StatusCode::BAD_REQUEST, Json(json!({ "error": reason }))).into_response()
        }
        BudgetProjectError::BudgetExceeded {
            requested,
            remaining,
        } => (
            StatusCode::PAYMENT_REQUIRED,
            Json(json!({
                "error": "budget exceeded",
                "requested": requested,
                "remaining": remaining,
            })),
        )
            .into_response(),
        BudgetProjectError::Db(e) => {
            tracing::warn!(error = %e, "budget_projects db error");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "database error" })),
            )
                .into_response()
        }
    }
}

/// `POST /v1/orgs/:id/projects` — create a new budget project.
pub async fn create_project(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    org_ctx: Option<Extension<OrgContext>>,
    Path(org_id): Path<Uuid>,
    Json(body): Json<CreateBudgetProjectRequest>,
) -> Response {
    let auth = match require_auth(&state, &headers, org_ctx.as_ref().map(|e| &e.0)) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_org_admin_access(&auth, org_id) {
        return resp;
    }

    if let Err((status, err)) = validate_name(&body.name, "name") {
        return (status, err).into_response();
    }
    if body.budget_usd_atomic < 0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "budget_usd_atomic must be >= 0" })),
        )
            .into_response();
    }

    let pool = require_db!(state);

    match BudgetProjectRepo::create(pool, org_id, &body).await {
        Ok(project) => {
            let actor_api_key = match &auth {
                AuthContext::OrgKey(ctx) => Some(ctx.api_key_id),
                AuthContext::Admin => None,
            };
            log_audit(
                pool,
                AuditEntry {
                    org_id: Some(org_id),
                    actor_wallet: None,
                    actor_api_key,
                    action: "project.created".to_string(),
                    resource_type: "budget_project".to_string(),
                    resource_id: Some(project.id.to_string()),
                    details: Some(serde_json::json!({
                        "name": project.name,
                        "budget_usd_atomic": project.budget_usd_atomic,
                    })),
                    ip_address: None,
                },
            );
            (StatusCode::CREATED, Json(project)).into_response()
        }
        Err(e) => map_repo_error(e),
    }
}

/// `GET /v1/orgs/:id/projects` — list all projects for an org.
pub async fn list_projects(
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

    match BudgetProjectRepo::list(pool, org_id).await {
        Ok(projects) => (StatusCode::OK, Json(projects)).into_response(),
        Err(e) => map_repo_error(e),
    }
}

/// `GET /v1/orgs/:id/projects/:pid` — get one project.
pub async fn get_project(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    org_ctx: Option<Extension<OrgContext>>,
    Path((org_id, project_id)): Path<(Uuid, Uuid)>,
) -> Response {
    let auth = match require_auth(&state, &headers, org_ctx.as_ref().map(|e| &e.0)) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_org_access(&auth, org_id) {
        return resp;
    }

    let pool = require_db!(state);

    match BudgetProjectRepo::get(pool, org_id, project_id).await {
        Ok(Some(project)) => (StatusCode::OK, Json(project)).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "budget project not found" })),
        )
            .into_response(),
        Err(e) => map_repo_error(e),
    }
}

/// `PUT /v1/orgs/:id/projects/:pid` — update name and/or budget cap.
pub async fn update_project(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    org_ctx: Option<Extension<OrgContext>>,
    Path((org_id, project_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<UpdateBudgetProjectRequest>,
) -> Response {
    let auth = match require_auth(&state, &headers, org_ctx.as_ref().map(|e| &e.0)) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_org_admin_access(&auth, org_id) {
        return resp;
    }

    if let Some(name) = &body.name {
        if let Err((status, err)) = validate_name(name, "name") {
            return (status, err).into_response();
        }
    }
    if let Some(b) = body.budget_usd_atomic {
        if b < 0 {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": "budget_usd_atomic must be >= 0" })),
            )
                .into_response();
        }
    }

    let pool = require_db!(state);

    match BudgetProjectRepo::update(pool, org_id, project_id, &body).await {
        Ok(project) => {
            let actor_api_key = match &auth {
                AuthContext::OrgKey(ctx) => Some(ctx.api_key_id),
                AuthContext::Admin => None,
            };
            log_audit(
                pool,
                AuditEntry {
                    org_id: Some(org_id),
                    actor_wallet: None,
                    actor_api_key,
                    action: "project.updated".to_string(),
                    resource_type: "budget_project".to_string(),
                    resource_id: Some(project.id.to_string()),
                    details: serde_json::to_value(&body).ok(),
                    ip_address: None,
                },
            );
            (StatusCode::OK, Json(project)).into_response()
        }
        Err(e) => map_repo_error(e),
    }
}

/// `DELETE /v1/orgs/:id/projects/:pid` — delete a project.
pub async fn delete_project(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    org_ctx: Option<Extension<OrgContext>>,
    Path((org_id, project_id)): Path<(Uuid, Uuid)>,
) -> Response {
    let auth = match require_auth(&state, &headers, org_ctx.as_ref().map(|e| &e.0)) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_org_admin_access(&auth, org_id) {
        return resp;
    }

    let pool = require_db!(state);

    match BudgetProjectRepo::delete(pool, org_id, project_id).await {
        Ok(true) => {
            let actor_api_key = match &auth {
                AuthContext::OrgKey(ctx) => Some(ctx.api_key_id),
                AuthContext::Admin => None,
            };
            log_audit(
                pool,
                AuditEntry {
                    org_id: Some(org_id),
                    actor_wallet: None,
                    actor_api_key,
                    action: "project.deleted".to_string(),
                    resource_type: "budget_project".to_string(),
                    resource_id: Some(project_id.to_string()),
                    details: None,
                    ip_address: None,
                },
            );
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "budget project not found" })),
        )
            .into_response(),
        Err(e) => map_repo_error(e),
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
    async fn create_project_requires_auth() {
        let app = test_router(Some("tok"));
        let org_id = Uuid::new_v4();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/orgs/{org_id}/projects"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"alpha","budget_usd_atomic":1000}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn list_projects_requires_auth() {
        let app = test_router(Some("tok"));
        let org_id = Uuid::new_v4();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/orgs/{org_id}/projects"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn create_project_no_db_returns_503() {
        let app = test_router(Some("mytoken"));
        let org_id = Uuid::new_v4();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/orgs/{org_id}/projects"))
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer mytoken")
                    .body(Body::from(r#"{"name":"alpha","budget_usd_atomic":1000}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn list_projects_no_db_returns_503() {
        let app = test_router(Some("mytoken"));
        let org_id = Uuid::new_v4();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/orgs/{org_id}/projects"))
                    .header("authorization", "Bearer mytoken")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn get_project_no_db_returns_503() {
        let app = test_router(Some("mytoken"));
        let org_id = Uuid::new_v4();
        let pid = Uuid::new_v4();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/orgs/{org_id}/projects/{pid}"))
                    .header("authorization", "Bearer mytoken")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn update_project_validates_negative_budget() {
        // No DB attached, but the negative-budget guard runs before
        // require_db!, so we expect 400 not 503.
        let app = test_router(Some("mytoken"));
        let org_id = Uuid::new_v4();
        let pid = Uuid::new_v4();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/v1/orgs/{org_id}/projects/{pid}"))
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer mytoken")
                    .body(Body::from(r#"{"budget_usd_atomic":-5}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn delete_project_no_db_returns_503() {
        let app = test_router(Some("mytoken"));
        let org_id = Uuid::new_v4();
        let pid = Uuid::new_v4();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/v1/orgs/{org_id}/projects/{pid}"))
                    .header("authorization", "Bearer mytoken")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
