//! Integration tests — services.
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
async fn test_services_endpoint_returns_all() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/services")
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
    // TEST_SERVICES_TOML has 3 services
    assert_eq!(data.len(), 3);
    assert_eq!(json["total"], 3);
}

#[tokio::test]
async fn test_services_each_entry_has_required_fields() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/services")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let data = json["data"].as_array().unwrap();

    for svc in data {
        assert!(svc["id"].is_string(), "missing id");
        assert!(svc["name"].is_string(), "missing name");
        assert!(svc["category"].is_string(), "missing category");
        assert!(svc["endpoint"].is_string(), "missing endpoint");
        assert!(svc["x402_enabled"].is_boolean(), "missing x402_enabled");
        assert!(svc["internal"].is_boolean(), "missing internal");
        assert!(svc["pricing"].is_string(), "missing pricing");
        let chains = svc["chains"].as_array().unwrap();
        assert!(
            chains.iter().any(|c| c == "solana"),
            "chains must include solana"
        );
    }
}

#[tokio::test]
async fn test_services_filter_by_category() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/services?category=intelligence")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let data = json["data"].as_array().unwrap();

    assert_eq!(data.len(), 1);
    assert_eq!(data[0]["id"], "llm-gateway");
    assert_eq!(data[0]["category"], "intelligence");
}

#[tokio::test]
async fn test_services_filter_by_internal_true() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/services?internal=true")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let data = json["data"].as_array().unwrap();

    // Only llm-gateway is internal in TEST_SERVICES_TOML
    assert_eq!(data.len(), 1);
    assert_eq!(data[0]["internal"], true);
}

#[tokio::test]
async fn test_services_filter_by_internal_false() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/services?internal=false")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let data = json["data"].as_array().unwrap();

    // legacy-api and web-search are external in TEST_SERVICES_TOML
    assert_eq!(data.len(), 2);
    for svc in data {
        assert_eq!(svc["internal"], false);
    }
}

#[tokio::test]
async fn test_services_unknown_category_returns_empty() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/services?category=doesnotexist")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let data = json["data"].as_array().unwrap();
    assert_eq!(data.len(), 0);
    assert_eq!(json["total"], 0);
}

#[tokio::test]
async fn test_proxy_returns_404_for_unknown_service() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/services/nonexistent-service/proxy")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query":"test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["type"], "invalid_request_error"); // code stayed the same ("model_not_found"); type changed by error envelope normalization
    assert!(json["error"]["message"]
        .as_str()
        .unwrap()
        .contains("nonexistent-service"));
}

#[tokio::test]
async fn test_proxy_returns_400_for_internal_service() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/services/llm-gateway/proxy")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query":"test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["type"], "invalid_request_error");
    assert!(json["error"]["message"]
        .as_str()
        .unwrap()
        .contains("internal"));
}

#[tokio::test]
async fn test_proxy_returns_400_for_non_x402_service() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/services/legacy-api/proxy")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query":"test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["type"], "invalid_request_error");
    assert!(json["error"]["message"].as_str().unwrap().contains("x402"));
}

#[tokio::test]
async fn test_proxy_returns_402_with_cost_breakdown() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/services/web-search/proxy")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query":"test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // The 402 response is wrapped inside the error.message as serialized JSON
    let error_msg = json["error"]["message"].as_str().unwrap();
    let payment_info: serde_json::Value = serde_json::from_str(error_msg).unwrap();

    assert_eq!(payment_info["x402_version"], 2);
    assert_eq!(payment_info["error"], "Payment required");

    // Verify cost breakdown
    let cost = &payment_info["cost_breakdown"];
    assert_eq!(cost["currency"], "USDC");
    assert_eq!(cost["fee_percent"], 5);
    // provider_cost should be 0.005000 (web-search price)
    assert_eq!(cost["provider_cost"], "0.005000");
    // platform_fee should be 5% of 0.005 = 0.000250
    assert_eq!(cost["platform_fee"], "0.000250");
    // total = 0.005 + 0.00025 = 0.005250
    assert_eq!(cost["total"], "0.005250");

    // Verify resource URL matches the proxy path
    assert_eq!(
        payment_info["resource"]["url"],
        "/v1/services/web-search/proxy"
    );
    assert_eq!(payment_info["resource"]["method"], "POST");

    // Verify accepts array has Solana/USDC payment scheme
    let accepts = payment_info["accepts"].as_array().unwrap();
    assert!(!accepts.is_empty());
    assert_eq!(accepts[0]["scheme"], "exact");
    assert_eq!(
        accepts[0]["network"],
        "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp"
    );
    assert_eq!(
        accepts[0]["asset"],
        "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
    );
}

#[tokio::test]
async fn test_proxy_returns_503_for_unhealthy_service() {
    let (app, state) = test_app_with_state();

    // Mark web-search as unhealthy via write lock on service_registry
    {
        let mut registry = state.service_registry.write().await;
        registry.set_health("web-search", false);
    }

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/services/web-search/proxy")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"query":"test"}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["error"].as_str().unwrap().contains("unavailable"));
}

#[tokio::test]
async fn test_register_service_requires_auth() {
    // Set the admin token env var so the endpoint is exposed

    let app = test_app();

    // No Authorization header
    let body = serde_json::json!({
        "id": "test-svc-no-auth",
        "name": "Test No Auth",
        "endpoint": "https://api.example.com/v1",
        "category": "data",
        "price_per_request_usdc": 0.01
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/services/register")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
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
async fn test_register_service_creates_entry() {
    let (app, state) = test_app_with_state();

    let body = serde_json::json!({
        "id": "my-new-api",
        "name": "My New API",
        "endpoint": "https://api.newservice.com/v1",
        "category": "data",
        "description": "A brand new service",
        "price_per_request_usdc": 0.02
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/services/register")
                .header("content-type", "application/json")
                .header("Authorization", format!("Bearer {TEST_ADMIN_TOKEN}"))
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);

    let resp_body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
    assert_eq!(json["id"], "my-new-api");
    assert_eq!(json["name"], "My New API");
    assert_eq!(json["source"], "api");
    assert_eq!(json["internal"], false);
    assert_eq!(json["x402_enabled"], true);

    // Verify the service appears in the registry via direct read
    let registry = state.service_registry.read().await;
    let entry = registry.get("my-new-api");
    assert!(entry.is_some());
    assert_eq!(entry.unwrap().name, "My New API");
}

#[tokio::test]
async fn test_register_service_rejects_duplicate_id() {
    let app = test_app();

    let body = serde_json::json!({
        "id": "web-search",
        "name": "Duplicate Web Search",
        "endpoint": "https://other-search.example.com/v1",
        "category": "search",
        "price_per_request_usdc": 0.01
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/services/register")
                .header("content-type", "application/json")
                .header("Authorization", format!("Bearer {TEST_ADMIN_TOKEN}"))
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);

    let resp_body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
    assert!(json["error"].as_str().unwrap().contains("already exists"));
}

#[tokio::test]
async fn test_register_service_validates_https() {
    let app = test_app();

    let body = serde_json::json!({
        "id": "insecure-svc",
        "name": "Insecure Service",
        "endpoint": "http://insecure.example.com/v1",
        "category": "data",
        "price_per_request_usdc": 0.01
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/services/register")
                .header("content-type", "application/json")
                .header("Authorization", format!("Bearer {TEST_ADMIN_TOKEN}"))
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let resp_body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&resp_body).unwrap();
    assert!(json["error"].as_str().unwrap().contains("https"));
}

#[tokio::test]
async fn test_register_service_validates_required_fields() {
    let app = test_app();

    // Empty id should fail validation
    let body = serde_json::json!({
        "id": "",
        "name": "Empty ID Service",
        "endpoint": "https://example.com/v1",
        "category": "data",
        "price_per_request_usdc": 0.01
    });

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/services/register")
                .header("content-type", "application/json")
                .header("Authorization", format!("Bearer {TEST_ADMIN_TOKEN}"))
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_services_list_includes_health_status() {
    let (app, state) = test_app_with_state();

    // Set health on web-search to true
    {
        let mut registry = state.service_registry.write().await;
        registry.set_health("web-search", true);
    }

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/services")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let data = json["data"].as_array().unwrap();

    // Find web-search and verify healthy=true
    let ws = data.iter().find(|s| s["id"] == "web-search").unwrap();
    assert_eq!(ws["healthy"], true);

    // Other services should have healthy=null (never checked)
    let llm = data.iter().find(|s| s["id"] == "llm-gateway").unwrap();
    assert!(llm["healthy"].is_null());
}

#[tokio::test]
async fn test_services_list_includes_registered_services() {
    let (router, state) = test_app_with_state();

    // Register a new service directly via the registry
    {
        let mut registry = state.service_registry.write().await;
        registry
            .register(gateway::services::ServiceEntry {
                id: "runtime-svc".to_string(),
                name: "Runtime Service".to_string(),
                category: "compute".to_string(),
                endpoint: "https://runtime.example.com/v1".to_string(),
                x402_enabled: true,
                internal: false,
                description: Some("Dynamically registered".to_string()),
                pricing_label: "$0.05/request".to_string(),
                chains: vec!["solana".to_string()],
                source: "api".to_string(),
                healthy: None,
                price_per_request_usdc: Some(0.05),
            })
            .unwrap();
    }

    // Now GET /v1/services should include the new service
    let response = router
        .oneshot(
            Request::builder()
                .uri("/v1/services")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let data = json["data"].as_array().unwrap();

    // Should include the runtime-registered service (3 from TOML + 1 registered = 4)
    assert_eq!(data.len(), 4);

    let runtime_svc = data.iter().find(|s| s["id"] == "runtime-svc").unwrap();
    assert_eq!(runtime_svc["name"], "Runtime Service");
    assert_eq!(runtime_svc["source"], "api");
    assert_eq!(runtime_svc["category"], "compute");
}
