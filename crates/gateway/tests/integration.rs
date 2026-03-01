//! Integration tests for the RustyClawRouter gateway.
//!
//! These tests spin up the Axum app in-process using `tower::ServiceExt`
//! and exercise the HTTP endpoints without needing a running server.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use http_body_util::BodyExt;
use tower::ServiceExt;

use gateway::config::AppConfig;
use gateway::providers::health::{CircuitBreakerConfig, ProviderHealthTracker};
use gateway::providers::ProviderRegistry;
use gateway::{build_router, AppState};
use router::models::ModelRegistry;
use x402::traits::{Error as X402Error, PaymentVerifier};
use x402::types::{PaymentPayload, SettlementResult, VerificationResult, SOLANA_NETWORK};

// ---------------------------------------------------------------------------
// Mock payment verifier for integration tests
// ---------------------------------------------------------------------------

/// A mock verifier that accepts all structurally-valid payment payloads.
/// Used so integration tests can exercise the full request path without
/// a live Solana RPC connection.
struct AlwaysPassVerifier;

#[async_trait::async_trait]
impl PaymentVerifier for AlwaysPassVerifier {
    fn network(&self) -> &str {
        SOLANA_NETWORK
    }

    async fn verify_payment(
        &self,
        _payload: &PaymentPayload,
    ) -> Result<VerificationResult, X402Error> {
        Ok(VerificationResult {
            valid: true,
            reason: None,
            verified_amount: Some(2625),
        })
    }

    async fn settle_payment(
        &self,
        _payload: &PaymentPayload,
    ) -> Result<SettlementResult, X402Error> {
        Ok(SettlementResult {
            success: true,
            tx_signature: Some("MockSettledTxSig123".to_string()),
            network: SOLANA_NETWORK.to_string(),
            error: None,
        })
    }
}

const TEST_MODELS_TOML: &str = r#"
[models.openai-gpt-4o]
provider = "openai"
model_id = "gpt-4o"
display_name = "GPT-4o"
input_cost_per_million = 2.50
output_cost_per_million = 10.00
context_window = 128000
supports_streaming = true
supports_tools = true
supports_vision = true

[models.deepseek-chat]
provider = "deepseek"
model_id = "deepseek-chat"
display_name = "DeepSeek V3.2 Chat"
input_cost_per_million = 0.28
output_cost_per_million = 0.42
context_window = 128000
supports_streaming = true

[models.anthropic-claude-sonnet]
provider = "anthropic"
model_id = "claude-sonnet-4.6"
display_name = "Claude Sonnet 4.6"
input_cost_per_million = 3.00
output_cost_per_million = 15.00
context_window = 200000
supports_streaming = true
supports_tools = true
supports_vision = true
"#;

/// Build a test app with the test model config (no real provider API keys).
///
/// Uses `AlwaysPassVerifier` so that properly-structured PaymentPayload headers
/// pass verification without a live Solana RPC connection. Malformed headers
/// (non-base64, non-JSON) are still correctly rejected by the route handler.
fn test_app() -> axum::Router {
    let model_registry = ModelRegistry::from_toml(TEST_MODELS_TOML).unwrap();

    // Use the always-pass mock verifier so tests exercise the full request path
    let facilitator = x402::facilitator::Facilitator::new(vec![Arc::new(AlwaysPassVerifier)]);

    let state = Arc::new(AppState {
        config: AppConfig::default(),
        model_registry,
        providers: ProviderRegistry::from_env(), // No keys set in test env
        facilitator,
        usage: gateway::usage::UsageTracker::noop(),
        cache: None, // No Redis in tests (replay check is skipped when cache=None)
        provider_health: ProviderHealthTracker::new(CircuitBreakerConfig::default()),
    });
    build_router(state)
}

/// Build a minimal valid PaymentPayload base64-encoded header for a given model path.
fn valid_payment_header(resource_url: &str) -> String {
    let payload = x402::types::PaymentPayload {
        x402_version: 2,
        resource: x402::types::Resource {
            url: resource_url.to_string(),
            method: "POST".to_string(),
        },
        accepted: x402::types::PaymentAccept {
            scheme: "exact".to_string(),
            network: SOLANA_NETWORK.to_string(),
            amount: "2625".to_string(),
            asset: x402::types::USDC_MINT.to_string(),
            pay_to: "GatewayRecipientWallet111111111111111111111111".to_string(),
            max_timeout_seconds: 300,
        },
        payload: x402::types::SolanaPayload {
            transaction: base64::engine::general_purpose::STANDARD.encode(b"mock_signed_tx_bytes"),
        },
    };
    let json = serde_json::to_vec(&payload).unwrap();
    base64::engine::general_purpose::STANDARD.encode(&json)
}

// ---------------------------------------------------------------------------
// GET /health
// ---------------------------------------------------------------------------

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
    assert_eq!(json["status"], "ok");
    assert!(json["version"].is_string());
}

// ---------------------------------------------------------------------------
// GET /v1/models
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// POST /v1/chat/completions — 402 flow
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_chat_returns_402_without_payment() {
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

    // Should return 402 Payment Required
    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // The error message contains the serialized PaymentRequired JSON
    let error_msg = json["error"]["message"].as_str().unwrap();
    let payment_info: serde_json::Value = serde_json::from_str(error_msg).unwrap();

    assert_eq!(payment_info["x402_version"], 2);
    assert!(payment_info["accepts"].is_array());
    assert!(payment_info["cost_breakdown"]["total"].is_string());
    assert_eq!(payment_info["cost_breakdown"]["currency"], "USDC");
    assert_eq!(payment_info["cost_breakdown"]["fee_percent"], 5);
}

#[tokio::test]
async fn test_chat_with_payment_returns_stub() {
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

    // Should return 200 with a stub response (no real provider configured)
    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["object"], "chat.completion");
    assert!(json["choices"].is_array());
    assert!(json["choices"][0]["message"]["content"]
        .as_str()
        .unwrap()
        .contains("STUB"));
}

#[tokio::test]
async fn test_malformed_payment_header_returns_402() {
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
                .header("payment-signature", "fake-payment-for-testing")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // Malformed (non-decodable) payment headers must be rejected — never served free
    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["type"], "invalid_payment");
    assert!(json["error"]["message"]
        .as_str()
        .unwrap()
        .contains("could not be decoded"));
}

// ---------------------------------------------------------------------------
// POST /v1/chat/completions — model aliases
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_chat_model_alias_resolution() {
    let app = test_app();

    let body = serde_json::json!({
        "model": "sonnet",
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

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["model"], "anthropic/claude-sonnet-4.6");
}

// ---------------------------------------------------------------------------
// POST /v1/chat/completions — unknown model
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_chat_unknown_model_returns_404() {
    let app = test_app();

    let body = serde_json::json!({
        "model": "nonexistent/model-v99",
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

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// POST /v1/chat/completions — smart routing profile
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_chat_smart_routing_eco_profile() {
    let app = test_app();

    let body = serde_json::json!({
        "model": "eco",
        "messages": [{"role": "user", "content": "Hi there"}],
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

    // The eco profile should route a simple greeting to deepseek/deepseek-chat
    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["model"], "deepseek/deepseek-chat");
}

// ---------------------------------------------------------------------------
// 404 for unknown routes
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// POST /v1/chat/completions — 402 response contains proper x402 fields
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_402_response_contains_x402_fields() {
    let app = test_app();

    let body = serde_json::json!({
        "model": "openai/gpt-4o",
        "messages": [{"role": "user", "content": "Tell me about Solana."}],
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

    // Parse the embedded PaymentRequired JSON from the error message
    let error_msg = json["error"]["message"].as_str().unwrap();
    let pr: serde_json::Value = serde_json::from_str(error_msg).unwrap();

    // x402 version
    assert_eq!(pr["x402_version"], 2);

    // accepts array with Solana network
    let accepts = pr["accepts"].as_array().unwrap();
    assert!(!accepts.is_empty());
    assert!(accepts[0]["network"]
        .as_str()
        .unwrap()
        .starts_with("solana:"));
    assert_eq!(accepts[0]["scheme"], "exact");
    assert_eq!(
        accepts[0]["asset"],
        "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
    );
    assert!(accepts[0]["amount"].is_string());
    assert!(accepts[0]["max_timeout_seconds"].is_number());

    // cost breakdown fields
    let cb = &pr["cost_breakdown"];
    assert!(cb["provider_cost"].is_string());
    assert!(cb["platform_fee"].is_string());
    assert!(cb["total"].is_string());
    assert_eq!(cb["currency"], "USDC");
    assert_eq!(cb["fee_percent"], 5);

    // total should be > 0
    let total: f64 = cb["total"].as_str().unwrap().parse().unwrap();
    assert!(total > 0.0, "total cost should be positive");

    // platform_fee should be ~5% of provider_cost
    let provider_cost: f64 = cb["provider_cost"].as_str().unwrap().parse().unwrap();
    let platform_fee: f64 = cb["platform_fee"].as_str().unwrap().parse().unwrap();
    let expected_fee = provider_cost * 0.05;
    assert!(
        (platform_fee - expected_fee).abs() < 0.000001,
        "platform fee {platform_fee} should be ~5% of provider cost {provider_cost}"
    );
}

// ---------------------------------------------------------------------------
// POST /v1/chat/completions — streaming request is accepted
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_chat_stream_request_returns_ok() {
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

    // With no provider configured, should still return 200 with a stub response
    // (the stub path doesn't differentiate stream vs non-stream)
    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["object"], "chat.completion");
}

// ---------------------------------------------------------------------------
// Rate limit headers present on responses
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_response_has_rate_limit_headers() {
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

    assert_eq!(response.status(), StatusCode::OK);

    // The rate limiter is configured with default 60 req/min.
    // After one request, x-ratelimit-remaining should be present.
    let remaining = response
        .headers()
        .get("x-ratelimit-remaining")
        .expect("should have x-ratelimit-remaining header");
    let remaining_val: u32 = remaining.to_str().unwrap().parse().unwrap();
    assert_eq!(
        remaining_val, 59,
        "should have 59 remaining after 1 request"
    );
}

// ---------------------------------------------------------------------------
// POST /v1/chat/completions — base64-encoded PaymentPayload header
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_chat_with_base64_payment_header() {
    let app = test_app();

    // Build a valid PaymentPayload and base64-encode it
    let payment_payload = x402::types::PaymentPayload {
        x402_version: 2,
        resource: x402::types::Resource {
            url: "/v1/chat/completions".to_string(),
            method: "POST".to_string(),
        },
        accepted: x402::types::PaymentAccept {
            scheme: "exact".to_string(),
            network: "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp".to_string(),
            amount: "2625".to_string(),
            asset: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            pay_to: "TestRecipientWallet".to_string(),
            max_timeout_seconds: 300,
        },
        payload: x402::types::SolanaPayload {
            transaction: "dGVzdHRyYW5zYWN0aW9u".to_string(), // base64("testtransaction")
        },
    };

    let json_bytes = serde_json::to_vec(&payment_payload).unwrap();
    let encoded = base64::engine::general_purpose::STANDARD.encode(&json_bytes);

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
                .header("payment-signature", encoded)
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // The AlwaysPassVerifier accepts the payment; stub response returned
    // because no real provider API key is configured in test env.
    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["object"], "chat.completion");
    assert!(json["choices"].is_array());
    assert!(json["choices"][0]["message"]["content"]
        .as_str()
        .unwrap()
        .contains("STUB"));
}
