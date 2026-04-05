//! Organization CRUD handlers.

use super::*;

/// `POST /v1/orgs` — Create a new organization.
pub async fn create_org(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<CreateOrgFullRequest>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers) {
        return resp;
    }

    if let Err((status, err)) = validate_name(&body.name, "name") {
        return (status, err).into_response();
    }
    if let Err((status, err)) = validate_slug(&body.slug) {
        return (status, err).into_response();
    }
    if let Err((status, err)) = validate_wallet_address(&body.owner_wallet) {
        return (status, err).into_response();
    }

    let pool = require_db!(state);

    let req = CreateOrgRequest {
        name: body.name,
        slug: body.slug,
    };

    match queries::create_org(pool, req, body.owner_wallet).await {
        Ok(org) => {
            log_audit(
                pool,
                AuditEntry {
                    org_id: Some(org.id),
                    actor_wallet: None,
                    actor_api_key: None,
                    action: "org.created".to_string(),
                    resource_type: "organization".to_string(),
                    resource_id: Some(org.id.to_string()),
                    details: None,
                    ip_address: None,
                },
            );
            (StatusCode::CREATED, Json(org)).into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to create org");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to create organization" })),
            )
                .into_response()
        }
    }
}

/// `GET /v1/orgs` — List all organizations (admin view, first 100).
pub async fn list_orgs(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    if let Err(resp) = require_admin(&state, &headers) {
        return resp;
    }
    let pool = require_db!(state);

    match sqlx::query_as::<_, crate::orgs::models::Organization>(
        r#"
        SELECT id, name, slug, owner_wallet, created_at, updated_at
        FROM organizations
        ORDER BY created_at ASC
        LIMIT 100
        "#,
    )
    .fetch_all(pool)
    .await
    {
        Ok(orgs) => (StatusCode::OK, Json(orgs)).into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "failed to list orgs");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to list organizations" })),
            )
                .into_response()
        }
    }
}

/// `GET /v1/orgs/:id` — Get an organization by ID.
pub async fn get_org(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers) {
        return resp;
    }
    let pool = require_db!(state);

    match queries::get_org(pool, id).await {
        Ok(Some(org)) => (StatusCode::OK, Json(org)).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "organization not found" })),
        )
            .into_response(),
        Err(e) => {
            tracing::warn!(org_id = %id, error = %e, "failed to get org");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to get organization" })),
            )
                .into_response()
        }
    }
}
