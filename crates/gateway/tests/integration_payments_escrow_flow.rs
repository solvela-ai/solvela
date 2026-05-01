//! Integration tests — payments_escrow_flow.
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
async fn test_402_offers_escrow_when_configured() {
    let app = test_app_with_escrow();

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

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let error_msg = json["error"]["message"].as_str().unwrap();
    let pr: serde_json::Value = serde_json::from_str(error_msg).unwrap();

    let accepts = pr["accepts"].as_array().unwrap();
    assert_eq!(
        accepts.len(),
        2,
        "should offer both exact and escrow schemes"
    );
    assert_eq!(accepts[0]["scheme"], "exact");
    assert_eq!(accepts[1]["scheme"], "escrow");
    assert!(
        accepts[1]["escrow_program_id"].is_string(),
        "escrow accept should include escrow_program_id"
    );
}

#[tokio::test]
async fn test_402_no_escrow_when_not_configured() {
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

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let error_msg = json["error"]["message"].as_str().unwrap();
    let pr: serde_json::Value = serde_json::from_str(error_msg).unwrap();

    let accepts = pr["accepts"].as_array().unwrap();
    assert_eq!(accepts.len(), 1, "should only offer exact scheme");
    assert_eq!(accepts[0]["scheme"], "exact");
}

#[tokio::test]
async fn test_escrow_payment_header_accepted() {
    let app = test_app_with_mock_provider_and_escrow();

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
                    valid_escrow_payment_header("/v1/chat/completions"),
                )
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // Escrow verifier passes, mock provider returns a response
    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["object"], "chat.completion");
    assert_eq!(json["choices"][0]["message"]["content"], "[mock response]");
}

#[tokio::test]
async fn test_escrow_scheme_dispatches_to_escrow_verifier() {
    // Build a facilitator with both verifiers and verify routing
    let exact_verifier = Arc::new(AlwaysPassVerifier);
    let escrow_verifier = Arc::new(AlwaysPassEscrowVerifier);

    let facilitator =
        solvela_x402::facilitator::Facilitator::new(vec![exact_verifier, escrow_verifier]);

    // Build an escrow payload
    let payload = PaymentPayload {
        x402_version: 2,
        resource: Resource {
            url: "/v1/chat/completions".to_string(),
            method: "POST".to_string(),
        },
        accepted: PaymentAccept {
            scheme: "escrow".to_string(),
            network: SOLANA_NETWORK.to_string(),
            amount: "2625".to_string(),
            asset: USDC_MINT.to_string(),
            pay_to: "TestRecipient".to_string(),
            max_timeout_seconds: 300,
            escrow_program_id: Some("9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU".to_string()),
        },
        payload: PayloadData::Escrow(EscrowPayload {
            deposit_tx: base64::engine::general_purpose::STANDARD.encode(b"mock_deposit_tx"),
            service_id: base64::engine::general_purpose::STANDARD.encode([0u8; 32]),
            agent_pubkey: "11111111111111111111111111111111".to_string(),
        }),
    };

    // Verify routes to escrow verifier
    let result = facilitator.verify(&payload).await;
    assert!(result.is_ok());
    assert!(result.unwrap().valid);

    // Verify and settle routes to escrow verifier
    let result = facilitator.verify_and_settle(&payload).await;
    assert!(result.is_ok());
    let settlement = result.unwrap();
    assert!(settlement.success);
    assert_eq!(
        settlement.tx_signature,
        Some("MockEscrowSettledTxSig123".to_string())
    );
}

/// Test that submitting scheme="exact" with an escrow PayloadData returns 400.
#[tokio::test]
async fn test_scheme_payload_mismatch_exact_with_escrow_returns_400() {
    let app = test_app_with_escrow();

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
                    mismatched_exact_scheme_escrow_payload_header("/v1/chat/completions"),
                )
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "scheme-payload mismatch (exact scheme + escrow data) must return 400"
    );

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["type"], "invalid_request_error");
    assert!(
        json["error"]["message"].as_str().unwrap().contains("exact")
            && json["error"]["message"]
                .as_str()
                .unwrap()
                .contains("escrow"),
        "error message should mention the scheme-payload mismatch"
    );
}

/// Test that submitting scheme="escrow" with a direct PayloadData returns 400.
#[tokio::test]
async fn test_scheme_payload_mismatch_escrow_with_direct_returns_400() {
    let app = test_app_with_escrow();

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
                    mismatched_escrow_scheme_direct_payload_header("/v1/chat/completions"),
                )
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "scheme-payload mismatch (escrow scheme + direct data) must return 400"
    );

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["type"], "invalid_request_error");
    assert!(
        json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("escrow")
            && json["error"]["message"]
                .as_str()
                .unwrap()
                .contains("direct"),
        "error message should mention the scheme-payload mismatch"
    );
}
