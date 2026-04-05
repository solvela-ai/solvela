//! Organization and team management REST API.
//!
//! All endpoints require admin token authentication via `Authorization: Bearer <token>`.
//! Database (`db_pool`) must be configured — returns 503 otherwise.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::audit::{log_audit, AuditEntry};
use crate::orgs::models::{
    AddMemberRequest, AssignWalletRequest, CreateApiKeyRequest, CreateOrgRequest, CreateTeamRequest,
};
use crate::orgs::queries;
use crate::security;
use crate::AppState;

/// Query parameters for the audit log endpoint.
#[derive(Debug, Deserialize)]
pub struct AuditLogQuery {
    /// Filter by action string (exact match).
    pub action: Option<String>,
    /// Return only events after this ISO 8601 timestamp.
    pub since: Option<DateTime<Utc>>,
    /// Maximum number of results (default 100, max 1000).
    #[serde(default)]
    pub limit: Option<i64>,
}

/// A single audit log entry returned from the database.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct AuditLogEntry {
    pub id: Uuid,
    pub org_id: Option<Uuid>,
    pub actor_wallet: Option<String>,
    pub actor_api_key: Option<Uuid>,
    pub action: String,
    pub resource_type: String,
    pub resource_id: Option<String>,
    pub details: Option<serde_json::Value>,
    pub ip_address: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Full request body for creating an organization (includes `owner_wallet`).
#[derive(Debug, Deserialize)]
pub struct CreateOrgFullRequest {
    pub name: String,
    pub slug: String,
    pub owner_wallet: String,
}

// ---------------------------------------------------------------------------
// Auth helper
// ---------------------------------------------------------------------------

/// Validate the `Authorization: Bearer <token>` header against the configured
/// admin token. Returns `Err(Response)` with the appropriate status code when
/// auth fails or the endpoint is not configured.
#[allow(clippy::result_large_err)]
fn require_admin(state: &AppState, headers: &HeaderMap) -> Result<(), Response> {
    let admin_token = match &state.admin_token {
        Some(t) => t,
        None => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "admin endpoint not configured" })),
            )
                .into_response());
        }
    };

    let authorized = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|token| security::constant_time_eq(token.as_bytes(), admin_token.as_bytes()));

    if !authorized {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response());
    }

    Ok(())
}

/// Retrieve the database pool or return 503.
macro_rules! require_db {
    ($state:expr) => {
        match &$state.db_pool {
            Some(pool) => pool,
            None => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({ "error": "database not configured" })),
                )
                    .into_response();
            }
        }
    };
}

// ---------------------------------------------------------------------------
// Organization endpoints
// ---------------------------------------------------------------------------

/// `POST /v1/orgs` — Create a new organization.
pub async fn create_org(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<CreateOrgFullRequest>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers) {
        return resp;
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

// ---------------------------------------------------------------------------
// Team endpoints
// ---------------------------------------------------------------------------

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
    Path((_org_id, team_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<AssignWalletRequest>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers) {
        return resp;
    }
    let pool = require_db!(state);

    match queries::assign_wallet(pool, team_id, &body).await {
        Ok(wallet) => {
            log_audit(
                pool,
                AuditEntry {
                    org_id: None,
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
    Path((_org_id, team_id)): Path<(Uuid, Uuid)>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers) {
        return resp;
    }
    let pool = require_db!(state);

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

// ---------------------------------------------------------------------------
// API key endpoints
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Audit log endpoint
// ---------------------------------------------------------------------------

/// `GET /v1/orgs/:id/audit-logs` — List audit log entries for an organization.
///
/// Query parameters:
/// - `action`  — filter by action string (exact match)
/// - `since`   — ISO 8601 timestamp; return only events after this time
/// - `limit`   — max results (default 100, capped at 1000)
pub async fn list_audit_logs(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(org_id): Path<Uuid>,
    Query(params): Query<AuditLogQuery>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers) {
        return resp;
    }
    let pool = require_db!(state);

    let limit = params.limit.unwrap_or(100).clamp(1, 1000);

    // Build query dynamically based on optional filters.
    // Using a fixed-shape query avoids sqlx macro limitations with optional clauses.
    let entries = sqlx::query_as::<_, AuditLogEntry>(
        r#"SELECT id, org_id, actor_wallet, actor_api_key, action, resource_type,
                  resource_id, details, ip_address, created_at
           FROM audit_logs
           WHERE org_id = $1
             AND ($2::TEXT IS NULL OR action = $2)
             AND ($3::TIMESTAMPTZ IS NULL OR created_at > $3)
           ORDER BY created_at DESC
           LIMIT $4"#,
    )
    .bind(org_id)
    .bind(&params.action)
    .bind(params.since)
    .bind(limit)
    .fetch_all(pool)
    .await;

    match entries {
        Ok(rows) => (StatusCode::OK, Json(rows)).into_response(),
        Err(e) => {
            tracing::warn!(org_id = %org_id, error = %e, "failed to list audit logs");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to list audit logs" })),
            )
                .into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// Budget management endpoints
// ---------------------------------------------------------------------------

/// Request body for setting budget limits.
#[derive(Debug, Deserialize)]
pub struct SetBudgetRequest {
    pub hourly: Option<f64>,
    pub daily: Option<f64>,
    pub monthly: Option<f64>,
}

/// Response body for budget endpoints (limits + current spend).
#[derive(Debug, Serialize)]
pub struct BudgetResponse {
    pub hourly_limit: Option<f64>,
    pub daily_limit: Option<f64>,
    pub monthly_limit: Option<f64>,
    pub hourly_spend: f64,
    pub daily_spend: f64,
    pub monthly_spend: f64,
}

/// `PUT /v1/orgs/:id/teams/:tid/budget` — Set team budget limits.
pub async fn set_team_budget(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((_org_id, team_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<SetBudgetRequest>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers) {
        return resp;
    }
    let pool = require_db!(state);

    let now = chrono::Utc::now();
    let result = sqlx::query(
        r#"INSERT INTO team_budgets (team_id, hourly_limit_usdc, daily_limit_usdc, monthly_limit_usdc, created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, $6)
           ON CONFLICT (team_id) DO UPDATE SET
               hourly_limit_usdc = EXCLUDED.hourly_limit_usdc,
               daily_limit_usdc = EXCLUDED.daily_limit_usdc,
               monthly_limit_usdc = EXCLUDED.monthly_limit_usdc,
               updated_at = EXCLUDED.updated_at"#,
    )
    .bind(team_id)
    .bind(body.hourly)
    .bind(body.daily)
    .bind(body.monthly)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await;

    match result {
        Ok(_) => (StatusCode::OK, Json(json!({ "updated": true }))).into_response(),
        Err(e) => {
            tracing::warn!(team_id = %team_id, error = %e, "failed to set team budget");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to set team budget" })),
            )
                .into_response()
        }
    }
}

/// `GET /v1/orgs/:id/teams/:tid/budget` — Get team budget + current spend.
pub async fn get_team_budget(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((_org_id, team_id)): Path<(Uuid, Uuid)>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers) {
        return resp;
    }
    let pool = require_db!(state);

    let row = sqlx::query_as::<_, (Option<f64>, Option<f64>, Option<f64>)>(
        r#"SELECT
            hourly_limit_usdc::DOUBLE PRECISION,
            daily_limit_usdc::DOUBLE PRECISION,
            monthly_limit_usdc::DOUBLE PRECISION
        FROM team_budgets
        WHERE team_id = $1"#,
    )
    .bind(team_id)
    .fetch_optional(pool)
    .await;

    let (hourly_limit, daily_limit, monthly_limit) = match row {
        Ok(Some(r)) => r,
        Ok(None) => (None, None, None),
        Err(e) => {
            tracing::warn!(team_id = %team_id, error = %e, "failed to query team budget");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to query team budget" })),
            )
                .into_response();
        }
    };

    let now = chrono::Utc::now();
    let tid_str = team_id.to_string();
    let (hourly_spend, daily_spend, monthly_spend) =
        if let Some(client) = &state.usage.redis_client() {
            let hour_key = format!("team_spend:{}:{}", tid_str, now.format("%Y-%m-%dT%H"));
            let day_key = format!("team_spend:{}:{}", tid_str, now.format("%Y-%m-%d"));
            let month_key = format!("team_spend:{}:{}", tid_str, now.format("%Y-%m"));
            (
                crate::usage::get_redis_spend(client, &hour_key)
                    .await
                    .unwrap_or(0.0),
                crate::usage::get_redis_spend(client, &day_key)
                    .await
                    .unwrap_or(0.0),
                crate::usage::get_redis_spend(client, &month_key)
                    .await
                    .unwrap_or(0.0),
            )
        } else {
            (0.0, 0.0, 0.0)
        };

    (
        StatusCode::OK,
        Json(BudgetResponse {
            hourly_limit,
            daily_limit,
            monthly_limit,
            hourly_spend,
            daily_spend,
            monthly_spend,
        }),
    )
        .into_response()
}

/// `PUT /v1/wallets/:wallet/budget` — Set wallet budget limits.
pub async fn set_wallet_budget(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(wallet): Path<String>,
    Json(body): Json<SetBudgetRequest>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers) {
        return resp;
    }
    let pool = require_db!(state);

    let now = chrono::Utc::now();
    let result = sqlx::query(
        r#"INSERT INTO wallet_budgets (wallet_address, hourly_limit_usdc, daily_limit_usdc, monthly_limit_usdc, created_at)
           VALUES ($1, $2, $3, $4, $5)
           ON CONFLICT (wallet_address) DO UPDATE SET
               hourly_limit_usdc = EXCLUDED.hourly_limit_usdc,
               daily_limit_usdc = EXCLUDED.daily_limit_usdc,
               monthly_limit_usdc = EXCLUDED.monthly_limit_usdc"#,
    )
    .bind(&wallet)
    .bind(body.hourly)
    .bind(body.daily)
    .bind(body.monthly)
    .bind(now)
    .execute(pool)
    .await;

    match result {
        Ok(_) => (StatusCode::OK, Json(json!({ "updated": true }))).into_response(),
        Err(e) => {
            tracing::warn!(wallet = %wallet, error = %e, "failed to set wallet budget");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to set wallet budget" })),
            )
                .into_response()
        }
    }
}

/// `GET /v1/wallets/:wallet/budget` — Get wallet budget + current spend.
pub async fn get_wallet_budget(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(wallet): Path<String>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers) {
        return resp;
    }
    let pool = require_db!(state);

    let row = sqlx::query_as::<_, (Option<f64>, Option<f64>, Option<f64>)>(
        r#"SELECT
            hourly_limit_usdc::DOUBLE PRECISION,
            daily_limit_usdc::DOUBLE PRECISION,
            monthly_limit_usdc::DOUBLE PRECISION
        FROM wallet_budgets
        WHERE wallet_address = $1"#,
    )
    .bind(&wallet)
    .fetch_optional(pool)
    .await;

    let (hourly_limit, daily_limit, monthly_limit) = match row {
        Ok(Some(r)) => r,
        Ok(None) => (None, Some(100.0), None), // Default $100/day
        Err(e) => {
            tracing::warn!(wallet = %wallet, error = %e, "failed to query wallet budget");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to query wallet budget" })),
            )
                .into_response();
        }
    };

    let now = chrono::Utc::now();
    let (hourly_spend, daily_spend, monthly_spend) =
        if let Some(client) = &state.usage.redis_client() {
            let hour_key = format!("spend:{}:{}", wallet, now.format("%Y-%m-%dT%H"));
            let day_key = format!("spend:{}:{}", wallet, now.format("%Y-%m-%d"));
            let month_key = format!("spend:{}:{}", wallet, now.format("%Y-%m"));
            (
                crate::usage::get_redis_spend(client, &hour_key)
                    .await
                    .unwrap_or(0.0),
                crate::usage::get_redis_spend(client, &day_key)
                    .await
                    .unwrap_or(0.0),
                crate::usage::get_redis_spend(client, &month_key)
                    .await
                    .unwrap_or(0.0),
            )
        } else {
            (0.0, 0.0, 0.0)
        };

    (
        StatusCode::OK,
        Json(BudgetResponse {
            hourly_limit,
            daily_limit,
            monthly_limit,
            hourly_spend,
            daily_spend,
            monthly_spend,
        }),
    )
        .into_response()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::{delete, get, post, put};
    use axum::Router;
    use tower::ServiceExt;

    use super::*;
    use crate::AppState;

    const TEST_MODELS_TOML: &str = r#"
[models.openai-gpt-4o]
provider = "openai"
model_id = "gpt-4o"
display_name = "GPT-4o"
input_cost_per_million = 2.50
output_cost_per_million = 10.00
context_window = 128000
supports_streaming = true
supports_tools = true
supports_vision = true
"#;

    /// Build a minimal test router with only the orgs routes and a fake admin token.
    fn test_router(admin_token: Option<&str>) -> Router {
        use crate::providers::health::{CircuitBreakerConfig, ProviderHealthTracker};
        use crate::services::ServiceRegistry;
        use router::models::ModelRegistry;
        use tokio::sync::RwLock;

        let model_registry =
            ModelRegistry::from_toml(TEST_MODELS_TOML).expect("test models toml must be valid");
        let service_registry = ServiceRegistry::empty();
        let facilitator = x402::facilitator::Facilitator::new(vec![]);

        let state = Arc::new(AppState {
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
            replay_set: AppState::new_replay_set(),
            http_client: reqwest::Client::new(),
            slot_cache: crate::routes::escrow::new_slot_cache(),
            escrow_metrics: None,
            admin_token: admin_token.map(String::from),
            prometheus_handle: None,
            dev_bypass_payment: false,
        });

        Router::new()
            .route("/v1/orgs", post(create_org).get(list_orgs))
            .route("/v1/orgs/{id}", get(get_org))
            .route("/v1/orgs/{id}/teams", post(create_team).get(list_teams))
            .route("/v1/orgs/{id}/members", post(add_member).get(list_members))
            .route(
                "/v1/orgs/{id}/teams/{tid}/wallets",
                post(assign_wallet).get(list_team_wallets),
            )
            .route(
                "/v1/orgs/{id}/api-keys",
                post(create_api_key).get(list_api_keys),
            )
            .route("/v1/orgs/{id}/api-keys/{kid}", delete(revoke_api_key))
            .route("/v1/orgs/{id}/audit-logs", get(list_audit_logs))
            .route(
                "/v1/orgs/{id}/teams/{tid}/budget",
                put(set_team_budget).get(get_team_budget),
            )
            .route(
                "/v1/wallets/{wallet}/budget",
                put(set_wallet_budget).get(get_wallet_budget),
            )
            .with_state(state)
    }

    // -----------------------------------------------------------------------
    // Auth tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn create_org_missing_token_returns_401() {
        let app = test_router(Some("secret-token"));

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/orgs")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"Acme","slug":"acme","owner_wallet":"WalletABC"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn create_org_wrong_token_returns_401() {
        let app = test_router(Some("secret-token"));

        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/v1/orgs")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer wrong-token")
                    .body(Body::from(
                        r#"{"name":"Acme","slug":"acme","owner_wallet":"WalletABC"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn list_orgs_no_admin_token_configured_returns_503() {
        let app = test_router(None);

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/orgs")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn list_orgs_valid_token_no_db_returns_503() {
        let app = test_router(Some("mytoken"));

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/orgs")
                    .header("authorization", "Bearer mytoken")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // db_pool is None → 503
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

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

        // db_pool is None → 503
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
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

        // db_pool is None → 503
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn audit_logs_missing_token_returns_401() {
        let app = test_router(Some("secret-token"));
        let org_id = Uuid::new_v4();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/orgs/{org_id}/audit-logs"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn audit_logs_valid_token_no_db_returns_503() {
        let app = test_router(Some("mytoken"));
        let org_id = Uuid::new_v4();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/orgs/{org_id}/audit-logs"))
                    .header("authorization", "Bearer mytoken")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // db_pool is None → 503
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    // -----------------------------------------------------------------------
    // Budget endpoint tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn set_team_budget_requires_auth() {
        let app = test_router(Some("tok"));
        let org_id = Uuid::new_v4();
        let team_id = Uuid::new_v4();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/v1/orgs/{org_id}/teams/{team_id}/budget"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"daily":200.0}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn get_team_budget_requires_auth() {
        let app = test_router(Some("tok"));
        let org_id = Uuid::new_v4();
        let team_id = Uuid::new_v4();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/orgs/{org_id}/teams/{team_id}/budget"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn set_team_budget_no_db_returns_503() {
        let app = test_router(Some("mytoken"));
        let org_id = Uuid::new_v4();
        let team_id = Uuid::new_v4();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/v1/orgs/{org_id}/teams/{team_id}/budget"))
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer mytoken")
                    .body(Body::from(r#"{"daily":200.0}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn get_team_budget_no_db_returns_503() {
        let app = test_router(Some("mytoken"));
        let org_id = Uuid::new_v4();
        let team_id = Uuid::new_v4();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/orgs/{org_id}/teams/{team_id}/budget"))
                    .header("authorization", "Bearer mytoken")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn set_wallet_budget_requires_auth() {
        let app = test_router(Some("tok"));

        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/v1/wallets/WalletABC/budget")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"daily":50.0}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn get_wallet_budget_requires_auth() {
        let app = test_router(Some("tok"));

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/wallets/WalletABC/budget")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn set_wallet_budget_no_db_returns_503() {
        let app = test_router(Some("mytoken"));

        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/v1/wallets/WalletABC/budget")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer mytoken")
                    .body(Body::from(r#"{"daily":50.0}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn get_wallet_budget_no_db_returns_503() {
        let app = test_router(Some("mytoken"));

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/wallets/WalletABC/budget")
                    .header("authorization", "Bearer mytoken")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
