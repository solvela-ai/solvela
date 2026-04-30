//! Integration tests — misc_admin_metrics.
//!
//! Split from the original `tests/integration.rs`.
//! Shared helpers live in `tests/common/mod.rs`.

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

#[tokio::test]
async fn test_metrics_without_auth_returns_401() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Admin token is in AppState — no env var race possible.
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "expected 401 when no Bearer token is provided"
    );
}

#[tokio::test]
async fn test_metrics_with_valid_token_returns_200() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .header("Authorization", format!("Bearer {TEST_ADMIN_TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Admin token is in AppState — no env var race possible.
    assert_eq!(response.status(), StatusCode::OK);

    // Verify content type is Prometheus text format
    let content_type = response
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap();
    assert!(
        content_type.contains("text/plain"),
        "expected text/plain content type, got: {content_type}"
    );
}

#[tokio::test]
async fn test_metrics_with_invalid_token_returns_401() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .header("Authorization", "Bearer wrong-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Admin token is in AppState — no env var race possible.
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "expected 401 when no Bearer token is provided"
    );
}

#[tokio::test]
async fn test_metrics_without_admin_token_not_accessible() {
    // Admin token is in AppState so there are no env var races.
    // test_app() sets admin_token: Some(...), so unauthenticated = 401.
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "unauthenticated /metrics request should return 401"
    );
}

#[tokio::test]
async fn test_metrics_contains_request_total_after_request() {
    let (app, state) = test_app_with_state();

    // First, make a request to /health to generate metrics
    let health_response = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health_response.status(), StatusCode::OK);

    // Now fetch /metrics and check for solvela_requests_total
    let metrics_response = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .header("Authorization", format!("Bearer {TEST_ADMIN_TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(metrics_response.status(), StatusCode::OK);

    let body = metrics_response
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes();
    let body_str = String::from_utf8_lossy(&body);

    // The global recorder is shared across all tests so we may see metrics
    // from other tests too, but solvela_requests_total should be present.
    // Also verify via the handle directly.
    let rendered = state.prometheus_handle.as_ref().unwrap().render();
    assert!(
        rendered.contains("solvela_requests_total"),
        "metrics output should contain solvela_requests_total, got:\n{rendered}"
    );

    // Body from the endpoint should also contain it
    assert!(
        body_str.contains("solvela_requests_total"),
        "metrics body should contain solvela_requests_total"
    );
}

#[tokio::test]
async fn test_metrics_contains_request_duration() {
    let (app, state) = test_app_with_state();

    // Make a request to generate duration metrics
    let _ = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Check that the histogram metric exists
    let rendered = state.prometheus_handle.as_ref().unwrap().render();
    assert!(
        rendered.contains("solvela_request_duration_seconds"),
        "metrics should contain solvela_request_duration_seconds histogram, got:\n{rendered}"
    );
}

#[tokio::test]
async fn test_metrics_not_counted_in_own_requests() {
    let (app, state) = test_app_with_state();

    // Set token immediately before each request to minimize env var race
    // with other parallel tests.

    let resp1 = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .header("Authorization", format!("Bearer {TEST_ADMIN_TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Admin token is in AppState — no env var race.
    assert_eq!(resp1.status(), StatusCode::OK);

    let _resp2 = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .header("Authorization", format!("Bearer {TEST_ADMIN_TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Primary assertion: the /metrics path must not appear in solvela_requests_total.
    let rendered = state.prometheus_handle.as_ref().unwrap().render();
    let has_metrics_path = rendered
        .lines()
        .any(|line| line.contains("solvela_requests_total") && line.contains("path=\"/metrics\""));
    assert!(
        !has_metrics_path,
        "/metrics path should not be counted in solvela_requests_total"
    );
}

#[tokio::test]
async fn test_admin_stats_returns_503_without_db() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/admin/stats")
                .header("Authorization", format!("Bearer {}", TEST_ADMIN_TOKEN))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // No db_pool configured in test_app → 503
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"], "database not configured");
}

#[tokio::test]
async fn test_admin_stats_returns_401_with_wrong_token() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/admin/stats")
                .header("Authorization", "Bearer wrong-token")
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

#[tokio::test]
async fn test_admin_stats_returns_401_without_auth_header() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/admin/stats")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_admin_stats_returns_404_when_admin_token_not_configured() {
    // Build a custom app with admin_token = None
    let model_registry = ModelRegistry::from_toml(TEST_MODELS_TOML).unwrap();
    let service_registry = ServiceRegistry::from_toml(TEST_SERVICES_TOML).unwrap();
    let facilitator =
        solvela_x402::facilitator::Facilitator::new(vec![Arc::new(AlwaysPassVerifier)]);

    let mut config = AppConfig::default();
    config.solana.recipient_wallet = TEST_RECIPIENT_WALLET.to_string();

    let state = Arc::new(AppState {
        config,
        model_registry,
        service_registry: RwLock::new(service_registry),
        providers: ProviderRegistry::from_env(reqwest::Client::new()),
        facilitator,
        usage: gateway::usage::UsageTracker::noop(),
        cache: None,
        provider_health: ProviderHealthTracker::new(CircuitBreakerConfig::default()),
        escrow_claimer: None,
        fee_payer_pool: None,
        nonce_pool: None,
        db_pool: None,
        session_secret: b"test-secret".to_vec(),
        http_client: reqwest::Client::new(),
        replay_set: AppState::new_replay_set(),
        slot_cache: gateway::routes::escrow::new_slot_cache(),
        escrow_metrics: None,
        admin_token: None, // <-- no admin token configured
        prometheus_handle: Some(test_prometheus_handle()),
        dev_bypass_payment: false,
        dedup_store: gateway::cache::request_dedup::InMemoryDedupStore::new(),
    });
    let app = build_router(
        Arc::clone(&state),
        RateLimiter::new(RateLimitConfig::default()),
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/admin/stats")
                .header("Authorization", "Bearer some-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Endpoint is hidden when admin_token is not configured
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_admin_stats_returns_400_for_days_zero() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/admin/stats?days=0")
                .header("Authorization", format!("Bearer {}", TEST_ADMIN_TOKEN))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let error = json["error"].as_str().unwrap();
    assert!(error.contains("days must be between 1 and 365"));
    assert!(error.contains("0"));
}

#[tokio::test]
async fn test_admin_stats_returns_400_for_days_over_365() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/admin/stats?days=999")
                .header("Authorization", format!("Bearer {}", TEST_ADMIN_TOKEN))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let error = json["error"].as_str().unwrap();
    assert!(error.contains("days must be between 1 and 365"));
    assert!(error.contains("999"));
}
