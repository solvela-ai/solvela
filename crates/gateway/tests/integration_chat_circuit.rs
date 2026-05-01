//! Integration tests — chat_circuit.
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
async fn test_circuit_breaker_model_state_queryable() {
    let (_app, state) = test_app_with_state();

    // Initially closed
    assert_eq!(
        state
            .provider_health
            .get_model_state("openai", "gpt-4o")
            .await,
        gateway::providers::health::CircuitState::Closed
    );

    // Record failures to open it
    for _ in 0..5 {
        state
            .provider_health
            .record_model_failure("openai", "gpt-4o", 500)
            .await;
    }

    assert_eq!(
        state
            .provider_health
            .get_model_state("openai", "gpt-4o")
            .await,
        gateway::providers::health::CircuitState::Open
    );

    // Other models unaffected
    assert_eq!(
        state
            .provider_health
            .get_model_state("openai", "gpt-4o-mini")
            .await,
        gateway::providers::health::CircuitState::Closed
    );
}

#[tokio::test]
async fn test_chat_with_broken_model_circuit_returns_stub() {
    let (app, state) = test_app_with_state();

    // Open the circuit for the requested model
    for _ in 0..5 {
        state
            .provider_health
            .record_model_failure("openai", "gpt-4o", 500)
            .await;
    }

    let body = serde_json::json!({
        "model": "openai-gpt-4o",
        "messages": [{"role": "user", "content": "hello"}],
    });

    let req = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("content-type", "application/json")
        .header(
            "payment-signature",
            &valid_payment_header("/v1/chat/completions"),
        )
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    // In test env with no real providers, paid requests return 500
    // (security fix: never serve stub responses to paying users) or 402
    assert!(
        resp.status() == StatusCode::INTERNAL_SERVER_ERROR
            || resp.status() == StatusCode::PAYMENT_REQUIRED,
        "expected 500 or 402, got {}",
        resp.status()
    );
}
