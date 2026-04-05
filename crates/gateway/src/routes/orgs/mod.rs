//! Organization and team management REST API.
//!
//! Endpoints accept **either** a global admin token (`Authorization: Bearer <admin_token>`)
//! or an org-scoped API key (`Authorization: Bearer rcr_k_...`).
//! Database (`db_pool`) must be configured — returns 503 otherwise.

mod analytics;
mod api_keys;
mod audit;
mod budget;
mod crud;
mod teams;

pub use analytics::*;
pub use api_keys::*;
pub use audit::*;
pub use budget::*;
pub use crud::*;
pub use teams::*;

use std::sync::Arc;

use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::{DateTime, Utc};
use redis;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;

use crate::audit::{log_audit, AuditEntry};
use crate::middleware::api_key::OrgContext;
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

// ---------------------------------------------------------------------------
// Dual auth: admin token OR org-scoped API key
// ---------------------------------------------------------------------------

/// Authentication context: either a global admin or an org-scoped API key.
pub(crate) enum AuthContext {
    /// Global admin — unrestricted access to all orgs.
    Admin,
    /// Org-scoped API key — can only access the associated org.
    OrgKey(OrgContext),
}

/// Authenticate the request: accept either admin token or org API key.
/// Returns `Err(Response)` with 401 if neither is present.
#[allow(clippy::result_large_err)]
pub(crate) fn require_auth(
    state: &AppState,
    headers: &HeaderMap,
    org_ctx: Option<&OrgContext>,
) -> Result<AuthContext, Response> {
    // Try admin token first
    if let Some(admin_token) = &state.admin_token {
        let is_admin = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .is_some_and(|token| {
                security::constant_time_eq(token.as_bytes(), admin_token.as_bytes())
            });
        if is_admin {
            return Ok(AuthContext::Admin);
        }
    }
    // Try org API key
    if let Some(ctx) = org_ctx {
        return Ok(AuthContext::OrgKey(ctx.clone()));
    }
    Err((
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": { "type": "unauthorized", "message": "Valid admin token or API key required" } })),
    )
        .into_response())
}

/// Verify the auth context has access to the given org_id.
/// Admin has access to everything. OrgKey must match the org_id.
#[allow(clippy::result_large_err)]
pub(crate) fn require_org_access(auth: &AuthContext, org_id: Uuid) -> Result<(), Response> {
    match auth {
        AuthContext::Admin => Ok(()),
        AuthContext::OrgKey(ctx) if ctx.org_id == org_id => Ok(()),
        AuthContext::OrgKey(_) => Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": { "type": "forbidden", "message": "API key not scoped to this organization" } })),
        )
            .into_response()),
    }
}

/// Verify the auth context has admin/owner access for the given org.
/// Global admin always passes. OrgKey must match org AND have admin/owner role.
#[allow(clippy::result_large_err)]
pub(crate) fn require_org_admin_access(auth: &AuthContext, org_id: Uuid) -> Result<(), Response> {
    match auth {
        AuthContext::Admin => Ok(()),
        AuthContext::OrgKey(ctx) if ctx.org_id == org_id && ctx.role.is_admin_or_owner() => Ok(()),
        AuthContext::OrgKey(ctx) if ctx.org_id != org_id => Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": { "type": "forbidden", "message": "API key not scoped to this organization" } })),
        )
            .into_response()),
        AuthContext::OrgKey(_) => Err((
            StatusCode::FORBIDDEN,
            Json(json!({ "error": { "type": "forbidden", "message": "Admin or owner role required" } })),
        )
            .into_response()),
    }
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

// Make macro available to submodules.
pub(crate) use require_db;

// ---------------------------------------------------------------------------
// Input validation helpers
// ---------------------------------------------------------------------------

fn validate_slug(slug: &str) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    if slug.is_empty() || slug.len() > 64 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "slug must be 1-64 characters"})),
        ));
    }
    if !slug
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "slug must contain only lowercase letters, digits, and hyphens"})),
        ));
    }
    if slug.starts_with('-') || slug.ends_with('-') {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "slug must not start or end with a hyphen"})),
        ));
    }
    Ok(())
}

fn validate_name(name: &str, field: &str) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.len() > 256 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": format!("{} must be 1-256 characters", field)})),
        ));
    }
    Ok(())
}

fn validate_wallet_address(addr: &str) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    if addr.is_empty() || addr.len() > 64 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({"error": "wallet address must be 1-64 characters"})),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod test_helpers {
    use std::sync::Arc;

    use axum::routing::{delete, get, post, put};
    use axum::Router;

    use super::*;
    use crate::AppState;

    pub(crate) const TEST_MODELS_TOML: &str = r#"
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
    pub(crate) fn test_router(admin_token: Option<&str>) -> Router {
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
            .route("/v1/orgs/{id}/teams/{tid}/stats", get(get_team_stats))
            .route("/v1/orgs/{id}/stats", get(get_org_stats))
            .with_state(state)
    }
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use super::test_helpers::test_router;

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
    async fn list_orgs_no_admin_token_configured_returns_401() {
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

        // No admin token configured AND no API key -> 401
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
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

        // db_pool is None -> 503
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    // -----------------------------------------------------------------------
    // Dual auth (require_auth / require_org_access / require_org_admin_access)
    // -----------------------------------------------------------------------

    #[test]
    fn require_auth_admin_token_succeeds() {
        use super::*;
        use crate::config::AppConfig;

        let state = AppState {
            config: AppConfig::default(),
            model_registry: router::models::ModelRegistry::from_toml(
                super::test_helpers::TEST_MODELS_TOML,
            )
            .unwrap(),
            service_registry: tokio::sync::RwLock::new(crate::services::ServiceRegistry::empty()),
            providers: crate::providers::ProviderRegistry::from_env(reqwest::Client::new()),
            facilitator: x402::facilitator::Facilitator::new(vec![]),
            usage: crate::usage::UsageTracker::noop(),
            cache: None,
            provider_health: crate::providers::health::ProviderHealthTracker::new(
                crate::providers::health::CircuitBreakerConfig::default(),
            ),
            escrow_claimer: None,
            fee_payer_pool: None,
            nonce_pool: None,
            db_pool: None,
            session_secret: vec![0u8; 32],
            replay_set: AppState::new_replay_set(),
            http_client: reqwest::Client::new(),
            slot_cache: crate::routes::escrow::new_slot_cache(),
            escrow_metrics: None,
            admin_token: Some("admin-secret".to_string()),
            prometheus_handle: None,
            dev_bypass_payment: false,
        };

        let mut headers = HeaderMap::new();
        headers.insert("authorization", "Bearer admin-secret".parse().unwrap());

        let result = require_auth(&state, &headers, None);
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), AuthContext::Admin));
    }

    #[test]
    fn require_auth_api_key_succeeds() {
        use super::*;
        use crate::config::AppConfig;
        use crate::middleware::api_key::OrgContext;
        use crate::orgs::models::OrgRole;

        let state = AppState {
            config: AppConfig::default(),
            model_registry: router::models::ModelRegistry::from_toml(
                super::test_helpers::TEST_MODELS_TOML,
            )
            .unwrap(),
            service_registry: tokio::sync::RwLock::new(crate::services::ServiceRegistry::empty()),
            providers: crate::providers::ProviderRegistry::from_env(reqwest::Client::new()),
            facilitator: x402::facilitator::Facilitator::new(vec![]),
            usage: crate::usage::UsageTracker::noop(),
            cache: None,
            provider_health: crate::providers::health::ProviderHealthTracker::new(
                crate::providers::health::CircuitBreakerConfig::default(),
            ),
            escrow_claimer: None,
            fee_payer_pool: None,
            nonce_pool: None,
            db_pool: None,
            session_secret: vec![0u8; 32],
            replay_set: AppState::new_replay_set(),
            http_client: reqwest::Client::new(),
            slot_cache: crate::routes::escrow::new_slot_cache(),
            escrow_metrics: None,
            admin_token: Some("admin-secret".to_string()),
            prometheus_handle: None,
            dev_bypass_payment: false,
        };

        let org_id = uuid::Uuid::new_v4();
        let ctx = OrgContext {
            org_id,
            api_key_id: uuid::Uuid::new_v4(),
            role: OrgRole::Member,
        };
        let headers = HeaderMap::new(); // no admin token header

        let result = require_auth(&state, &headers, Some(&ctx));
        assert!(result.is_ok());
        assert!(matches!(result.unwrap(), AuthContext::OrgKey(_)));
    }

    #[test]
    fn require_auth_neither_returns_401() {
        use super::*;
        use crate::config::AppConfig;

        let state = AppState {
            config: AppConfig::default(),
            model_registry: router::models::ModelRegistry::from_toml(
                super::test_helpers::TEST_MODELS_TOML,
            )
            .unwrap(),
            service_registry: tokio::sync::RwLock::new(crate::services::ServiceRegistry::empty()),
            providers: crate::providers::ProviderRegistry::from_env(reqwest::Client::new()),
            facilitator: x402::facilitator::Facilitator::new(vec![]),
            usage: crate::usage::UsageTracker::noop(),
            cache: None,
            provider_health: crate::providers::health::ProviderHealthTracker::new(
                crate::providers::health::CircuitBreakerConfig::default(),
            ),
            escrow_claimer: None,
            fee_payer_pool: None,
            nonce_pool: None,
            db_pool: None,
            session_secret: vec![0u8; 32],
            replay_set: AppState::new_replay_set(),
            http_client: reqwest::Client::new(),
            slot_cache: crate::routes::escrow::new_slot_cache(),
            escrow_metrics: None,
            admin_token: Some("admin-secret".to_string()),
            prometheus_handle: None,
            dev_bypass_payment: false,
        };

        let headers = HeaderMap::new();
        let result = require_auth(&state, &headers, None);
        assert!(result.is_err());
    }

    #[test]
    fn require_org_access_admin_always_passes() {
        use super::*;

        let org_id = uuid::Uuid::new_v4();
        let auth = AuthContext::Admin;
        assert!(require_org_access(&auth, org_id).is_ok());
    }

    #[test]
    fn require_org_access_matching_org_passes() {
        use super::*;
        use crate::middleware::api_key::OrgContext;
        use crate::orgs::models::OrgRole;

        let org_id = uuid::Uuid::new_v4();
        let auth = AuthContext::OrgKey(OrgContext {
            org_id,
            api_key_id: uuid::Uuid::new_v4(),
            role: OrgRole::Member,
        });
        assert!(require_org_access(&auth, org_id).is_ok());
    }

    #[test]
    fn require_org_access_wrong_org_returns_403() {
        use super::*;
        use crate::middleware::api_key::OrgContext;
        use crate::orgs::models::OrgRole;

        let auth = AuthContext::OrgKey(OrgContext {
            org_id: uuid::Uuid::new_v4(),
            api_key_id: uuid::Uuid::new_v4(),
            role: OrgRole::Member,
        });
        let other_org_id = uuid::Uuid::new_v4();
        assert!(require_org_access(&auth, other_org_id).is_err());
    }

    #[test]
    fn require_org_admin_access_member_role_returns_403() {
        use super::*;
        use crate::middleware::api_key::OrgContext;
        use crate::orgs::models::OrgRole;

        let org_id = uuid::Uuid::new_v4();
        let auth = AuthContext::OrgKey(OrgContext {
            org_id,
            api_key_id: uuid::Uuid::new_v4(),
            role: OrgRole::Member,
        });
        assert!(require_org_admin_access(&auth, org_id).is_err());
    }

    #[test]
    fn require_org_admin_access_admin_role_passes() {
        use super::*;
        use crate::middleware::api_key::OrgContext;
        use crate::orgs::models::OrgRole;

        let org_id = uuid::Uuid::new_v4();
        let auth = AuthContext::OrgKey(OrgContext {
            org_id,
            api_key_id: uuid::Uuid::new_v4(),
            role: OrgRole::Admin,
        });
        assert!(require_org_admin_access(&auth, org_id).is_ok());
    }

    #[test]
    fn require_org_admin_access_owner_role_passes() {
        use super::*;
        use crate::middleware::api_key::OrgContext;
        use crate::orgs::models::OrgRole;

        let org_id = uuid::Uuid::new_v4();
        let auth = AuthContext::OrgKey(OrgContext {
            org_id,
            api_key_id: uuid::Uuid::new_v4(),
            role: OrgRole::Owner,
        });
        assert!(require_org_admin_access(&auth, org_id).is_ok());
    }

    #[test]
    fn require_org_admin_access_wrong_org_returns_403() {
        use super::*;
        use crate::middleware::api_key::OrgContext;
        use crate::orgs::models::OrgRole;

        let auth = AuthContext::OrgKey(OrgContext {
            org_id: uuid::Uuid::new_v4(),
            api_key_id: uuid::Uuid::new_v4(),
            role: OrgRole::Owner,
        });
        let other_org_id = uuid::Uuid::new_v4();
        assert!(require_org_admin_access(&auth, other_org_id).is_err());
    }

    // -----------------------------------------------------------------------
    // Input validation: validate_slug
    // -----------------------------------------------------------------------

    #[test]
    fn validate_slug_empty_returns_err() {
        use super::validate_slug;
        assert!(validate_slug("").is_err());
    }

    #[test]
    fn validate_slug_too_long_returns_err() {
        use super::validate_slug;
        let slug = "a".repeat(65);
        assert!(validate_slug(&slug).is_err());
    }

    #[test]
    fn validate_slug_uppercase_returns_err() {
        use super::validate_slug;
        assert!(validate_slug("MyOrg").is_err());
    }

    #[test]
    fn validate_slug_special_chars_returns_err() {
        use super::validate_slug;
        assert!(validate_slug("my_org").is_err());
        assert!(validate_slug("my org").is_err());
        assert!(validate_slug("my@org").is_err());
    }

    #[test]
    fn validate_slug_leading_hyphen_returns_err() {
        use super::validate_slug;
        assert!(validate_slug("-myorg").is_err());
    }

    #[test]
    fn validate_slug_trailing_hyphen_returns_err() {
        use super::validate_slug;
        assert!(validate_slug("myorg-").is_err());
    }

    #[test]
    fn validate_slug_valid() {
        use super::validate_slug;
        assert!(validate_slug("my-org").is_ok());
        assert!(validate_slug("acme").is_ok());
        assert!(validate_slug("org123").is_ok());
        assert!(validate_slug("my-org-2").is_ok());
    }

    // -----------------------------------------------------------------------
    // Input validation: validate_name
    // -----------------------------------------------------------------------

    #[test]
    fn validate_name_empty_returns_err() {
        use super::validate_name;
        assert!(validate_name("", "name").is_err());
    }

    #[test]
    fn validate_name_whitespace_only_returns_err() {
        use super::validate_name;
        assert!(validate_name("   ", "name").is_err());
    }

    #[test]
    fn validate_name_too_long_returns_err() {
        use super::validate_name;
        let name = "a".repeat(257);
        assert!(validate_name(&name, "name").is_err());
    }

    #[test]
    fn validate_name_valid() {
        use super::validate_name;
        assert!(validate_name("Acme Corp", "name").is_ok());
        assert!(validate_name("a", "name").is_ok());
        assert!(validate_name(&"a".repeat(256), "name").is_ok());
    }

    // -----------------------------------------------------------------------
    // Input validation: validate_wallet_address
    // -----------------------------------------------------------------------

    #[test]
    fn validate_wallet_address_empty_returns_err() {
        use super::validate_wallet_address;
        assert!(validate_wallet_address("").is_err());
    }

    #[test]
    fn validate_wallet_address_too_long_returns_err() {
        use super::validate_wallet_address;
        let addr = "a".repeat(65);
        assert!(validate_wallet_address(&addr).is_err());
    }

    #[test]
    fn validate_wallet_address_valid() {
        use super::validate_wallet_address;
        assert!(validate_wallet_address("So11111111111111111111111111111111111111112").is_ok());
        assert!(validate_wallet_address(&"a".repeat(64)).is_ok());
    }
}
