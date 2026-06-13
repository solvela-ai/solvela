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
        semantic_cache: None,
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
        api_key_hmac_secret: None,
        prometheus_handle: None,
        dev_bypass_payment: false,
        free_rate_limiter: gateway::middleware::rate_limit::RateLimiter::new(
            gateway::middleware::rate_limit::RateLimitConfig::free_default(),
        ),
        receipts_rate_limiter: gateway::middleware::rate_limit::RateLimiter::new(
            gateway::middleware::rate_limit::RateLimitConfig::receipts_default(),
        ),
        free_global_cap: gateway::middleware::rate_limit::FreeTierGlobalCap::new(
            gateway::middleware::rate_limit::FREE_TIER_GLOBAL_RPM_DEFAULT,
        ),
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

/// Gap 3: the team-stats response must include a `by_provider` breakdown,
/// grouped on `spend_logs.provider` (joined through `team_wallets`), mirroring
/// the existing `by_model` breakdown. Seed spend rows spanning two providers
/// and assert the array aggregates correctly and is ordered by spend DESC.
#[sqlx::test(migrations = "../../migrations")]
async fn team_stats_includes_by_provider_breakdown(pool: PgPool) {
    let app = router_with_pool(pool.clone());
    let org_id = create_test_org(&app, "acme").await;

    // Create a team via HTTP.
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

    // Assign a wallet to the team via HTTP.
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

    // Seed spend rows spanning two providers for that wallet.
    // anthropic: 0.30 + 0.20 = 0.50 over 2 requests; openai: 0.10 over 1.
    for (model, provider, cost) in [
        ("anthropic/claude-sonnet-4", "anthropic", 0.30_f64),
        ("anthropic/claude-haiku", "anthropic", 0.20_f64),
        ("openai/gpt-4o", "openai", 0.10_f64),
    ] {
        sqlx::query(
            r#"INSERT INTO spend_logs
                   (wallet_address, model, provider, input_tokens, output_tokens, cost_usdc)
               VALUES ($1, $2, $3, 10, 20, $4)"#,
        )
        .bind(TEST_MEMBER_WALLET)
        .bind(model)
        .bind(provider)
        .bind(cost)
        .execute(&pool)
        .await
        .expect("seed spend_log");
    }

    let resp = app
        .oneshot(bare_request(
            "GET",
            &format!("/v1/orgs/{org_id}/teams/{team_id}/stats?days=7"),
        ))
        .await
        .expect("team stats");
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_to_json(resp).await;

    let by_provider = json["by_provider"]
        .as_array()
        .expect("by_provider must be an array");
    assert_eq!(by_provider.len(), 2, "two distinct providers seeded");

    // Ordered by total_cost_usdc DESC: anthropic (0.50) before openai (0.10).
    assert_eq!(by_provider[0]["provider"], "anthropic");
    assert_eq!(by_provider[0]["request_count"], 2);
    assert!(
        (by_provider[0]["total_cost_usdc"].as_f64().unwrap() - 0.50).abs() < 1e-9,
        "anthropic total must aggregate to 0.50, got {}",
        by_provider[0]["total_cost_usdc"]
    );

    assert_eq!(by_provider[1]["provider"], "openai");
    assert_eq!(by_provider[1]["request_count"], 1);
    assert!(
        (by_provider[1]["total_cost_usdc"].as_f64().unwrap() - 0.10).abs() < 1e-9,
        "openai total must aggregate to 0.10, got {}",
        by_provider[1]["total_cost_usdc"]
    );
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

// ---------------------------------------------------------------------------
// E1: org budget rollups + analytics polish
//
// Shared helpers for the org-stats E1 tests below: create a team, assign a
// wallet, set a team budget, and seed a raw spend row. All go through the real
// HTTP handlers where one exists (so the test exercises the production wiring),
// and direct SQL only for spend_logs (there is no public spend-write route).
// ---------------------------------------------------------------------------

async fn mk_team(app: &Router, org_id: Uuid, name: &str) -> Uuid {
    let resp = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/v1/orgs/{org_id}/teams"),
            &format!(r#"{{"name":"{name}"}}"#),
        ))
        .await
        .expect("create team");
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "team create must succeed"
    );
    Uuid::parse_str(body_to_json(resp).await["id"].as_str().unwrap()).expect("uuid")
}

async fn assign_team_wallet(app: &Router, org_id: Uuid, team_id: Uuid, wallet: &str) {
    let resp = app
        .clone()
        .oneshot(json_request(
            "POST",
            &format!("/v1/orgs/{org_id}/teams/{team_id}/wallets"),
            &format!(r#"{{"wallet_address":"{wallet}"}}"#),
        ))
        .await
        .expect("assign wallet");
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "wallet assign must succeed"
    );
}

async fn set_team_budget_http(app: &Router, org_id: Uuid, team_id: Uuid, body: &str) {
    let resp = app
        .clone()
        .oneshot(json_request(
            "PUT",
            &format!("/v1/orgs/{org_id}/teams/{team_id}/budget"),
            body,
        ))
        .await
        .expect("set budget");
    assert_eq!(resp.status(), StatusCode::OK, "budget set must succeed");
}

/// Seed a plain (non-vendor) chat spend row.
async fn seed_spend(pool: &PgPool, wallet: &str, model: &str, provider: &str, cost: f64) {
    sqlx::query(
        r#"INSERT INTO spend_logs
               (wallet_address, model, provider, input_tokens, output_tokens, cost_usdc)
           VALUES ($1, $2, $3, 10, 20, $4)"#,
    )
    .bind(wallet)
    .bind(model)
    .bind(provider)
    .bind(cost)
    .execute(pool)
    .await
    .expect("seed spend_log");
}

/// Seed a spend row with a tenant tag and vendor-settlement legs.
#[allow(clippy::too_many_arguments)]
async fn seed_spend_full(
    pool: &PgPool,
    wallet: &str,
    model: &str,
    provider: &str,
    cost: f64,
    tenant: Option<&str>,
    vendor_wallet: Option<&str>,
    vendor_settled_atomic: Option<i64>,
    vendor_fee_receivable_atomic: Option<i64>,
) {
    sqlx::query(
        r#"INSERT INTO spend_logs
               (wallet_address, model, provider, input_tokens, output_tokens, cost_usdc,
                tenant, vendor_wallet, vendor_settled_atomic, vendor_fee_receivable_atomic)
           VALUES ($1, $2, $3, 10, 20, $4, $5, $6, $7, $8)"#,
    )
    .bind(wallet)
    .bind(model)
    .bind(provider)
    .bind(cost)
    .bind(tenant)
    .bind(vendor_wallet)
    .bind(vendor_settled_atomic)
    .bind(vendor_fee_receivable_atomic)
    .execute(pool)
    .await
    .expect("seed spend_log full");
}

/// Item 2 (the headline bug): a wallet assigned to TWO teams of the same org,
/// with ONE paid request, must be counted ONCE in the org summary total — not
/// doubled by the spend_logs -> team_wallets fan-out. by_team legitimately
/// shows the row under both teams (each team's slice is real membership), but
/// the org-level total_requests / total_spend must not double.
#[sqlx::test(migrations = "../../migrations")]
async fn org_stats_does_not_double_count_multi_team_wallet(pool: PgPool) {
    let app = router_with_pool(pool.clone());
    let org_id = create_test_org(&app, "acme").await;

    let team_a = mk_team(&app, org_id, "Eng").await;
    let team_b = mk_team(&app, org_id, "Ops").await;
    // Same wallet in BOTH teams.
    assign_team_wallet(&app, org_id, team_a, TEST_MEMBER_WALLET).await;
    assign_team_wallet(&app, org_id, team_b, TEST_MEMBER_WALLET).await;

    // Exactly one paid request.
    seed_spend(&pool, TEST_MEMBER_WALLET, "openai/gpt-4o", "openai", 0.40).await;

    let resp = app
        .oneshot(bare_request("GET", &format!("/v1/orgs/{org_id}/stats")))
        .await
        .expect("org stats");
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_to_json(resp).await;

    // The summary must count the single row once, despite two-team membership.
    assert_eq!(json["total_requests"], 1, "row counted once at org level");
    assert!(
        (json["total_spend_usdc"].as_f64().unwrap() - 0.40).abs() < 1e-9,
        "org total must be 0.40 (not doubled), got {}",
        json["total_spend_usdc"]
    );

    // top_wallets must also dedupe: one wallet, one row, count 1, spend 0.40.
    let top = json["top_wallets"].as_array().expect("top_wallets array");
    assert_eq!(top.len(), 1, "one distinct wallet");
    assert_eq!(top[0]["request_count"], 1, "wallet row counted once");
    assert!(
        (top[0]["total_cost_usdc"].as_f64().unwrap() - 0.40).abs() < 1e-9,
        "top wallet spend not doubled"
    );

    // by_team legitimately attributes the row under both teams.
    let by_team = json["by_team"].as_array().expect("by_team array");
    assert_eq!(by_team.len(), 2, "both teams present");
    for t in by_team {
        assert_eq!(
            t["request_count"], 1,
            "each team sees the membership row once"
        );
    }

    // Counterpart to the dedupe above: by_team INTENTIONALLY attributes the
    // shared wallet's 0.40 under BOTH teams, so the sum of by_team totals
    // (0.40 + 0.40 = 0.80) deliberately EXCEEDS the deduped org summary total
    // (0.40). Pinning the exact sum guards documented semantics against a future
    // refactor that dedupes by_team (which would silently pass every other
    // assertion here).
    let by_team_sum: f64 = by_team
        .iter()
        .map(|t| t["total_cost_usdc"].as_f64().expect("total_cost_usdc"))
        .sum();
    assert!(
        (by_team_sum - 0.80).abs() < 1e-9,
        "by_team totals must sum to 0.80 (shared wallet counted under each team), got {by_team_sum}"
    );
    assert!(
        by_team_sum > json["total_spend_usdc"].as_f64().unwrap(),
        "by_team sum must exceed the deduped org summary total"
    );
}

/// Item 1: org budget-vs-spend is the SUM of the teams' team_budgets limits per
/// period, with absent budgets treated as unlimited (contributing nothing) and
/// surfaced via a teams-with-budget count so the figure is not silently
/// misleading.
#[sqlx::test(migrations = "../../migrations")]
async fn org_stats_budget_is_sum_of_team_budgets(pool: PgPool) {
    let app = router_with_pool(pool.clone());
    let org_id = create_test_org(&app, "acme").await;

    let team_a = mk_team(&app, org_id, "Eng").await;
    let team_b = mk_team(&app, org_id, "Ops").await;
    let _team_c = mk_team(&app, org_id, "NoBudget").await;

    // team_a: daily 10, monthly 100 (no hourly). team_b: hourly 1, daily 5.
    set_team_budget_http(&app, org_id, team_a, r#"{"daily":10.0,"monthly":100.0}"#).await;
    set_team_budget_http(&app, org_id, team_b, r#"{"hourly":1.0,"daily":5.0}"#).await;
    // team_c: no budget row at all -> contributes nothing.

    let resp = app
        .oneshot(bare_request("GET", &format!("/v1/orgs/{org_id}/stats")))
        .await
        .expect("org stats");
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_to_json(resp).await;

    let budget = &json["budget"];
    // hourly: only team_b set (1.0); 1 of 3 teams.
    assert!((budget["hourly_limit_usdc"].as_f64().unwrap() - 1.0).abs() < 1e-9);
    assert_eq!(budget["teams_with_hourly_budget"], 1);
    // daily: 10 + 5 = 15; 2 of 3 teams.
    assert!((budget["daily_limit_usdc"].as_f64().unwrap() - 15.0).abs() < 1e-9);
    assert_eq!(budget["teams_with_daily_budget"], 2);
    // monthly: only team_a set (100.0); 1 of 3 teams.
    assert!((budget["monthly_limit_usdc"].as_f64().unwrap() - 100.0).abs() < 1e-9);
    assert_eq!(budget["teams_with_monthly_budget"], 1);
    // total teams in the org for context.
    assert_eq!(budget["teams_total"], 3);
}

/// Item 1 (empty case): an org with no team budgets at all reports NULL limits
/// and zero teams-with-budget, never a misleading 0.0 that looks like a cap.
#[sqlx::test(migrations = "../../migrations")]
async fn org_stats_budget_absent_reports_null_not_zero(pool: PgPool) {
    let app = router_with_pool(pool.clone());
    let org_id = create_test_org(&app, "acme").await;
    let _team = mk_team(&app, org_id, "Eng").await;

    let resp = app
        .oneshot(bare_request("GET", &format!("/v1/orgs/{org_id}/stats")))
        .await
        .expect("org stats");
    let json = body_to_json(resp).await;
    let budget = &json["budget"];
    assert!(
        budget["hourly_limit_usdc"].is_null(),
        "no cap -> null, not 0"
    );
    assert!(budget["daily_limit_usdc"].is_null());
    assert!(budget["monthly_limit_usdc"].is_null());
    assert_eq!(budget["teams_with_daily_budget"], 0);
    assert_eq!(budget["teams_total"], 1);
}

/// Item 3: org stats include a daily time series mirroring get_stats_by_day.
#[sqlx::test(migrations = "../../migrations")]
async fn org_stats_includes_by_day_time_series(pool: PgPool) {
    let app = router_with_pool(pool.clone());
    let org_id = create_test_org(&app, "acme").await;
    let team = mk_team(&app, org_id, "Eng").await;
    assign_team_wallet(&app, org_id, team, TEST_MEMBER_WALLET).await;

    // Two rows today, one row dated 3 days ago.
    seed_spend(&pool, TEST_MEMBER_WALLET, "openai/gpt-4o", "openai", 0.10).await;
    seed_spend(&pool, TEST_MEMBER_WALLET, "openai/gpt-4o", "openai", 0.20).await;
    sqlx::query(
        r#"INSERT INTO spend_logs
               (wallet_address, model, provider, input_tokens, output_tokens, cost_usdc, created_at)
           VALUES ($1, 'openai/gpt-4o', 'openai', 10, 20, 0.05, NOW() - INTERVAL '3 days')"#,
    )
    .bind(TEST_MEMBER_WALLET)
    .execute(&pool)
    .await
    .expect("seed dated spend");

    let resp = app
        .oneshot(bare_request(
            "GET",
            &format!("/v1/orgs/{org_id}/stats?days=7"),
        ))
        .await
        .expect("org stats");
    let json = body_to_json(resp).await;
    let by_day = json["by_day"].as_array().expect("by_day array");
    // Two distinct days.
    assert_eq!(by_day.len(), 2, "two distinct spend days");
    // Ascending by date: oldest first.
    let day0_spend = by_day[0]["spend_usdc"].as_f64().unwrap();
    let day1_spend = by_day[1]["spend_usdc"].as_f64().unwrap();
    assert!((day0_spend - 0.05).abs() < 1e-9, "older day = 0.05");
    assert!((day1_spend - 0.30).abs() < 1e-9, "today = 0.10 + 0.20");
    assert_eq!(by_day[1]["requests"], 2);
}

/// Item 4: top_wallets pagination via clamped limit/offset.
#[sqlx::test(migrations = "../../migrations")]
async fn org_stats_top_wallets_pagination(pool: PgPool) {
    let app = router_with_pool(pool.clone());
    let org_id = create_test_org(&app, "acme").await;
    let team = mk_team(&app, org_id, "Eng").await;

    // Three distinct wallets with descending spend.
    let wallets = [
        ("So11111111111111111111111111111111111111112", 0.30),
        ("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", 0.20),
        ("9n4nbM75f5Ui33ZbPYXn59EwSgE8CGsHtAeTH5YFeJ9A", 0.10),
    ];
    for (w, cost) in wallets {
        assign_team_wallet(&app, org_id, team, w).await;
        seed_spend(&pool, w, "openai/gpt-4o", "openai", cost).await;
    }

    // limit=1 -> only the top wallet (0.30).
    let resp = app
        .clone()
        .oneshot(bare_request(
            "GET",
            &format!("/v1/orgs/{org_id}/stats?limit=1"),
        ))
        .await
        .expect("limit=1");
    let json = body_to_json(resp).await;
    let top = json["top_wallets"].as_array().unwrap();
    assert_eq!(top.len(), 1);
    assert!((top[0]["total_cost_usdc"].as_f64().unwrap() - 0.30).abs() < 1e-9);

    // limit=1&offset=1 -> the second wallet (0.20).
    let resp = app
        .oneshot(bare_request(
            "GET",
            &format!("/v1/orgs/{org_id}/stats?limit=1&offset=1"),
        ))
        .await
        .expect("offset=1");
    let json = body_to_json(resp).await;
    let top = json["top_wallets"].as_array().unwrap();
    assert_eq!(top.len(), 1);
    assert!((top[0]["total_cost_usdc"].as_f64().unwrap() - 0.20).abs() < 1e-9);
}

/// Item 5: org stats include by_tenant and by_vendor breakdowns. Vendor amounts
/// are atomic integers formatted to USDC strings via the receipts helper.
#[sqlx::test(migrations = "../../migrations")]
async fn org_stats_includes_tenant_and_vendor_breakdowns(pool: PgPool) {
    let app = router_with_pool(pool.clone());
    let org_id = create_test_org(&app, "acme").await;
    let team = mk_team(&app, org_id, "Eng").await;
    assign_team_wallet(&app, org_id, team, TEST_MEMBER_WALLET).await;

    let vendor = "GDDMwNyyx8uB6zrqwBFHjLLG3TBYk2F8Az4yrQC5RzMp";
    // tenant "alpha": two rows; tenant "beta": one row; plus one untagged row.
    seed_spend_full(
        &pool,
        TEST_MEMBER_WALLET,
        "openai/gpt-4o",
        "openai",
        0.10,
        Some("alpha"),
        Some(vendor),
        Some(1_000_000),
        Some(50_000),
    )
    .await;
    seed_spend_full(
        &pool,
        TEST_MEMBER_WALLET,
        "openai/gpt-4o",
        "openai",
        0.20,
        Some("alpha"),
        Some(vendor),
        Some(2_000_000),
        Some(100_000),
    )
    .await;
    seed_spend_full(
        &pool,
        TEST_MEMBER_WALLET,
        "openai/gpt-4o",
        "openai",
        0.05,
        Some("beta"),
        None,
        None,
        None,
    )
    .await;
    seed_spend(&pool, TEST_MEMBER_WALLET, "openai/gpt-4o", "openai", 0.07).await;

    let resp = app
        .oneshot(bare_request("GET", &format!("/v1/orgs/{org_id}/stats")))
        .await
        .expect("org stats");
    assert_eq!(resp.status(), StatusCode::OK);
    let json = body_to_json(resp).await;

    // by_tenant: only tagged rows; alpha (0.30, 2 reqs) before beta (0.05, 1).
    let by_tenant = json["by_tenant"].as_array().expect("by_tenant array");
    assert_eq!(
        by_tenant.len(),
        2,
        "two distinct tenants (untagged excluded)"
    );
    assert_eq!(by_tenant[0]["tenant"], "alpha");
    assert_eq!(by_tenant[0]["request_count"], 2);
    assert!((by_tenant[0]["total_cost_usdc"].as_f64().unwrap() - 0.30).abs() < 1e-9);
    assert_eq!(by_tenant[1]["tenant"], "beta");

    // by_vendor: one vendor; settled 3.000000, fee 0.150000 (atomic-summed).
    let by_vendor = json["by_vendor"].as_array().expect("by_vendor array");
    assert_eq!(by_vendor.len(), 1, "one vendor wallet");
    assert_eq!(by_vendor[0]["vendor_wallet"], vendor);
    assert_eq!(by_vendor[0]["request_count"], 2);
    assert_eq!(by_vendor[0]["settled_usdc"], "3.000000");
    assert_eq!(by_vendor[0]["fee_receivable_usdc"], "0.150000");
}
