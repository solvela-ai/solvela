//! Organization and team management REST API.
//!
//! All endpoints require admin token authentication via `Authorization: Bearer <token>`.
//! Database (`db_pool`) must be configured — returns 503 otherwise.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use crate::orgs::models::{
    AddMemberRequest, AssignWalletRequest, CreateApiKeyRequest, CreateOrgRequest,
    CreateTeamRequest,
};
use crate::orgs::queries;
use crate::security;
use crate::AppState;

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
        Ok(org) => (StatusCode::OK, Json(org)).into_response(),
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
pub async fn list_orgs(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Response {
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
        Ok(team) => (StatusCode::OK, Json(team)).into_response(),
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
        Ok(member) => (StatusCode::OK, Json(member)).into_response(),
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
        Ok(wallet) => (StatusCode::OK, Json(wallet)).into_response(),
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
        Ok(created) => (StatusCode::OK, Json(created)).into_response(),
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
        Ok(true) => (StatusCode::OK, Json(json!({ "revoked": true }))).into_response(),
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
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::{delete, get, post};
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

        let model_registry = ModelRegistry::from_toml(TEST_MODELS_TOML)
            .expect("test models toml must be valid");
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
            .route(
                "/v1/orgs/{id}/teams",
                post(create_team).get(list_teams),
            )
            .route(
                "/v1/orgs/{id}/members",
                post(add_member).get(list_members),
            )
            .route(
                "/v1/orgs/{id}/teams/{tid}/wallets",
                post(assign_wallet).get(list_team_wallets),
            )
            .route(
                "/v1/orgs/{id}/api-keys",
                post(create_api_key).get(list_api_keys),
            )
            .route(
                "/v1/orgs/{id}/api-keys/{kid}",
                delete(revoke_api_key),
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
}
