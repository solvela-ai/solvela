//! Integration tests — misc_health_endpoints.
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
async fn test_health_endpoint() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    // Test app has no provider API keys → "error" status (zero providers configured).
    // HTTP status is always 200 (Fly.io health checks need 2xx).
    assert_eq!(json["status"], "error");
    // Unauthenticated requests do not include version or checks (security hardening)
    assert!(
        json.get("version").is_none() || json["version"].is_null(),
        "unauthenticated health must not include version"
    );
    assert!(
        json.get("checks").is_none() || json["checks"].is_null(),
        "unauthenticated health must not include checks"
    );
}

/// 14.3: GET /health returns a `version` field when authenticated with admin token.
#[tokio::test]
async fn test_health_returns_version() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .header("Authorization", format!("Bearer {TEST_ADMIN_TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // version must be a non-empty string
    let version = json["version"].as_str().expect("version must be a string");
    assert!(!version.is_empty(), "version must not be empty");
}

/// 14.3: GET /health returns `"error"` when no providers are configured.
///
/// The test app has `db_pool: None` and no API keys set, so the provider
/// registry is empty. The health endpoint status logic returns `"error"`
/// when zero providers are configured (regardless of DB/Redis state).
/// HTTP status is always 200 (Fly.io health checks need 2xx).
/// Authenticated with admin token to verify detailed checks.
#[tokio::test]
async fn test_health_returns_error_without_providers() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .header("Authorization", format!("Bearer {TEST_ADMIN_TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Health endpoint always returns HTTP 200
    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // No providers configured in test env → "error"
    assert_eq!(json["status"], "error");

    // DB and Redis are not configured (not errored), so checks reflect that
    assert_eq!(json["checks"]["database"], "not_configured");
    assert_eq!(json["checks"]["redis"], "not_configured");
}

/// 14.3: GET /health response contains a `checks` object with `providers` array
/// when authenticated with admin token.
///
/// Verifies the expanded health response shape: `checks` object with
/// `database`, `redis`, `providers`, and `solana_rpc` fields.
#[tokio::test]
async fn test_health_returns_checks_with_providers() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .header("Authorization", format!("Bearer {TEST_ADMIN_TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Verify the checks object structure
    assert!(json["checks"].is_object(), "checks must be an object");
    assert!(
        json["checks"]["providers"].is_array(),
        "checks.providers must be an array"
    );
    assert!(
        json["checks"]["database"].is_string(),
        "checks.database must be a string"
    );
    assert!(
        json["checks"]["redis"].is_string(),
        "checks.redis must be a string"
    );
    assert!(
        json["checks"]["solana_rpc"].is_string(),
        "checks.solana_rpc must be a string"
    );

    // status and version always present
    assert!(json["status"].is_string());
    assert!(json["version"].is_string());
}
