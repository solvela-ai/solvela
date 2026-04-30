//! Integration tests — chat_request_debug.
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
async fn test_request_id_always_present_on_success() {
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
    let request_id = response.headers().get("x-rcr-request-id");
    assert!(
        request_id.is_some(),
        "X-RCR-Request-Id must be present on all responses"
    );
    // Should be a valid UUID (36 chars with dashes)
    let id_str = request_id.unwrap().to_str().unwrap();
    assert_eq!(id_str.len(), 36, "server-generated ID should be a UUID");
}

#[tokio::test]
async fn test_request_id_always_present_on_402() {
    let app = test_app();

    let body = serde_json::json!({
        "model": "openai/gpt-4o",
        "messages": [{"role": "user", "content": "Hello!"}],
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
    assert!(
        response.headers().get("x-rcr-request-id").is_some(),
        "X-RCR-Request-Id must be present on 402 responses"
    );
}

#[tokio::test]
async fn test_client_provided_request_id_echoed() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .header("x-request-id", "my-custom-id-123")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("x-rcr-request-id")
            .unwrap()
            .to_str()
            .unwrap(),
        "my-custom-id-123",
        "client-provided request ID should be echoed back"
    );
}

#[tokio::test]
async fn test_invalid_request_id_replaced_with_uuid() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .header("x-request-id", "invalid id with spaces!")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let id = response
        .headers()
        .get("x-rcr-request-id")
        .unwrap()
        .to_str()
        .unwrap();
    assert_ne!(
        id, "invalid id with spaces!",
        "invalid ID should be replaced"
    );
    assert_eq!(id.len(), 36, "replacement should be a UUID");
}

#[tokio::test]
async fn test_oversized_request_id_replaced_with_uuid() {
    let app = test_app();
    let long_id = "a".repeat(200);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .header("x-request-id", &long_id)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let id = response
        .headers()
        .get("x-rcr-request-id")
        .unwrap()
        .to_str()
        .unwrap();
    assert_ne!(id, &long_id, "oversized ID should be replaced");
    assert_eq!(id.len(), 36, "replacement should be a UUID");
}

#[tokio::test]
async fn test_no_debug_headers_without_flag() {
    let app = test_app_with_mock_provider();

    let body = serde_json::json!({
        "model": "openai/gpt-4o",
        "messages": [{"role": "user", "content": "Hello!"}],
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .header(
                    "payment-signature",
                    valid_payment_header("/v1/chat/completions"),
                )
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    // Request ID should always be present
    assert!(response.headers().get("x-rcr-request-id").is_some());
    // Debug headers should NOT be present
    assert!(
        response.headers().get("x-rcr-model").is_none(),
        "x-rcr-model must not leak without debug flag"
    );
    assert!(response.headers().get("x-rcr-tier").is_none());
    assert!(response.headers().get("x-rcr-score").is_none());
    assert!(response.headers().get("x-rcr-provider").is_none());
    assert!(response.headers().get("x-rcr-cache").is_none());
    assert!(response.headers().get("x-rcr-latency-ms").is_none());
    assert!(response.headers().get("x-rcr-payment-status").is_none());
}

#[tokio::test]
async fn test_debug_headers_present_with_flag() {
    let app = test_app_with_mock_provider();

    let body = serde_json::json!({
        "model": "openai/gpt-4o",
        "messages": [{"role": "user", "content": "Hello!"}],
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .header("x-rcr-debug", "true")
                .header(
                    "payment-signature",
                    valid_payment_header("/v1/chat/completions"),
                )
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    // All debug headers should be present on successful responses
    assert!(
        response.headers().get("x-rcr-model").is_some(),
        "x-rcr-model must be present with debug flag"
    );
    assert!(
        response.headers().get("x-rcr-provider").is_some(),
        "x-rcr-provider must be present with debug flag"
    );
    assert!(
        response.headers().get("x-rcr-cache").is_some(),
        "x-rcr-cache must be present with debug flag"
    );
    assert!(
        response.headers().get("x-rcr-latency-ms").is_some(),
        "x-rcr-latency-ms must be present with debug flag"
    );
    assert!(
        response.headers().get("x-rcr-payment-status").is_some(),
        "x-rcr-payment-status must be present with debug flag"
    );
    assert_eq!(
        response
            .headers()
            .get("x-rcr-payment-status")
            .unwrap()
            .to_str()
            .unwrap(),
        "verified"
    );
}

#[tokio::test]
async fn test_debug_flag_false_no_debug_headers() {
    let app = test_app_with_mock_provider();

    let body = serde_json::json!({
        "model": "openai/gpt-4o",
        "messages": [{"role": "user", "content": "Hello!"}],
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .header("x-rcr-debug", "false")
                .header(
                    "payment-signature",
                    valid_payment_header("/v1/chat/completions"),
                )
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    // Debug headers should NOT be present when flag is "false"
    assert!(response.headers().get("x-rcr-model").is_none());
    assert!(response.headers().get("x-rcr-tier").is_none());
}

#[tokio::test]
async fn test_debug_headers_on_smart_routed_request() {
    let app = test_app_with_mock_provider();

    // Use "eco" profile — Simple tier maps to deepseek-chat which is in test registry
    let body = serde_json::json!({
        "model": "eco",
        "messages": [{"role": "user", "content": "Hello!"}],
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .header("x-rcr-debug", "true")
                .header(
                    "payment-signature",
                    valid_payment_header("/v1/chat/completions"),
                )
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    // Smart-routed request should have routing debug headers
    assert!(
        response.headers().get("x-rcr-model").is_some(),
        "x-rcr-model must be present on smart-routed debug request"
    );
    assert!(
        response.headers().get("x-rcr-profile").is_some(),
        "x-rcr-profile must be present on smart-routed request"
    );
    assert!(
        response.headers().get("x-rcr-tier").is_some(),
        "x-rcr-tier must be present on smart-routed request"
    );
    assert!(
        response.headers().get("x-rcr-score").is_some(),
        "x-rcr-score must be present on smart-routed request"
    );
}

/// G.2 Test 8: Request ID present on 500 error responses.
///
/// A paid request with no real provider configured returns 500.
/// The RequestIdLayer middleware should still attach the request ID.
#[tokio::test]
async fn test_request_id_present_on_500_error() {
    let app = test_app();

    let body = serde_json::json!({
        "model": "openai/gpt-4o",
        "messages": [{"role": "user", "content": "Hello!"}],
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .header(
                    "payment-signature",
                    valid_payment_header("/v1/chat/completions"),
                )
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert!(
        response.headers().get("x-rcr-request-id").is_some(),
        "X-RCR-Request-Id must be present on 500 error responses"
    );
    // Should be a valid UUID (36 chars with dashes)
    let id = response
        .headers()
        .get("x-rcr-request-id")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(id.len(), 36, "server-generated ID should be a UUID");
}

/// G.2 Test 9: Request ID present on streaming request responses.
///
/// Streaming requests still go through the RequestIdLayer, so the
/// X-RCR-Request-Id header should be attached even for SSE responses.
/// Without a real provider, the paid streaming request returns 500.
#[tokio::test]
async fn test_request_id_present_on_streaming_request() {
    let app = test_app();

    let body = serde_json::json!({
        "model": "openai/gpt-4o",
        "messages": [{"role": "user", "content": "Hello!"}],
        "stream": true,
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .header(
                    "payment-signature",
                    valid_payment_header("/v1/chat/completions"),
                )
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // With no real provider, streaming paid requests also fail.
    // The RequestIdLayer middleware runs regardless.
    assert!(
        response.headers().get("x-rcr-request-id").is_some(),
        "X-RCR-Request-Id must be present on streaming responses"
    );
}

/// G.2 Test 10: Debug headers on streaming responses when flag set.
///
/// When `X-RCR-Debug: true` is set on a streaming request, debug headers
/// should be attached on successful responses.
#[tokio::test]
async fn test_debug_headers_on_streaming_with_flag() {
    let app = test_app_with_mock_provider();

    let body = serde_json::json!({
        "model": "openai/gpt-4o",
        "messages": [{"role": "user", "content": "Hello!"}],
        "stream": true,
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .header("x-rcr-debug", "true")
                .header(
                    "payment-signature",
                    valid_payment_header("/v1/chat/completions"),
                )
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get("x-rcr-request-id").is_some());
    // Debug headers should be present on successful streaming responses
    assert!(
        response.headers().get("x-rcr-model").is_some(),
        "x-rcr-model must be present on streaming debug response"
    );
    assert!(
        response.headers().get("x-rcr-provider").is_some(),
        "x-rcr-provider must be present on streaming debug response"
    );
}

/// G.2 Test 11: Cache miss reflected in X-RCR-Cache header.
///
/// Since integration tests don't have Redis configured (`cache: None`),
/// all non-streaming requests show cache_status = Miss.
#[tokio::test]
async fn test_cache_miss_on_non_streaming_without_redis() {
    let app = test_app_with_mock_provider();

    let body = serde_json::json!({
        "model": "openai/gpt-4o",
        "messages": [{"role": "user", "content": "Hello!"}],
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .header("x-rcr-debug", "true")
                .header(
                    "payment-signature",
                    valid_payment_header("/v1/chat/completions"),
                )
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers().get("x-rcr-request-id").is_some());
    // Cache header should show "miss" when no Redis is configured
    let cache_header = response
        .headers()
        .get("x-rcr-cache")
        .expect("x-rcr-cache must be present with debug flag");
    assert_eq!(
        cache_header.to_str().unwrap(),
        "miss",
        "cache status should be 'miss' without Redis"
    );
}

/// G.2 Test 15: Debug headers not leaked when flag is absent (security).
///
/// Comprehensive security check: even on a paid 500 error, no routing
/// internals should leak in response headers when the debug flag is absent.
#[tokio::test]
async fn test_debug_headers_not_leaked_security() {
    let app = test_app();

    let body = serde_json::json!({
        "model": "openai/gpt-4o",
        "messages": [{"role": "user", "content": "Hello!"}],
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .header(
                    "payment-signature",
                    valid_payment_header("/v1/chat/completions"),
                )
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // Verify NO debug headers are present (security requirement)
    let debug_headers = [
        "x-rcr-model",
        "x-rcr-tier",
        "x-rcr-score",
        "x-rcr-profile",
        "x-rcr-provider",
        "x-rcr-cache",
        "x-rcr-latency-ms",
        "x-rcr-payment-status",
        "x-rcr-token-estimate-in",
        "x-rcr-token-estimate-out",
    ];
    for header in &debug_headers {
        assert!(
            response.headers().get(*header).is_none(),
            "debug header '{}' must not be present without X-RCR-Debug: true",
            header
        );
    }
    // Request ID is NOT a debug header — it should always be present
    assert!(response.headers().get("x-rcr-request-id").is_some());
}
