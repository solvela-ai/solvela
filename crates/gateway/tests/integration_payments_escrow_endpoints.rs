//! Integration tests — payments_escrow_endpoints.
//!
//! Split from the original `tests/integration.rs`.
//! Shared helpers live in `tests/common/mod.rs`.

#![allow(unused_imports)]

#[path = "common/mod.rs"]
mod common;

#[allow(unused_imports)]
use std::collections::HashMap;
#[allow(unused_imports)]
use std::pin::Pin;
use std::sync::Arc;

#[allow(unused_imports)]
use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
#[allow(unused_imports)]
use futures::stream;
use http_body_util::BodyExt;
#[allow(unused_imports)]
use tokio::sync::RwLock;
use tower::ServiceExt;

#[allow(unused_imports)]
use gateway::config::AppConfig;
use gateway::middleware::rate_limit::{RateLimitConfig, RateLimiter};
#[allow(unused_imports)]
use gateway::providers::health::{CircuitBreakerConfig, ProviderHealthTracker};
#[allow(unused_imports)]
use gateway::providers::{ChatStream, LLMProvider, ProviderRegistry};
#[allow(unused_imports)]
use gateway::services::ServiceRegistry;
#[allow(unused_imports)]
use gateway::{build_router, AppState};
#[allow(unused_imports)]
use solvela_protocol::{
    ChatChoice, ChatChunk, ChatChunkChoice, ChatDelta, ChatMessage, ChatResponse, ModelInfo, Role,
    Usage,
};
#[allow(unused_imports)]
use solvela_router::models::ModelRegistry;
#[allow(unused_imports)]
use solvela_x402::traits::{Error as X402Error, PaymentVerifier};
#[allow(unused_imports)]
use solvela_x402::types::{
    EscrowPayload, PayloadData, PaymentAccept, PaymentPayload, Resource, SettlementResult,
    SolanaPayload, VerificationResult, SOLANA_NETWORK, USDC_MINT,
};

#[allow(unused_imports)]
use common::*;

/// Helper that builds a test app with escrow configured AND metrics enabled.
fn test_app_with_escrow_metrics() -> axum::Router {
    let model_registry = ModelRegistry::from_toml(TEST_MODELS_TOML).unwrap();
    let service_registry = ServiceRegistry::from_toml(TEST_SERVICES_TOML).unwrap();

    let facilitator = solvela_x402::facilitator::Facilitator::new(vec![
        Arc::new(AlwaysPassVerifier),
        Arc::new(AlwaysPassEscrowVerifier),
    ]);

    let mut config = AppConfig::default();
    config.solana.recipient_wallet = TEST_RECIPIENT_WALLET.to_string();
    config.solana.escrow_program_id =
        Some("9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU".to_string());

    let test_keypair = {
        use ed25519_dalek::SigningKey;
        let sk = SigningKey::from_bytes(&[1u8; 32]);
        let mut kp = [0u8; 64];
        kp[..32].copy_from_slice(&[1u8; 32]);
        kp[32..].copy_from_slice(sk.verifying_key().as_bytes());
        bs58::encode(&kp).into_string()
    };
    let test_fee_payer_pool = Arc::new(
        solvela_x402::fee_payer::FeePayerPool::from_keys(&[test_keypair])
            .expect("test pool must load"),
    );

    let escrow_claimer = solvela_x402::escrow::EscrowClaimer::new(
        "https://api.devnet.solana.com".to_string(),
        test_fee_payer_pool.clone(),
        "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU",
        "11111111111111111111111111111111",
        "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
        None,
    )
    .expect("test claimer must be valid");

    // Pre-populate metrics with some values
    let metrics = Arc::new(solvela_x402::escrow::EscrowMetrics::new());
    metrics
        .claims_submitted
        .store(42, std::sync::atomic::Ordering::Relaxed);
    metrics
        .claims_succeeded
        .store(38, std::sync::atomic::Ordering::Relaxed);
    metrics
        .claims_failed
        .store(3, std::sync::atomic::Ordering::Relaxed);
    metrics
        .claims_retried
        .store(1, std::sync::atomic::Ordering::Relaxed);

    let state = Arc::new(AppState {
        config,
        model_registry,
        service_registry: RwLock::new(service_registry),
        providers: ProviderRegistry::from_env(reqwest::Client::new()),
        facilitator,
        usage: gateway::usage::UsageTracker::noop(),
        cache: None,
        provider_health: ProviderHealthTracker::new(CircuitBreakerConfig::default()),
        escrow_claimer: Some(Arc::new(escrow_claimer)),
        fee_payer_pool: Some(test_fee_payer_pool),
        nonce_pool: None,
        db_pool: None,
        session_secret: b"test-secret".to_vec(),
        http_client: reqwest::Client::new(),
        replay_set: AppState::new_replay_set(),
        slot_cache: gateway::routes::escrow::new_slot_cache(),
        escrow_metrics: Some(metrics),
        admin_token: Some(TEST_ADMIN_TOKEN.to_string()),
        prometheus_handle: Some(test_prometheus_handle()),
        dev_bypass_payment: false,
        dedup_store: gateway::cache::request_dedup::InMemoryDedupStore::new(),
    });
    build_router(state, RateLimiter::new(RateLimitConfig::default()))
}

/// Test 11: escrow config returns 404 when escrow_program_id is not set.
#[tokio::test]
async fn test_escrow_config_returns_404_when_not_configured() {
    let app = test_app(); // default config has escrow_program_id: None

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/escrow/config")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "escrow not configured");
}

/// Test 12: escrow config returns 200 with escrow params when configured.
/// Since we cannot make a real Solana RPC call in tests, current_slot may be null.
#[tokio::test]
async fn test_escrow_config_returns_200_when_configured() {
    let app = test_app_with_escrow();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/escrow/config")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(
        json["escrow_program_id"],
        "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU"
    );
    assert_eq!(json["network"], SOLANA_NETWORK);
    assert_eq!(json["usdc_mint"], USDC_MINT);
    assert_eq!(json["provider_wallet"], TEST_RECIPIENT_WALLET);
    // current_slot may be null if devnet RPC is unreachable in CI
    assert!(
        json["current_slot"].is_u64() || json["current_slot"].is_null(),
        "current_slot must be a u64 or null, got: {}",
        json["current_slot"]
    );
}

/// Test 13a: escrow health returns 401 when no Authorization header is sent.
#[tokio::test]
async fn test_escrow_health_returns_401_without_auth_header() {
    let app = test_app_with_escrow();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/escrow/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "unauthorized");
}

/// Test 13b: escrow health returns 401 when bearer token is wrong.
#[tokio::test]
async fn test_escrow_health_returns_401_with_wrong_token() {
    let app = test_app_with_escrow();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/escrow/health")
                .header("authorization", "Bearer wrong-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "unauthorized");
}

/// Test 13c: escrow health returns 404 when escrow is not configured (with valid auth).
#[tokio::test]
async fn test_escrow_health_returns_404_when_not_configured() {
    let app = test_app(); // default config has escrow_program_id: None

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/escrow/health")
                .header("authorization", format!("Bearer {TEST_ADMIN_TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "escrow not configured");
}

/// Test 14: escrow health returns 200 with correct shape when escrow is configured.
#[tokio::test]
async fn test_escrow_health_returns_200_when_configured() {
    let app = test_app_with_escrow();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/escrow/health")
                .header("authorization", format!("Bearer {TEST_ADMIN_TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Verify response shape
    assert!(
        json["status"].is_string(),
        "status must be a string, got: {}",
        json["status"]
    );
    assert!(json["escrow_enabled"].is_boolean());
    assert!(json["fee_payer_wallets"].is_number());
    assert!(json["claims"].is_object());
    assert!(json["claims"]["submitted"].is_number());
    assert!(json["claims"]["succeeded"].is_number());
    assert!(json["claims"]["failed"].is_number());
    assert!(json["claims"]["retried"].is_number());

    // Without metrics or DB, claims should be zero and pending null
    assert_eq!(json["claims"]["submitted"], 0);
    assert_eq!(json["claims"]["succeeded"], 0);
    assert_eq!(json["claims"]["failed"], 0);
    assert_eq!(json["claims"]["retried"], 0);
    assert!(json["claims"]["pending_in_queue"].is_null());
}

/// Test 15: escrow health returns populated metrics when metrics are configured.
#[tokio::test]
async fn test_escrow_health_returns_metrics() {
    let app = test_app_with_escrow_metrics();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/escrow/health")
                .header("authorization", format!("Bearer {TEST_ADMIN_TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Metrics should reflect pre-populated values
    assert_eq!(json["claims"]["submitted"], 42);
    assert_eq!(json["claims"]["succeeded"], 38);
    assert_eq!(json["claims"]["failed"], 3);
    assert_eq!(json["claims"]["retried"], 1);

    // With escrow_claimer + fee_payer_pool but no db_pool,
    // status should be "degraded" (claim_processor_running is false without DB)
    assert_eq!(json["escrow_enabled"], true);
    assert_eq!(json["fee_payer_wallets"], 1);
    assert!(json["claims"]["pending_in_queue"].is_null());
}

/// Test that the escrow config endpoint returns the correct program ID
/// when escrow is configured, along with all required fields.
#[tokio::test]
async fn test_escrow_config_returns_correct_program_id() {
    let app = test_app_with_escrow();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/escrow/config")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Program ID must match exactly what was configured
    assert_eq!(
        json["escrow_program_id"], "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU",
        "escrow_program_id must match configured value"
    );

    // All required fields must be present and have correct types
    assert!(json["network"].is_string(), "network must be a string");
    assert!(json["usdc_mint"].is_string(), "usdc_mint must be a string");
    assert!(
        json["provider_wallet"].is_string(),
        "provider_wallet must be a string"
    );

    // Network must be the Solana network identifier
    assert!(
        json["network"].as_str().unwrap().starts_with("solana:"),
        "network must start with 'solana:'"
    );
}

/// Test that escrow health endpoint reflects atomically incremented metrics.
/// This verifies that the metrics flow from atomic counters -> snapshot -> JSON
/// works correctly with various increment patterns.
#[tokio::test]
async fn test_escrow_health_reflects_incremented_metrics() {
    use std::sync::atomic::Ordering;

    let model_registry = ModelRegistry::from_toml(TEST_MODELS_TOML).unwrap();
    let service_registry = ServiceRegistry::from_toml(TEST_SERVICES_TOML).unwrap();

    let facilitator = solvela_x402::facilitator::Facilitator::new(vec![
        Arc::new(AlwaysPassVerifier),
        Arc::new(AlwaysPassEscrowVerifier),
    ]);

    let mut config = AppConfig::default();
    config.solana.recipient_wallet = TEST_RECIPIENT_WALLET.to_string();
    config.solana.escrow_program_id =
        Some("9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU".to_string());

    let test_keypair = {
        use ed25519_dalek::SigningKey;
        let sk = SigningKey::from_bytes(&[1u8; 32]);
        let mut kp = [0u8; 64];
        kp[..32].copy_from_slice(&[1u8; 32]);
        kp[32..].copy_from_slice(sk.verifying_key().as_bytes());
        bs58::encode(&kp).into_string()
    };
    let test_fee_payer_pool = Arc::new(
        solvela_x402::fee_payer::FeePayerPool::from_keys(&[test_keypair])
            .expect("test pool must load"),
    );

    let escrow_claimer = solvela_x402::escrow::EscrowClaimer::new(
        "https://api.devnet.solana.com".to_string(),
        test_fee_payer_pool.clone(),
        "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU",
        "11111111111111111111111111111111",
        "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
        None,
    )
    .expect("test claimer must be valid");

    // Start with zero metrics
    let metrics = Arc::new(solvela_x402::escrow::EscrowMetrics::new());

    let state = Arc::new(AppState {
        config,
        model_registry,
        service_registry: RwLock::new(service_registry),
        providers: ProviderRegistry::from_env(reqwest::Client::new()),
        facilitator,
        usage: gateway::usage::UsageTracker::noop(),
        cache: None,
        provider_health: ProviderHealthTracker::new(CircuitBreakerConfig::default()),
        escrow_claimer: Some(Arc::new(escrow_claimer)),
        fee_payer_pool: Some(test_fee_payer_pool),
        nonce_pool: None,
        db_pool: None,
        session_secret: b"test-secret".to_vec(),
        http_client: reqwest::Client::new(),
        replay_set: AppState::new_replay_set(),
        slot_cache: gateway::routes::escrow::new_slot_cache(),
        escrow_metrics: Some(Arc::clone(&metrics)),
        admin_token: Some(TEST_ADMIN_TOKEN.to_string()),
        prometheus_handle: Some(test_prometheus_handle()),
        dev_bypass_payment: false,
        dedup_store: gateway::cache::request_dedup::InMemoryDedupStore::new(),
    });

    // Simulate claim processing by incrementing metrics atomically
    metrics.claims_submitted.fetch_add(5, Ordering::Relaxed);
    metrics.claims_succeeded.fetch_add(3, Ordering::Relaxed);
    metrics.claims_failed.fetch_add(1, Ordering::Relaxed);
    metrics.claims_retried.fetch_add(1, Ordering::Relaxed);

    let app = build_router(
        Arc::clone(&state),
        RateLimiter::new(RateLimitConfig::default()),
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/escrow/health")
                .header("authorization", format!("Bearer {TEST_ADMIN_TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["claims"]["submitted"], 5);
    assert_eq!(json["claims"]["succeeded"], 3);
    assert_eq!(json["claims"]["failed"], 1);
    assert_eq!(json["claims"]["retried"], 1);

    // Verify status reflects operational state
    assert_eq!(json["escrow_enabled"], true);
    assert_eq!(json["fee_payer_wallets"], 1);
}

/// Test that escrow health reports "down" when escrow is configured but no
/// claimer is present (e.g., fee payer key missing).
#[tokio::test]
async fn test_escrow_health_status_down_without_claimer() {
    let model_registry = ModelRegistry::from_toml(TEST_MODELS_TOML).unwrap();
    let service_registry = ServiceRegistry::from_toml(TEST_SERVICES_TOML).unwrap();
    let facilitator =
        solvela_x402::facilitator::Facilitator::new(vec![Arc::new(AlwaysPassVerifier)]);

    let mut config = AppConfig::default();
    config.solana.recipient_wallet = TEST_RECIPIENT_WALLET.to_string();
    config.solana.escrow_program_id =
        Some("9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU".to_string());

    let state = Arc::new(AppState {
        config,
        model_registry,
        service_registry: RwLock::new(service_registry),
        providers: ProviderRegistry::from_env(reqwest::Client::new()),
        facilitator,
        usage: gateway::usage::UsageTracker::noop(),
        cache: None,
        provider_health: ProviderHealthTracker::new(CircuitBreakerConfig::default()),
        escrow_claimer: None, // No claimer configured
        fee_payer_pool: None,
        nonce_pool: None,
        db_pool: None,
        session_secret: b"test-secret".to_vec(),
        http_client: reqwest::Client::new(),
        replay_set: AppState::new_replay_set(),
        slot_cache: gateway::routes::escrow::new_slot_cache(),
        escrow_metrics: None,
        admin_token: Some(TEST_ADMIN_TOKEN.to_string()),
        prometheus_handle: Some(test_prometheus_handle()),
        dev_bypass_payment: false,
        dedup_store: gateway::cache::request_dedup::InMemoryDedupStore::new(),
    });

    let app = build_router(state, RateLimiter::new(RateLimitConfig::default()));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/escrow/health")
                .header("authorization", format!("Bearer {TEST_ADMIN_TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(
        json["status"], "down",
        "status should be 'down' when escrow_claimer is None"
    );
    assert_eq!(json["escrow_enabled"], false);
    assert_eq!(json["fee_payer_wallets"], 0);
}

/// Test that escrow health reports "degraded" when claimer is present but
/// no DB pool is available (claim processor cannot run).
#[tokio::test]
async fn test_escrow_health_status_degraded_without_db() {
    // test_app_with_escrow has escrow_claimer but no db_pool
    let app = test_app_with_escrow();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/escrow/health")
                .header("authorization", format!("Bearer {TEST_ADMIN_TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Escrow is enabled but claim processor can't run without DB
    assert_eq!(json["escrow_enabled"], true);
    // test_app_with_escrow now sets fee_payer_pool, so wallets > 0,
    // but no db_pool => claim_processor_running is false => "degraded"
    assert_eq!(
        json["status"], "degraded",
        "status should be 'degraded' without DB but with fee payer pool"
    );
}
