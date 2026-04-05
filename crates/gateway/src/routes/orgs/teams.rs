//! Team, member, and team-wallet handlers.

use super::*;

/// `POST /v1/orgs/:id/teams` — Create a new team within an organization.
pub async fn create_team(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(org_id): Path<Uuid>,
    Json(body): Json<CreateTeamRequest>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers) {
        return resp;
    }

    if let Err((status, err)) = validate_name(&body.name, "name") {
        return (status, err).into_response();
    }

    let pool = require_db!(state);

    match queries::create_team(pool, org_id, body).await {
        Ok(team) => {
            log_audit(
                pool,
                AuditEntry {
                    org_id: Some(org_id),
                    actor_wallet: None,
                    actor_api_key: None,
                    action: "team.created".to_string(),
                    resource_type: "team".to_string(),
                    resource_id: Some(team.id.to_string()),
                    details: None,
                    ip_address: None,
                },
            );
            (StatusCode::CREATED, Json(team)).into_response()
        }
        Err(e) => {
            tracing::warn!(org_id = %org_id, error = %e, "failed to create team");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to create team" })),
            )
                .into_response()
        }
    }
}

/// `GET /v1/orgs/:id/teams` — List all teams in an organization.
pub async fn list_teams(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(org_id): Path<Uuid>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers) {
        return resp;
    }
    let pool = require_db!(state);

    match queries::list_teams(pool, org_id).await {
        Ok(teams) => (StatusCode::OK, Json(teams)).into_response(),
        Err(e) => {
            tracing::warn!(org_id = %org_id, error = %e, "failed to list teams");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to list teams" })),
            )
                .into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// Member endpoints
// ---------------------------------------------------------------------------

/// `POST /v1/orgs/:id/members` — Add a wallet as an org member.
pub async fn add_member(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(org_id): Path<Uuid>,
    Json(body): Json<AddMemberRequest>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers) {
        return resp;
    }

    if let Err((status, err)) = validate_wallet_address(&body.wallet_address) {
        return (status, err).into_response();
    }

    let pool = require_db!(state);

    match queries::add_member(pool, org_id, body).await {
        Ok(member) => {
            log_audit(
                pool,
                AuditEntry {
                    org_id: Some(org_id),
                    actor_wallet: None,
                    actor_api_key: None,
                    action: "member.added".to_string(),
                    resource_type: "org_member".to_string(),
                    resource_id: Some(member.id.to_string()),
                    details: None,
                    ip_address: None,
                },
            );
            (StatusCode::CREATED, Json(member)).into_response()
        }
        Err(e) => {
            tracing::warn!(org_id = %org_id, error = %e, "failed to add member");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to add member" })),
            )
                .into_response()
        }
    }
}

/// `GET /v1/orgs/:id/members` — List all members of an organization.
pub async fn list_members(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(org_id): Path<Uuid>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers) {
        return resp;
    }
    let pool = require_db!(state);

    match queries::list_members(pool, org_id).await {
        Ok(members) => (StatusCode::OK, Json(members)).into_response(),
        Err(e) => {
            tracing::warn!(org_id = %org_id, error = %e, "failed to list members");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to list members" })),
            )
                .into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// Team wallet endpoints
// ---------------------------------------------------------------------------

/// `POST /v1/orgs/:id/teams/:tid/wallets` — Assign a wallet to a team.
pub async fn assign_wallet(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org_id, team_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<AssignWalletRequest>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers) {
        return resp;
    }

    if let Err((status, err)) = validate_wallet_address(&body.wallet_address) {
        return (status, err).into_response();
    }

    let pool = require_db!(state);

    // Verify the team belongs to the org
    let team_exists: bool = match sqlx::query_as::<_, (bool,)>(
        "SELECT EXISTS(SELECT 1 FROM teams WHERE id = $1 AND org_id = $2)",
    )
    .bind(team_id)
    .bind(org_id)
    .fetch_one(pool)
    .await
    {
        Ok((exists,)) => exists,
        Err(e) => {
            tracing::warn!(team_id = %team_id, error = %e, "failed to verify team");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to verify team" })),
            )
                .into_response();
        }
    };

    if !team_exists {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "team not found in this organization" })),
        )
            .into_response();
    }

    match queries::assign_wallet(pool, team_id, &body).await {
        Ok(wallet) => {
            log_audit(
                pool,
                AuditEntry {
                    org_id: Some(org_id),
                    actor_wallet: None,
                    actor_api_key: None,
                    action: "wallet.assigned".to_string(),
                    resource_type: "team_wallet".to_string(),
                    resource_id: Some(wallet.id.to_string()),
                    details: None,
                    ip_address: None,
                },
            );
            (StatusCode::CREATED, Json(wallet)).into_response()
        }
        Err(e) => {
            tracing::warn!(team_id = %team_id, error = %e, "failed to assign wallet");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to assign wallet" })),
            )
                .into_response()
        }
    }
}

/// `GET /v1/orgs/:id/teams/:tid/wallets` — List wallets assigned to a team.
pub async fn list_team_wallets(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((org_id, team_id)): Path<(Uuid, Uuid)>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers) {
        return resp;
    }
    let pool = require_db!(state);

    // Verify the team belongs to the org
    let team_exists: bool = match sqlx::query_as::<_, (bool,)>(
        "SELECT EXISTS(SELECT 1 FROM teams WHERE id = $1 AND org_id = $2)",
    )
    .bind(team_id)
    .bind(org_id)
    .fetch_one(pool)
    .await
    {
        Ok((exists,)) => exists,
        Err(e) => {
            tracing::warn!(team_id = %team_id, error = %e, "failed to verify team");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to verify team" })),
            )
                .into_response();
        }
    };

    if !team_exists {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "team not found in this organization" })),
        )
            .into_response();
    }

    match queries::list_team_wallets(pool, team_id).await {
        Ok(wallets) => (StatusCode::OK, Json(wallets)).into_response(),
        Err(e) => {
            tracing::warn!(team_id = %team_id, error = %e, "failed to list team wallets");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to list team wallets" })),
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
    async fn create_team_requires_auth() {
        let app = test_router(Some("tok"));
        let id = Uuid::new_v4();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/orgs/{id}/teams"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"Engineering"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn create_team_no_db_returns_503() {
        let app = test_router(Some("mytoken"));
        let org_id = Uuid::new_v4();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/v1/orgs/{org_id}/teams"))
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer mytoken")
                    .body(Body::from(r#"{"name":"Engineering"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        // db_pool is None -> 503
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
