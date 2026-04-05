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

/// `GET /v1/orgs` — List organizations.
///
/// Admin: returns all orgs (first 100).
/// API key: returns only the org associated with the key.
pub async fn list_orgs(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    org_ctx: Option<Extension<OrgContext>>,
) -> Response {
    let auth = match require_auth(&state, &headers, org_ctx.as_ref().map(|e| &e.0)) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    let pool = require_db!(state);

    match &auth {
        AuthContext::Admin => {
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
        AuthContext::OrgKey(ctx) => match queries::get_org(pool, ctx.org_id).await {
            Ok(Some(org)) => (StatusCode::OK, Json(vec![org])).into_response(),
            Ok(None) => (
                StatusCode::OK,
                Json(Vec::<crate::orgs::models::Organization>::new()),
            )
                .into_response(),
            Err(e) => {
                tracing::warn!(org_id = %ctx.org_id, error = %e, "failed to list orgs for api key");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "error": "failed to list organizations" })),
                )
                    .into_response()
            }
        },
    }
}

/// `GET /v1/orgs/:id` — Get an organization by ID.
pub async fn get_org(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    org_ctx: Option<Extension<OrgContext>>,
    Path(id): Path<Uuid>,
) -> Response {
    let auth = match require_auth(&state, &headers, org_ctx.as_ref().map(|e| &e.0)) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_org_access(&auth, id) {
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
