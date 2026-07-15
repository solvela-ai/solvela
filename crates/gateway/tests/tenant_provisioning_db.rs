//! Real-path DB tests for the tenant-budget provisioning endpoint
//! (`PUT /v1/wallet/{wallet}/tenants/{tenant}`).
//!
//! These exercise the production handler through `build_router` + `oneshot`
//! with a live Postgres pool (and, for the cache-bust test, a live Redis
//! client), so the upsert / 201-vs-200 / cache-invalidation behavior is
//! proven through the real route — not just at the struct level
//! (Architectural Rule #10, and `feedback_test_through_real_paths`).
//!
//! Deliberately a SEPARATE test binary from `integration.rs`: the
//! `#[sqlx::test]` macro loads the repo `.env` (for `DATABASE_URL`) into the
//! process environment, which carries provider API keys. In the
//! `integration.rs` binary that would leak into `ProviderRegistry::from_env()`
//! and flip the env-sensitive `/health` tests. Same isolation rationale as
//! `stats_http_redis.rs`.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use sqlx::PgPool;
use tokio::sync::RwLock;
use tower::ServiceExt;

use gateway::config::AppConfig;
use gateway::middleware::rate_limit::{
    FreeTierGlobalCap, RateLimitConfig, RateLimiter, FREE_TIER_GLOBAL_RPM_DEFAULT,
};
use gateway::providers::health::{CircuitBreakerConfig, ProviderHealthTracker};
use gateway::providers::ProviderRegistry;
use gateway::services::ServiceRegistry;
use gateway::usage::UsageTracker;
use gateway::{build_router, AppState};
use solvela_router::models::ModelRegistry;

const REDIS_URL: &str = "redis://127.0.0.1:6379";
const TEST_RECIPIENT_WALLET: &str = "GatewayRecipientWallet111111111111111111111111";
const TEST_ADMIN_TOKEN: &str = "test-admin-token-for-tenant-provisioning";

const TEST_SERVICES_TOML: &str = r#"
[services.llm-gateway]
name = "LLM Intelligence"
endpoint = "/v1/chat/completions"
category = "intelligence"
x402_enabled = true
internal = true
description = "OpenAI-compatible LLM inference"
pricing_label = "per-token (see /pricing)"
"#;

const TEST_MODELS_TOML: &str = r#"
[models.openai-gpt-4o]
provider = "openai"
model_id = "gpt-4o"
display_name = "GPT-4o"
input_cost_per_million = 2.50
output_cost_per_million = 10.00
context_window = 128000
supports_streaming = true
"#;

/// Build a router whose `AppState` carries the supplied `UsageTracker` + pool.
/// The provisioning route never touches the facilitator/providers, so an empty
/// facilitator and env-derived (empty in CI) provider registry are fine.
fn provisioning_router(usage: UsageTracker, db_pool: Option<PgPool>) -> axum::Router {
    let model_registry = ModelRegistry::from_toml(TEST_MODELS_TOML).unwrap();
    let service_registry = ServiceRegistry::from_toml(TEST_SERVICES_TOML).unwrap();
    let facilitator = solvela_x402::facilitator::Facilitator::new(vec![]);

    let mut config = AppConfig::default();
    config.solana.recipient_wallet = TEST_RECIPIENT_WALLET.to_string();

    let state = Arc::new(AppState {
        config,
        model_registry,
        service_registry: RwLock::new(service_registry),
        providers: ProviderRegistry::from_env(reqwest::Client::new()),
        native_anthropic: None,
        search_provider: None,
        price_provider: None,
        facilitator,
        usage,
        cache: None,
        semantic_cache: None,
        provider_health: ProviderHealthTracker::new(CircuitBreakerConfig::default()),
        escrow_claimer: None,
        fee_payer_pool: None,
        nonce_pool: None,
        db_pool,
        faucet: None,
        session_secret: b"test-secret".to_vec(),
        http_client: reqwest::Client::new(),
        replay_set: AppState::new_replay_set(),
        slot_cache: gateway::routes::escrow::new_slot_cache(),
        escrow_metrics: None,
        admin_token: Some(gateway::secret::AdminToken::new(
            TEST_ADMIN_TOKEN.to_string(),
        )),
        api_key_hmac_secret: None,
        auth_provider: None,
        prometheus_handle: None,
        dev_bypass_payment: false,
        free_rate_limiter: RateLimiter::new(RateLimitConfig::free_default()),
        receipts_rate_limiter: RateLimiter::new(RateLimitConfig::receipts_default()),
        a2a_tasks_rate_limiter: RateLimiter::new(RateLimitConfig::a2a_tasks_default()),
        faucet_rate_limiter: RateLimiter::new(RateLimitConfig::faucet_default()),
        deposit_tx_rate_limiter: RateLimiter::new(RateLimitConfig::deposit_tx_default()),
        free_global_cap: FreeTierGlobalCap::new(FREE_TIER_GLOBAL_RPM_DEFAULT),
    });
    build_router(
        Arc::clone(&state),
        RateLimiter::new(RateLimitConfig::default()),
    )
    .expect("test router builds: default request timeout is valid")
}

/// A random VALID base58 32-byte pubkey per test, so parallel tests never
/// collide on a `(wallet, tenant)` row or a `tenant_budget:` cache key. The
/// provisioning endpoint validates the wallet as a real pubkey, so (unlike
/// `stats_http_redis.rs`) this must round-trip through base58 decode.
fn random_pubkey_wallet() -> String {
    let a = uuid::Uuid::new_v4();
    let b = uuid::Uuid::new_v4();
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(a.as_bytes());
    bytes[16..].copy_from_slice(b.as_bytes());
    bs58::encode(bytes).into_string()
}

/// PUT the provisioning endpoint with the admin token and return
/// `(status, parsed JSON body)`.
async fn put_tenant_budget(
    app: axum::Router,
    wallet: &str,
    tenant: &str,
    body: &str,
) -> (StatusCode, serde_json::Value) {
    let response = app
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(format!("/v1/wallet/{wallet}/tenants/{tenant}"))
                .header("content-type", "application/json")
                .header("Authorization", format!("Bearer {TEST_ADMIN_TOKEN}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&bytes).unwrap();
    (status, json)
}

/// Read the provisioned row back as TEXT so scale-6 rendering is asserted
/// exactly (DECIMAL(18,6) prints all six decimal places).
async fn fetch_row(
    pool: &PgPool,
    wallet: &str,
    tenant: &str,
) -> Option<(Option<String>, Option<String>, Option<String>)> {
    sqlx::query_as(
        r#"SELECT hourly_limit_usdc::TEXT, daily_limit_usdc::TEXT, monthly_limit_usdc::TEXT
           FROM tenant_budgets
           WHERE wallet_address = $1 AND tenant = $2"#,
    )
    .bind(wallet)
    .bind(tenant)
    .fetch_optional(pool)
    .await
    .expect("query tenant_budgets row")
}

/// Fresh `(wallet, tenant)` → 201 + `created: true`, row durably in Postgres;
/// re-PUT of the same key → 200 + `created: false` with the caps REPLACED
/// (idempotent upsert — "already exists" is success, never an error, because
/// the external provisioner retries under Stripe webhook redelivery).
#[sqlx::test(migrations = "../../migrations")]
async fn provision_creates_row_201_then_updates_200(pool: PgPool) {
    let wallet = random_pubkey_wallet();

    // First PUT: create. Short decimal "5.5" must normalize to 6dp.
    let app = provisioning_router(
        UsageTracker::new(Some(pool.clone()), None),
        Some(pool.clone()),
    );
    let (status, json) = put_tenant_budget(
        app,
        &wallet,
        "acme",
        r#"{"hourly_limit_usdc":null,"daily_limit_usdc":"5.5","monthly_limit_usdc":"90.000000"}"#,
    )
    .await;

    assert_eq!(status, StatusCode::CREATED, "fresh row must 201: {json}");
    assert_eq!(json["created"], true);
    assert_eq!(json["wallet_address"], wallet.as_str());
    assert_eq!(json["tenant"], "acme");
    assert!(json["hourly_limit_usdc"].is_null());
    assert_eq!(json["daily_limit_usdc"], "5.500000");
    assert_eq!(json["monthly_limit_usdc"], "90.000000");

    let row = fetch_row(&pool, &wallet, "acme").await.expect("row exists");
    assert_eq!(
        row,
        (None, Some("5.500000".into()), Some("90.000000".into()))
    );

    // Second PUT (same key, new caps): update, not error.
    let app = provisioning_router(
        UsageTracker::new(Some(pool.clone()), None),
        Some(pool.clone()),
    );
    let (status, json) = put_tenant_budget(
        app,
        &wallet,
        "acme",
        r#"{"hourly_limit_usdc":"0.250000","daily_limit_usdc":"7.000000","monthly_limit_usdc":null}"#,
    )
    .await;

    assert_eq!(status, StatusCode::OK, "existing row must 200: {json}");
    assert_eq!(json["created"], false);
    assert_eq!(json["hourly_limit_usdc"], "0.250000");
    assert_eq!(json["daily_limit_usdc"], "7.000000");
    assert!(json["monthly_limit_usdc"].is_null());

    let row = fetch_row(&pool, &wallet, "acme").await.expect("row exists");
    assert_eq!(
        row,
        (Some("0.250000".into()), Some("7.000000".into()), None)
    );
}

/// All three windows null is a valid provision: the row exists (so
/// `require_tenant` wallets accept the tag) with NO caps.
#[sqlx::test(migrations = "../../migrations")]
async fn provision_all_null_caps_creates_uncapped_row(pool: PgPool) {
    let wallet = random_pubkey_wallet();
    let app = provisioning_router(
        UsageTracker::new(Some(pool.clone()), None),
        Some(pool.clone()),
    );

    let (status, json) = put_tenant_budget(
        app,
        &wallet,
        "uncapped",
        r#"{"hourly_limit_usdc":null,"daily_limit_usdc":null,"monthly_limit_usdc":null}"#,
    )
    .await;

    assert_eq!(status, StatusCode::CREATED, "{json}");
    assert_eq!(json["created"], true);

    let row = fetch_row(&pool, &wallet, "uncapped")
        .await
        .expect("row exists");
    assert_eq!(row, (None, None, None), "all three caps must be NULL");
}

/// A successful provision must DELETE the `tenant_budget:{wallet}:{tenant}`
/// Redis key — otherwise the 60s budget-config cache (including its negative
/// "none" sentinel) can keep rejecting a freshly provisioned tenant under
/// `require_tenant`.
#[sqlx::test(migrations = "../../migrations")]
async fn provision_busts_tenant_budget_cache_key(pool: PgPool) {
    let client = redis::Client::open(REDIS_URL).expect("redis client");
    let wallet = random_pubkey_wallet();
    let cache_key = format!("tenant_budget:{wallet}:acme");

    // Seed the negative "none" sentinel the enforcement path would have cached.
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("redis conn");
    let _: () = redis::cmd("SET")
        .arg(&cache_key)
        .arg("none")
        .arg("EX")
        .arg(60)
        .query_async(&mut conn)
        .await
        .expect("seed cache sentinel");

    let usage = UsageTracker::new(Some(pool.clone()), Some(client.clone()));
    let app = provisioning_router(usage, Some(pool.clone()));
    let (status, json) = put_tenant_budget(app, &wallet, "acme",
        r#"{"hourly_limit_usdc":null,"daily_limit_usdc":"5.000000","monthly_limit_usdc":"90.000000"}"#,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{json}");

    let cached: Option<String> = redis::cmd("GET")
        .arg(&cache_key)
        .query_async(&mut conn)
        .await
        .expect("read cache key");
    assert_eq!(
        cached, None,
        "provision must delete the stale tenant_budget cache entry"
    );
}
