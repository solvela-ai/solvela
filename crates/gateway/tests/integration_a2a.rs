//! Integration tests — a2a.
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
async fn test_a2a_agent_card_returns_capabilities() {
    let app = test_app();
    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/.well-known/agent.json")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["name"], "Solvela");
    assert_eq!(json["version"], "0.1.0");
    let extensions = json["capabilities"]["extensions"].as_array().unwrap();
    assert!(extensions.len() >= 2, "should have AP2 + x402 extensions");
}

#[tokio::test]
async fn test_a2a_unknown_method_returns_method_not_found() {
    let app = test_app();
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "unknown/method",
        "id": "1",
        "params": {}
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/a2a")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK); // JSON-RPC errors use 200
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], -32601);
}

#[tokio::test]
async fn test_a2a_message_send_requires_redis() {
    // test_app() has cache: None — new A2A requests must be rejected without Redis
    // because clients cannot pay USDC against a task that cannot be persisted and
    // loaded back. ERR_INTERNAL (-32603) is returned to signal the store is unavailable.
    let app = test_app();
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "message/send",
        "id": "req-1",
        "params": {
            "message": {
                "role": "user",
                "parts": [{"kind": "text", "text": "Hello, what is Solana?"}],
                "metadata": {"model": "openai-gpt-4o"}
            }
        }
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/a2a")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // ERR_INTERNAL = -32603: task store unavailable
    assert_eq!(
        json["error"]["code"], -32603_i32,
        "should return ERR_INTERNAL when Redis is unavailable"
    );
    assert!(json["result"].is_null(), "result should be null on error");
}

#[tokio::test]
async fn test_a2a_echoes_extension_header() {
    let app = test_app();
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "message/send",
        "id": "1",
        "params": {
            "message": {
                "role": "user",
                "parts": [{"kind": "text", "text": "test"}],
                "metadata": {"model": "openai-gpt-4o"}
            }
        }
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/a2a")
                .header("content-type", "application/json")
                .header(
                    "x-a2a-extensions",
                    "https://github.com/google-a2a/a2a-x402/v0.1",
                )
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert!(
        resp.headers().contains_key("x-a2a-extensions"),
        "should echo extension header"
    );
}

#[tokio::test]
async fn test_a2a_invalid_jsonrpc_version() {
    let app = test_app();
    let body = serde_json::json!({
        "jsonrpc": "1.0",
        "method": "message/send",
        "id": "1",
        "params": {}
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/a2a")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["code"], -32700);
}

#[tokio::test]
async fn test_a2a_message_send_no_text_returns_error() {
    let app = test_app();
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "message/send",
        "id": "1",
        "params": {
            "message": {
                "role": "user",
                "parts": [{"kind": "data", "contentType": "application/json", "data": {}}]
            }
        }
    });
    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/a2a")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["error"].is_object(), "should return JSON-RPC error");
}
