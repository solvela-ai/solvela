//! HTTP-level integration tests for the org-management routes.
//!
//! Builds a minimal `Router` per test with `db_pool: Some(pool)` (from
//! `#[sqlx::test]`) and exercises the admin-token happy path of each
//! handler via `tower::ServiceExt::oneshot`. Auth-rejection paths are
//! already covered by inline tests in `src/routes/orgs/*`.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::{delete, get, post, put};
use axum::Router;
use http_body_util::BodyExt;
use serde_json::Value;
use sqlx::PgPool;
use tokio::sync::RwLock;
use tower::ServiceExt;
use uuid::Uuid;

use gateway::config::AppConfig;
use gateway::providers::health::{CircuitBreakerConfig, ProviderHealthTracker};
use gateway::providers::ProviderRegistry;
use gateway::routes::orgs::{
    add_member, assign_wallet, create_api_key, create_org, create_team, get_org, get_org_stats,
    get_team_budget, get_team_stats, get_wallet_budget, list_api_keys, list_audit_logs,
    list_members, list_orgs, list_team_wallets, list_teams, revoke_api_key, set_team_budget,
    set_wallet_budget,
};
use gateway::services::ServiceRegistry;
use gateway::usage::UsageTracker;
use gateway::AppState;
use solvela_router::models::ModelRegistry;

const ADMIN_TOKEN: &str = "test-admin-token";

// Valid 32-byte Solana base58 pubkeys for happy-path fixtures. The wrapped-SOL
// mint and the USDC-SPL mint are used as visually-distinct placeholders so the
// org owner and added-member rows are easy to tell apart in test output. The
// tightened `validate_wallet_address` in `src/routes/orgs/mod.rs` rejects
// anything that doesn't bs58-decode to exactly 32 bytes, so these constants
// have to be real pubkeys (see #173 L2 GW).
const TEST_OWNER_WALLET: &str = "So11111111111111111111111111111111111111112";
const TEST_MEMBER_WALLET: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
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

/// Build a Router with the org routes wired up against the supplied
/// per-test Postgres pool. Mirrors the production registration in `lib.rs`.
fn router_with_pool(pool: PgPool) -> Router {
    let model_registry = ModelRegistry::from_toml(TEST_MODELS_TOML).expect("models toml");
    let service_registry = ServiceRegistry::empty();
    let facilitator = solvela_x402::facilitator::Facilitator::new(vec![]);

    let state = Arc::new(AppState {
        config: AppConfig::default(),
        model_registry,
        service_registry: RwLock::new(service_registry),
        providers: ProviderRegistry::from_env(reqwest::Client::new()),
        facilitator,
        usage: UsageTracker::noop(),
        cache: None,
        provider_health: ProviderHealthTracker::new(CircuitBreakerConfig::default()),
        escrow_claimer: None,
        fee_payer_pool: None,
        nonce_pool: None,
        db_pool: Some(pool),
        session_secret: vec![0u8; 32],
        replay_set: AppState::new_replay_set(),
        http_client: reqwest::Client::new(),
        slot_cache: gateway::routes::escrow::new_slot_cache(),
        escrow_metrics: None,
        admin_token: Some(gateway::secret::AdminToken::new(ADMIN_TOKEN.to_string())),
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

fn auth_header() -> (&'static str, String) {
    ("authorization", format!("Bearer {ADMIN_TOKEN}"))
}

fn json_request(method: &str, uri: &str, body: &str) -> Request<Body> {
    let (k, v) = auth_header();
    Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header(k, v)
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn bare_request(method: &str, uri: &str) -> Request<Body> {
    let (k, v) = auth_header();
    Request::builder()
        .method(method)
        .uri(uri)
        .header(k, v)
        .body(Body::empty())
        .unwrap()
}

async fn body_to_json(resp: axum::response::Response) -> Value {
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).expect("body must be JSON")
}

async fn create_test_org(app: &Router, slug: &str) -> Uuid {
    let body =
        format!(r#"{{"name":"Org {slug}","slug":"{slug}","owner_wallet":"{TEST_OWNER_WALLET}"}}"#,);
    let resp = app
        .clone()
        .oneshot(json_request("POST", "/v1/orgs", &body))
        .await
        .expect("create_org");
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "create_org must succeed"
    );
    let json = body_to_json(resp).await;
    Uuid::parse_str(json["id"].as_str().expect("id field")).expect("uuid")
}

// ---------------------------------------------------------------------------
// /v1/orgs CRUD
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../../migrations")]
async fn create_org_returns_201_with_id(pool: PgPool) {
    let app = router_with_pool(pool);

    let resp = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/v1/orgs",
            &format!(r#"{{"name":"Acme","slug":"acme","owner_wallet":"{TEST_OWNER_WALLET}"}}"#,),
        ))
        .await
        .expect("call");

    assert_eq!(resp.status(), StatusCode::CREATED);
    let json = body_to_json(resp).await;
    assert_eq!(json["name"], "Acme");
    assert_eq!(json["slug"], "acme");
    assert_eq!(json["owner_wallet"], TEST_OWNER_WALLET);
    assert!(Uuid::parse_str(json["id"].as_str().unwrap()).is_ok());
}

#[sqlx::test(migrations = "../../migrations")]
async fn create_org_rejects_invalid_slug(pool: PgPool) {
    let app = router_with_pool(pool);

    let resp = app
        .oneshot(json_request(
            "POST",
            "/v1/orgs",
            r#"{"name":"Acme","slug":"bad_slug","owner_wallet":"acmewallet"}"#,
        ))
        .await
        .expect("call");

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = "../../migrations")]
async fn create_org_rejects_oversized_wallet(pool: PgPool) {
    let app = router_with_pool(pool);

    let big_wallet = "a".repeat(65);
    let body = format!(r#"{{"name":"Acme","slug":"acme","owner_wallet":"{big_wallet}"}}"#);

    let resp = app
        .oneshot(json_request("POST", "/v1/orgs", &body))
        .await
        .expect("call");

    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = "../../migrations")]
async fn list_orgs_returns_admin_view(pool: PgPool) {
    let app = router_with_pool(pool);
    let _id_a = create_test_org(&app, "first").await;
    let _id_b = create_test_org(&app, "second").await;

    let resp = app
        .oneshot(bare_request("GET", "/v1/orgs"))
        .await
        .expect("call");
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_to_json(resp).await;
    let arr = json.as_array().expect("array");
    assert_eq!(arr.len(), 2);
}

#[sqlx::test(migrations = "../../migrations")]
async fn get_org_returns_existing(pool: PgPool) {
    let app = router_with_pool(pool);
    let org_id = create_test_org(&app, "acme").await;

    let resp = app
        .oneshot(bare_request("GET", &format!("/v1/orgs/{org_id}")))
        .await
        .expect("call");
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_to_json(resp).await;
    assert_eq!(json["id"].as_str().unwrap(), org_id.to_string());
    assert_eq!(json["slug"], "acme");
}

#[sqlx::test(migrations = "../../migrations")]
async fn get_org_returns_404_for_unknown(pool: PgPool) {
    let app = router_with_pool(pool);
    let bogus = Uuid::new_v4();

    let resp = app
        .oneshot(bare_request("GET", &format!("/v1/orgs/{bogus}")))
        .await
        .expect("call");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// teams + members + team wallets
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../../migrations")]
async fn create_and_list_teams_via_http(pool: PgPool) {
    let app = router_with_pool(pool);
    let org_id = create_test_org(&app, "acme").await;

    let resp = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/v1/orgs/{org_id}/teams"),
            r#"{"name":"Engineering"}"#,
        ))
        .await
        .expect("create team");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let team_json = body_to_json(resp).await;
    assert_eq!(team_json["name"], "Engineering");
    let team_id = Uuid::parse_str(team_json["id"].as_str().unwrap()).expect("uuid");

    let resp = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/v1/orgs/{org_id}/teams"),
            r#"{"name":"Marketing"}"#,
        ))
        .await
        .expect("second team");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .oneshot(bare_request("GET", &format!("/v1/orgs/{org_id}/teams")))
        .await
        .expect("list teams");
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_to_json(resp).await;
    let teams = json.as_array().expect("array");
    assert_eq!(teams.len(), 2);
    assert!(teams
        .iter()
        .any(|t| t["id"].as_str().unwrap() == team_id.to_string()));
}

#[sqlx::test(migrations = "../../migrations")]
async fn create_team_rejects_blank_name(pool: PgPool) {
    let app = router_with_pool(pool);
    let org_id = create_test_org(&app, "acme").await;

    let resp = app
        .oneshot(json_request(
            "POST",
            &format!("/v1/orgs/{org_id}/teams"),
            r#"{"name":""}"#,
        ))
        .await
        .expect("call");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = "../../migrations")]
async fn add_and_list_members_via_http(pool: PgPool) {
    let app = router_with_pool(pool);
    let org_id = create_test_org(&app, "acme").await;

    let resp = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/v1/orgs/{org_id}/members"),
            &format!(r#"{{"wallet_address":"{TEST_MEMBER_WALLET}","role":"admin"}}"#),
        ))
        .await
        .expect("add member");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let json = body_to_json(resp).await;
    assert_eq!(json["wallet_address"], TEST_MEMBER_WALLET);
    assert_eq!(json["role"], "admin");

    let resp = app
        .oneshot(bare_request("GET", &format!("/v1/orgs/{org_id}/members")))
        .await
        .expect("list members");
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_to_json(resp).await;
    let members = json.as_array().unwrap();
    // Owner (auto-enrolled) + alice
    assert_eq!(members.len(), 2);
}

#[sqlx::test(migrations = "../../migrations")]
async fn assign_wallet_404s_when_team_not_in_org(pool: PgPool) {
    let app = router_with_pool(pool);
    let org_id = create_test_org(&app, "acme").await;
    let bogus_team = Uuid::new_v4();

    let resp = app
        .oneshot(json_request(
            "POST",
            &format!("/v1/orgs/{org_id}/teams/{bogus_team}/wallets"),
            &format!(r#"{{"wallet_address":"{TEST_MEMBER_WALLET}"}}"#),
        ))
        .await
        .expect("call");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
async fn assign_and_list_team_wallets_via_http(pool: PgPool) {
    let app = router_with_pool(pool);
    let org_id = create_test_org(&app, "acme").await;

    let resp = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/v1/orgs/{org_id}/teams"),
            r#"{"name":"Eng"}"#,
        ))
        .await
        .expect("team");
    let team_json = body_to_json(resp).await;
    let team_id = Uuid::parse_str(team_json["id"].as_str().unwrap()).expect("uuid");

    let resp = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/v1/orgs/{org_id}/teams/{team_id}/wallets"),
            &format!(r#"{{"wallet_address":"{TEST_MEMBER_WALLET}"}}"#),
        ))
        .await
        .expect("assign wallet");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let resp = app
        .oneshot(bare_request(
            "GET",
            &format!("/v1/orgs/{org_id}/teams/{team_id}/wallets"),
        ))
        .await
        .expect("list");
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_to_json(resp).await;
    let arr = json.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["wallet_address"], TEST_MEMBER_WALLET);
}

// ---------------------------------------------------------------------------
// /v1/orgs/:id/api-keys
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../../migrations")]
async fn create_list_revoke_api_key_round_trip(pool: PgPool) {
    let app = router_with_pool(pool);
    let org_id = create_test_org(&app, "acme").await;

    let resp = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/v1/orgs/{org_id}/api-keys"),
            r#"{"name":"ci-runner","role":"admin"}"#,
        ))
        .await
        .expect("create");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let json = body_to_json(resp).await;
    assert_eq!(json["name"], "ci-runner");
    assert_eq!(json["role"], "admin");
    let key_id = Uuid::parse_str(json["id"].as_str().unwrap()).expect("uuid");
    assert!(json["key"].as_str().unwrap().starts_with("solvela_k_"));

    let resp = app
        .clone()
        .oneshot(bare_request("GET", &format!("/v1/orgs/{org_id}/api-keys")))
        .await
        .expect("list");
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_to_json(resp).await;
    let arr = json.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["id"].as_str().unwrap(), key_id.to_string());

    let resp = app
        .clone()
        .oneshot(bare_request(
            "DELETE",
            &format!("/v1/orgs/{org_id}/api-keys/{key_id}"),
        ))
        .await
        .expect("revoke");
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_to_json(resp).await;
    assert_eq!(json["revoked"], true);

    let resp = app
        .clone()
        .oneshot(bare_request(
            "DELETE",
            &format!("/v1/orgs/{org_id}/api-keys/{key_id}"),
        ))
        .await
        .expect("second revoke");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let resp = app
        .oneshot(bare_request("GET", &format!("/v1/orgs/{org_id}/api-keys")))
        .await
        .expect("list after revoke");
    let json = body_to_json(resp).await;
    assert_eq!(json.as_array().unwrap().len(), 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn create_api_key_rejects_blank_name(pool: PgPool) {
    let app = router_with_pool(pool);
    let org_id = create_test_org(&app, "acme").await;

    let resp = app
        .oneshot(json_request(
            "POST",
            &format!("/v1/orgs/{org_id}/api-keys"),
            r#"{"name":"   "}"#,
        ))
        .await
        .expect("call");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

// ---------------------------------------------------------------------------
// budgets
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../../migrations")]
async fn set_and_get_team_budget_via_http(pool: PgPool) {
    let app = router_with_pool(pool);
    let org_id = create_test_org(&app, "acme").await;

    let resp = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/v1/orgs/{org_id}/teams"),
            r#"{"name":"Eng"}"#,
        ))
        .await
        .expect("team");
    let team_id = Uuid::parse_str(body_to_json(resp).await["id"].as_str().unwrap()).expect("uuid");

    let resp = app
        .clone()
        .oneshot(json_request(
            "PUT",
            &format!("/v1/orgs/{org_id}/teams/{team_id}/budget"),
            r#"{"hourly":1.0,"daily":10.0,"monthly":100.0}"#,
        ))
        .await
        .expect("set budget");
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(bare_request(
            "GET",
            &format!("/v1/orgs/{org_id}/teams/{team_id}/budget"),
        ))
        .await
        .expect("get budget");
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_to_json(resp).await;
    assert_eq!(json["hourly_limit"], 1.0);
    assert_eq!(json["daily_limit"], 10.0);
    assert_eq!(json["monthly_limit"], 100.0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn set_team_budget_rejects_negative_value(pool: PgPool) {
    let app = router_with_pool(pool);
    let org_id = create_test_org(&app, "acme").await;
    let resp = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/v1/orgs/{org_id}/teams"),
            r#"{"name":"Eng"}"#,
        ))
        .await
        .expect("team");
    let team_id = Uuid::parse_str(body_to_json(resp).await["id"].as_str().unwrap()).expect("uuid");

    let resp = app
        .oneshot(json_request(
            "PUT",
            &format!("/v1/orgs/{org_id}/teams/{team_id}/budget"),
            r#"{"daily":-1.0}"#,
        ))
        .await
        .expect("call");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = "../../migrations")]
async fn set_team_budget_404s_when_team_not_in_org(pool: PgPool) {
    let app = router_with_pool(pool);
    let org_id = create_test_org(&app, "acme").await;
    let bogus_team = Uuid::new_v4();

    let resp = app
        .oneshot(json_request(
            "PUT",
            &format!("/v1/orgs/{org_id}/teams/{bogus_team}/budget"),
            r#"{"daily":1.0}"#,
        ))
        .await
        .expect("call");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = "../../migrations")]
async fn set_and_get_wallet_budget_via_http(pool: PgPool) {
    let app = router_with_pool(pool);

    let wallet = "alice";

    let resp = app
        .clone()
        .oneshot(json_request(
            "PUT",
            &format!("/v1/wallets/{wallet}/budget"),
            r#"{"hourly":0.5,"daily":5.0,"monthly":50.0}"#,
        ))
        .await
        .expect("set");
    assert_eq!(resp.status(), StatusCode::OK);

    let resp = app
        .oneshot(bare_request("GET", &format!("/v1/wallets/{wallet}/budget")))
        .await
        .expect("get");
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_to_json(resp).await;
    assert_eq!(json["hourly_limit"], 0.5);
    assert_eq!(json["daily_limit"], 5.0);
    assert_eq!(json["monthly_limit"], 50.0);
    assert_eq!(json["hourly_spend"], 0.0);
    assert_eq!(json["daily_spend"], 0.0);
    assert_eq!(json["monthly_spend"], 0.0);
}

// ---------------------------------------------------------------------------
// audit logs + stats (smoke: handler reaches DB and returns JSON shape)
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../../migrations")]
async fn audit_logs_endpoint_returns_json_array(pool: PgPool) {
    let app = router_with_pool(pool);
    let org_id = create_test_org(&app, "acme").await;

    let resp = app
        .oneshot(bare_request(
            "GET",
            &format!("/v1/orgs/{org_id}/audit-logs?limit=10"),
        ))
        .await
        .expect("audit logs");
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_to_json(resp).await;
    assert!(json.is_array(), "audit-logs response must be an array");
}

#[sqlx::test(migrations = "../../migrations")]
async fn org_stats_endpoint_returns_200(pool: PgPool) {
    let app = router_with_pool(pool);
    let org_id = create_test_org(&app, "acme").await;

    let resp = app
        .oneshot(bare_request("GET", &format!("/v1/orgs/{org_id}/stats")))
        .await
        .expect("org stats");
    assert_eq!(resp.status(), StatusCode::OK);
    let _json = body_to_json(resp).await;
}
