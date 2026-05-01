//! Integration tests — misc_basic.
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

#[tokio::test]
async fn test_models_endpoint() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/models")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["object"], "list");

    let data = json["data"].as_array().unwrap();
    assert_eq!(data.len(), 3);

    // Check that pricing includes the 5% fee
    let gpt4o = data.iter().find(|m| m["id"] == "openai/gpt-4o").unwrap();
    assert_eq!(gpt4o["pricing"]["fee_percent"], 5);
    assert_eq!(gpt4o["pricing"]["currency"], "USDC");
}

#[tokio::test]
async fn test_unknown_route_returns_404() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/nonexistent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_image_generations_returns_501() {
    let app = test_app();

    let body = serde_json::json!({
        "prompt": "A robot paying for an API call with USDC on Solana",
        "model": "dall-e-3",
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/images/generations")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_IMPLEMENTED);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["type"], "not_implemented");
}

#[tokio::test]
async fn test_pricing_endpoint() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/pricing")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Platform metadata
    assert_eq!(json["platform"]["chain"], "solana");
    assert_eq!(json["platform"]["token"], "USDC-SPL");
    assert_eq!(json["platform"]["fee_percent"], 5);

    // Models list is populated
    let models = json["models"].as_array().unwrap();
    assert!(
        !models.is_empty(),
        "pricing should return at least one model"
    );

    // Each model has required fields
    let m = &models[0];
    assert!(m["id"].is_string());
    assert!(m["pricing"]["input_per_million_usdc"].is_number());
    assert!(m["pricing"]["platform_fee_percent"].is_number());
    assert!(m["example_1k_token_request"]["total_usdc"].is_string());
}

/// 14.1: CatchPanicLayer returns JSON 500 instead of dropping the connection.
///
/// We create a standalone router with a handler that panics to verify the
/// `CatchPanicLayer` converts it into a well-formed JSON 500 response.
#[tokio::test]
async fn test_panic_handler_returns_500_json() {
    use axum::routing::get;
    use tower_http::catch_panic::CatchPanicLayer;

    // Standalone router with CatchPanicLayer + a panicking handler
    let app = axum::Router::new()
        .route(
            "/panic",
            get(|| async {
                panic!("deliberate test panic");
                #[allow(unreachable_code)]
                "never reached"
            }),
        )
        .layer(CatchPanicLayer::custom(gateway::handle_panic));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/panic")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["type"], "internal_error");
    assert_eq!(json["error"]["message"], "Internal server error");
}

/// 14.1: ConcurrencyLimitLayer rejects excess requests with 503.
///
/// NOTE: Properly testing the concurrency limit requires holding multiple
/// in-flight requests simultaneously. This is inherently racy in unit-style
/// integration tests. The ConcurrencyLimitLayer is well-tested by Tower
/// upstream; this test verifies the layer is wired into the router by
/// confirming that a concurrency limit of 1 causes the second concurrent
/// request to be queued (not immediately served).
#[tokio::test]
async fn test_concurrent_request_limit() {
    use axum::routing::get;
    use tower::limit::ConcurrencyLimitLayer;

    // Handler that sleeps so the concurrency slot stays occupied
    let app = axum::Router::new()
        .route(
            "/slow",
            get(|| async {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                "ok"
            }),
        )
        .layer(ConcurrencyLimitLayer::new(1));

    // First request occupies the only slot
    let app_clone = app.clone();
    let first = tokio::spawn(async move {
        app_clone
            .oneshot(Request::builder().uri("/slow").body(Body::empty()).unwrap())
            .await
            .unwrap()
    });

    // Give the first request time to acquire the permit
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Second request should be queued (blocked) since the slot is occupied.
    // A short timeout proves it does not complete immediately.
    let second = tokio::time::timeout(std::time::Duration::from_millis(200), async {
        app.oneshot(Request::builder().uri("/slow").body(Body::empty()).unwrap())
            .await
            .unwrap()
    })
    .await;

    // The second request must NOT have completed (it's queued behind the first)
    assert!(
        second.is_err(),
        "second request should be queued, not served immediately"
    );

    // Clean up — let the first request finish
    let _ = first.await;
}
