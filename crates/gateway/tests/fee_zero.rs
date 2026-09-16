//! End-to-end proof that a 0% runtime platform fee flows through EVERY
//! fee-emitting surface: the per-request 402, the discovery 402, `/v1/models`,
//! and `/pricing` — via the real router (`tower::ServiceExt::oneshot`), not
//! struct-level unit tests.
//!
//! The fee knob is process-global, so this file is its own test binary (own
//! process): it sets 0% and NEVER restores 5% — `integration.rs` keeps pinning
//! `fee_percent == 5` in its own process. The minimal `AppState` literal below
//! is deliberately DUPLICATED from `integration.rs::test_app_with_state`
//! rather than shared through a `tests/common` module (that would drag ~200
//! lines of unrelated helpers into this binary).

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tokio::sync::RwLock;
use tower::ServiceExt;

use gateway::config::AppConfig;
use gateway::middleware::rate_limit::{
    FreeTierGlobalCap, RateLimitConfig, RateLimiter, FREE_TIER_GLOBAL_RPM_DEFAULT,
};
use gateway::providers::health::{CircuitBreakerConfig, ProviderHealthTracker};
use gateway::providers::ProviderRegistry;
use gateway::services::ServiceRegistry;
use gateway::{build_router, AppState};
use solvela_protocol::{platform_fee_percent, set_platform_fee_percent};
use solvela_router::models::ModelRegistry;

/// One PRICED model: every quote below must be non-zero so `total ==
/// provider_cost` is a real assertion, not `"0.000000" == "0.000000"`.
const MODELS_TOML: &str = r#"
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

const RECIPIENT_WALLET: &str = "GatewayRecipientWallet111111111111111111111111";

fn generous_limiter() -> RateLimiter {
    RateLimiter::new(RateLimitConfig {
        max_requests: 10_000,
        window: Duration::from_secs(60),
        unknown_max_requests: 10_000,
    })
}

/// Build the router with the platform fee set to 0%.
///
/// Idempotent: every test calls it, all set the same value, none restores 5%.
fn zero_fee_app() -> axum::Router {
    set_platform_fee_percent(0).expect("0% is a legal platform fee");
    assert_eq!(platform_fee_percent(), 0, "knob must read back 0");

    let model_registry = ModelRegistry::from_toml(MODELS_TOML).expect("test models toml parses");
    let mut config = AppConfig::default();
    config.solana.recipient_wallet = RECIPIENT_WALLET.to_string();

    let state = Arc::new(AppState {
        config,
        model_registry,
        service_registry: RwLock::new(ServiceRegistry::empty()),
        providers: ProviderRegistry::from_env(reqwest::Client::new()),
        native_anthropic: None,
        search_provider: None,
        price_provider: None,
        // No verifier: every request below is UNPAID (no PAYMENT-SIGNATURE).
        facilitator: solvela_x402::facilitator::Facilitator::new(vec![]),
        usage: gateway::usage::UsageTracker::noop(),
        cache: None,
        semantic_cache: None,
        provider_health: ProviderHealthTracker::new(CircuitBreakerConfig::default()),
        escrow_claimer: None,
        fee_payer_pool: None,
        nonce_pool: None,
        db_pool: None,
        faucet: None,
        session_secret: b"test-secret".to_vec(),
        http_client: reqwest::Client::new(),
        replay_set: AppState::new_replay_set(),
        slot_cache: gateway::routes::escrow::new_slot_cache(),
        escrow_metrics: None,
        admin_token: None,
        api_key_hmac_secret: None,
        auth_provider: None,
        prometheus_handle: None,
        dev_bypass_payment: false,
        free_rate_limiter: RateLimiter::new(RateLimitConfig::free_default()),
        receipts_rate_limiter: generous_limiter(),
        a2a_tasks_rate_limiter: generous_limiter(),
        faucet_rate_limiter: generous_limiter(),
        deposit_tx_rate_limiter: generous_limiter(),
        free_global_cap: FreeTierGlobalCap::new(FREE_TIER_GLOBAL_RPM_DEFAULT),
    });
    build_router(state, RateLimiter::new(RateLimitConfig::default()))
        .expect("test router builds: default request timeout is valid")
}

async fn json_body(response: axum::response::Response) -> serde_json::Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).expect("response body is JSON")
}

/// "0.040960" → "40960" (the atomic-unit string the 402 `accepts[].amount` carries).
fn decimal_to_atomic_string(decimal: &str) -> String {
    let (whole, frac) = decimal.split_once('.').expect("6-dp decimal string");
    assert_eq!(
        frac.len(),
        6,
        "USDC decimal must have exactly 6 dp: {decimal}"
    );
    format!("{whole}{frac}")
        .parse::<u64>()
        .expect("atomic parses as u64")
        .to_string()
}

/// (a) Per-request 402: `fee_percent` 0, `platform_fee` zero, `total ==
/// provider_cost`, and the advertised `accepts[0].amount` is the total in
/// atomic units.
#[tokio::test]
async fn zero_fee_chat_402_quotes_provider_cost_only() {
    let app = zero_fee_app();
    let body = serde_json::json!({
        "model": "openai/gpt-4o",
        "messages": [{"role": "user", "content": "Hello"}],
    });
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);

    let v = json_body(response).await;
    let cb = &v["cost_breakdown"];
    assert_eq!(cb["fee_percent"], 0, "402 fee_percent must be 0: {cb}");
    assert_eq!(
        cb["platform_fee"], "0.000000",
        "402 platform_fee must be zero: {cb}"
    );
    let total = cb["total"].as_str().expect("total is a string");
    assert_eq!(
        total,
        cb["provider_cost"]
            .as_str()
            .expect("provider_cost is a string"),
        "at 0% the total must equal the provider cost: {cb}"
    );
    assert_ne!(
        total, "0.000000",
        "priced model must quote a non-zero total"
    );
    assert_eq!(
        v["accepts"][0]["amount"],
        decimal_to_atomic_string(total),
        "accepts[0].amount must be the (fee-free) total in atomic units: {v}"
    );
}

/// (b) Discovery 402 (`GET`): the floor is split by the live fee — at 0% the
/// provider share IS the total and the fee share is zero (no phantom ~4.76%).
#[tokio::test]
async fn zero_fee_discovery_402_has_no_phantom_fee_split() {
    let app = zero_fee_app();
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/chat/completions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);

    let v = json_body(response).await;
    let cb = &v["cost_breakdown"];
    assert_eq!(
        cb["fee_percent"], 0,
        "discovery fee_percent must be 0: {cb}"
    );
    assert_eq!(
        cb["platform_fee"], "0.000000",
        "discovery platform_fee must be zero: {cb}"
    );
    let total = cb["total"].as_str().expect("total is a string");
    assert_eq!(
        total,
        cb["provider_cost"]
            .as_str()
            .expect("provider_cost is a string"),
        "at 0% the discovery total must equal the provider cost: {cb}"
    );
    assert_ne!(
        total, "0.000000",
        "discovery floor must be non-zero for a priced model"
    );
}

/// (c) `GET /v1/models`: every `pricing.fee_percent` reflects the live knob.
#[tokio::test]
async fn zero_fee_models_list_reports_fee_percent_zero() {
    let app = zero_fee_app();
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/models")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let v = json_body(response).await;
    let data = v["data"].as_array().expect("data is an array");
    assert!(!data.is_empty(), "models list must not be empty");
    for m in data {
        assert_eq!(
            m["pricing"]["fee_percent"], 0,
            "model fee_percent must be 0: {m}"
        );
    }
}

/// (d) `GET /pricing`: platform block + every per-model block + the example
/// request all reflect 0%, and the human-readable description says "0%".
#[tokio::test]
async fn zero_fee_pricing_page_reports_zero_everywhere() {
    let app = zero_fee_app();
    let response = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/pricing")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let v = json_body(response).await;
    assert_eq!(v["platform"]["fee_percent"], 0, "{}", v["platform"]);
    let description = v["platform"]["fee_description"]
        .as_str()
        .expect("fee_description is a string");
    assert!(
        description.starts_with("0%"),
        "fee_description must lead with the live percent, got {description:?}"
    );

    let models = v["models"].as_array().expect("models is an array");
    assert!(!models.is_empty(), "pricing must list at least one model");
    for m in models {
        assert_eq!(m["pricing"]["platform_fee_percent"], 0, "{m}");
        let ex = &m["example_1k_token_request"];
        assert_eq!(ex["platform_fee_usdc"], "0.000000", "{ex}");
        assert_eq!(
            ex["total_usdc"], ex["provider_cost_usdc"],
            "at 0% the example total must equal the provider cost: {ex}"
        );
        assert_ne!(
            ex["total_usdc"], "0.000000",
            "priced example must be non-zero: {ex}"
        );
    }
}
