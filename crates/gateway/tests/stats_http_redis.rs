//! Real-path HTTP tests for the daily/monthly Redis-backed fields on
//! `GET /v1/wallet/:address/stats`.
//!
//! These exercise the production handler through `build_router` + `oneshot`
//! with a live Postgres pool AND a live Redis client, so
//! `summary.daily_cost_usdc` / `summary.monthly_cost_usdc` are populated by the
//! real route — not just asserted at the struct level (Architectural Rule #10,
//! and `feedback_test_through_real_paths`).
//!
//! Deliberately a SEPARATE test binary from `integration.rs`: the `#[sqlx::test]`
//! macro loads the repo `.env` (for `DATABASE_URL`) into the process
//! environment, which carries provider API keys. In the `integration.rs` binary
//! that would leak into `ProviderRegistry::from_env()` and flip the
//! env-sensitive `/health` "no providers => error" tests. Isolating these tests
//! in their own process keeps that env mutation contained.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use sqlx::PgPool;
use tokio::sync::RwLock;
use tower::ServiceExt;

use gateway::config::AppConfig;
use gateway::middleware::rate_limit::{RateLimitConfig, RateLimiter};
use gateway::providers::health::{CircuitBreakerConfig, ProviderHealthTracker};
use gateway::providers::ProviderRegistry;
use gateway::services::ServiceRegistry;
use gateway::usage::UsageTracker;
use gateway::{build_router, AppState};
use solvela_router::models::ModelRegistry;

const REDIS_URL: &str = "redis://127.0.0.1:6379";
const TEST_RECIPIENT_WALLET: &str = "GatewayRecipientWallet111111111111111111111111";
const TEST_ADMIN_TOKEN: &str = "test-admin-token-for-stats-http-redis";
const SESSION_SECRET: &[u8] = b"test-secret";

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
/// The stats route never touches the facilitator/providers, so an empty
/// facilitator and env-derived (empty in CI) provider registry are fine.
fn stats_router(usage: UsageTracker, db_pool: Option<PgPool>) -> axum::Router {
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
        facilitator,
        usage,
        cache: None,
        semantic_cache: None,
        provider_health: ProviderHealthTracker::new(CircuitBreakerConfig::default()),
        escrow_claimer: None,
        fee_payer_pool: None,
        nonce_pool: None,
        db_pool,
        session_secret: SESSION_SECRET.to_vec(),
        http_client: reqwest::Client::new(),
        replay_set: AppState::new_replay_set(),
        slot_cache: gateway::routes::escrow::new_slot_cache(),
        escrow_metrics: None,
        admin_token: Some(gateway::secret::AdminToken::new(
            TEST_ADMIN_TOKEN.to_string(),
        )),
        api_key_hmac_secret: None,
        prometheus_handle: None,
        dev_bypass_payment: false,
    });
    build_router(
        Arc::clone(&state),
        RateLimiter::new(RateLimitConfig::default()),
    )
}

/// A randomly-generated, valid base58 Solana-style wallet (44 chars) so each
/// test isolates its own `spend:{wallet}:{period}` Redis counters even under
/// parallel execution. Must satisfy the handler's `is_valid_wallet_address`.
fn random_valid_wallet() -> String {
    const BASE58: &[u8] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let raw = uuid::Uuid::new_v4().simple().to_string();
    let raw2 = uuid::Uuid::new_v4().simple().to_string();
    format!("{raw}{raw2}")
        .bytes()
        .take(44)
        .map(|b| BASE58[(b as usize) % BASE58.len()] as char)
        .collect()
}

/// Build a valid (non-expired) session token bound to `wallet`, signed with the
/// same secret the router uses.
fn session_token_for(wallet: &str) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let claims = gateway::session::SessionClaims {
        wallet: wallet.to_string(),
        budget_remaining: 5_000_000,
        issued_at: now,
        expires_at: now + 3600,
        allowed_models: vec![],
    };
    gateway::session::create_session_token(&claims, SESSION_SECRET).unwrap()
}

async fn redis_set_spend(client: &redis::Client, key: &str, val: &str) {
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("redis conn");
    let _: () = redis::cmd("SET")
        .arg(key)
        .arg(val)
        .arg("EX")
        .arg(3600)
        .query_async(&mut conn)
        .await
        .expect("seed spend");
}

async fn redis_del_keys(client: &redis::Client, keys: &[&str]) {
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("redis conn");
    for key in keys {
        let _: Result<i64, _> = redis::cmd("DEL").arg(*key).query_async(&mut conn).await;
    }
}

async fn get_stats_json(app: axum::Router, wallet: &str) -> (StatusCode, serde_json::Value) {
    let token = session_token_for(wallet);
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/v1/wallet/{wallet}/stats"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json = serde_json::from_slice(&body).unwrap();
    (status, json)
}

/// With both Postgres and Redis present, the stats response must surface the
/// seeded daily/monthly counters formatted to 6 decimals.
#[sqlx::test(migrations = "../../migrations")]
async fn stats_endpoint_surfaces_daily_monthly_from_redis(pool: PgPool) {
    let client = redis::Client::open(REDIS_URL).expect("redis client");
    let wallet = random_valid_wallet();
    let now = chrono::Utc::now();
    let day_key = format!("spend:{}:{}", wallet, now.format("%Y-%m-%d"));
    let month_key = format!("spend:{}:{}", wallet, now.format("%Y-%m"));

    redis_set_spend(&client, &day_key, "0.250000").await;
    redis_set_spend(&client, &month_key, "1.750000").await;

    let usage = UsageTracker::new(Some(pool.clone()), Some(client.clone()));
    let app = stats_router(usage, Some(pool.clone()));

    let (status, json) = get_stats_json(app, &wallet).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["summary"]["daily_cost_usdc"], "0.250000");
    assert_eq!(json["summary"]["monthly_cost_usdc"], "1.750000");
    // DB-backed summary is intact (no spend rows => zeros).
    assert_eq!(json["summary"]["total_requests"], 0);
    assert_eq!(json["summary"]["total_cost_usdc"], "0.000000");

    redis_del_keys(&client, &[&day_key, &month_key]).await;
}

/// With Redis present but no counters seeded for the wallet, daily/monthly must
/// be 0.0 (formatted "0.000000"), never an error — and the endpoint still 200s.
#[sqlx::test(migrations = "../../migrations")]
async fn stats_endpoint_daily_monthly_zero_when_counters_missing(pool: PgPool) {
    let client = redis::Client::open(REDIS_URL).expect("redis client");
    let wallet = random_valid_wallet(); // fresh => no spend keys

    let usage = UsageTracker::new(Some(pool.clone()), Some(client.clone()));
    let app = stats_router(usage, Some(pool.clone()));

    let (status, json) = get_stats_json(app, &wallet).await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(json["summary"]["daily_cost_usdc"], "0.000000");
    assert_eq!(json["summary"]["monthly_cost_usdc"], "0.000000");
}

/// Redis-OPTIONAL (Architectural Rule #12): with a DB but NO Redis client, the
/// stats endpoint must still return 200 with the DB-backed summary intact and
/// daily/monthly == "0.000000" — it must NOT start hard-requiring Redis.
#[sqlx::test(migrations = "../../migrations")]
async fn stats_endpoint_returns_200_with_redis_absent(pool: PgPool) {
    // DB present, Redis absent.
    let usage = UsageTracker::new(Some(pool.clone()), None);
    let wallet = random_valid_wallet();
    let app = stats_router(usage, Some(pool.clone()));

    let (status, json) = get_stats_json(app, &wallet).await;

    assert_eq!(
        status,
        StatusCode::OK,
        "endpoint must NOT hard-require Redis"
    );
    assert_eq!(json["summary"]["daily_cost_usdc"], "0.000000");
    assert_eq!(json["summary"]["monthly_cost_usdc"], "0.000000");
    assert_eq!(json["summary"]["total_requests"], 0);
}
