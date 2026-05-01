//! Integration tests — misc_stats_session.
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

/// Helper: build a valid session token for tests.
fn test_session_token() -> String {
    let claims = gateway::session::SessionClaims {
        wallet: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
        budget_remaining: 5_000_000,
        issued_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        expires_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 3600,
        allowed_models: vec![],
    };
    gateway::session::create_session_token(&claims, b"test-secret").unwrap()
}

/// Helper: build an expired session token for tests.
fn test_expired_session_token() -> String {
    let claims = gateway::session::SessionClaims {
        wallet: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
        budget_remaining: 5_000_000,
        issued_at: 1_000_000,
        expires_at: 1_000_001, // expired long ago
        allowed_models: vec![],
    };
    gateway::session::create_session_token(&claims, b"test-secret").unwrap()
}

#[tokio::test]
async fn test_stats_missing_auth_returns_401() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/wallet/7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU/stats")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_stats_invalid_token_returns_401() {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/wallet/7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU/stats")
                .header("authorization", "Bearer invalid-token-garbage")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_stats_expired_token_returns_401() {
    let app = test_app();
    let token = test_expired_session_token();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/wallet/7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU/stats")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_stats_no_db_returns_503() {
    let app = test_app(); // test_app has db_pool: None
    let token = test_session_token();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/wallet/7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU/stats")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["error"].as_str().unwrap().contains("no database"));
}

#[tokio::test]
async fn test_stats_days_too_large_returns_400() {
    let app = test_app();
    let token = test_session_token();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/wallet/7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU/stats?days=500")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_stats_days_too_small_returns_400() {
    let app = test_app();
    let token = test_session_token();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/wallet/7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU/stats?days=0")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_stats_invalid_wallet_returns_400() {
    let app = test_app();
    let token = test_session_token();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/wallet/short/stats")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["error"]
        .as_str()
        .unwrap()
        .contains("invalid wallet address"));
}

#[tokio::test]
async fn test_stats_default_days_is_30() {
    // When no `days` param is provided, the default should be 30.
    // Since we have no DB, we'll get 503, but the route itself is matched.
    let app = test_app();
    let token = test_session_token();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/wallet/7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU/stats")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // Without DB we get 503 — this confirms the route is reachable and auth works
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn test_stats_explicit_days_7() {
    let app = test_app();
    let token = test_session_token();
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/wallet/7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU/stats?days=7")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // Without DB we get 503
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn test_stats_wallet_with_invalid_chars_returns_400() {
    let app = test_app();
    let token = test_session_token();
    // '0' and 'O' are not in base58 alphabet
    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/wallet/0xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAs/stats")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_stats_wallet_mismatch_returns_403() {
    let app = test_app();
    // Token is for wallet "7xKX..." but we request stats for a different wallet.
    let token = test_session_token();
    let other_wallet = "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM";
    let response = app
        .oneshot(
            Request::builder()
                .uri(format!("/v1/wallet/{other_wallet}/stats"))
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["error"].as_str().unwrap().contains("does not match"));
}

#[test]
fn test_heartbeat_module_accessible() {
    assert_eq!(
        gateway::providers::heartbeat::HEARTBEAT_SENTINEL,
        "__heartbeat__"
    );
}
