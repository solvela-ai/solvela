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
use rcr_common::services::ServiceRegistry;
use router::models::ModelRegistry;
use x402::traits::{Error as X402Error, PaymentVerifier};
use x402::types::{PaymentPayload, SettlementResult, VerificationResult, SOLANA_NETWORK};

// ---------------------------------------------------------------------------
// Mock payment verifier for integration tests
// ---------------------------------------------------------------------------

/// A mock verifier that accepts all structurally-valid payment payloads (scheme="exact").
/// Used so integration tests can exercise the full request path without
/// a live Solana RPC connection.
struct AlwaysPassVerifier;

#[async_trait::async_trait]
impl PaymentVerifier for AlwaysPassVerifier {
    fn network(&self) -> &str {
        SOLANA_NETWORK
    }

    fn scheme(&self) -> &str {
        "exact"
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
            verified_amount: None,
        })
    }
}

/// A mock verifier for the escrow scheme.
struct AlwaysPassEscrowVerifier;

#[async_trait::async_trait]
impl PaymentVerifier for AlwaysPassEscrowVerifier {
    fn network(&self) -> &str {
        SOLANA_NETWORK
    }

    fn scheme(&self) -> &str {
        "escrow"
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
            tx_signature: Some("MockEscrowSettledTxSig123".to_string()),
            network: SOLANA_NETWORK.to_string(),
            error: None,
            verified_amount: Some(2625),
        })
    }
}

const TEST_SERVICES_TOML: &str = r#"
[services.llm-gateway]
name = "LLM Intelligence"
endpoint = "/v1/chat/completions"
category = "intelligence"
x402_enabled = true
internal = true
description = "OpenAI-compatible LLM inference"
pricing_label = "per-token (see /pricing)"

[services.web-search]
name = "Web Search"
endpoint = "https://search.example.com/v1/query"
category = "search"
x402_enabled = true
internal = false
pricing_label = "$0.005/query"
"#;

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
    let service_registry = ServiceRegistry::from_toml(TEST_SERVICES_TOML).unwrap();

    // Use the always-pass mock verifier so tests exercise the full request path
    let facilitator = x402::facilitator::Facilitator::new(vec![Arc::new(AlwaysPassVerifier)]);

    let state = Arc::new(AppState {
        config: AppConfig::default(),
        model_registry,
        service_registry,
        providers: ProviderRegistry::from_env(), // No keys set in test env
        facilitator,
        usage: gateway::usage::UsageTracker::noop(),
        cache: None, // No Redis in tests (replay check is skipped when cache=None)
        provider_health: ProviderHealthTracker::new(CircuitBreakerConfig::default()),
        escrow_claimer: None,
    });
    build_router(state)
}

/// Build a test app with escrow support enabled.
fn test_app_with_escrow() -> axum::Router {
    let model_registry = ModelRegistry::from_toml(TEST_MODELS_TOML).unwrap();
    let service_registry = ServiceRegistry::from_toml(TEST_SERVICES_TOML).unwrap();

    // Include both exact and escrow verifiers
    let facilitator = x402::facilitator::Facilitator::new(vec![
        Arc::new(AlwaysPassVerifier),
        Arc::new(AlwaysPassEscrowVerifier),
    ]);

    let mut config = AppConfig::default();
    config.solana.escrow_program_id =
        Some("GTs7ik3NbW3xwSXq33jyVRGgmshNEyW1h9rxDNATiFLy".to_string());

    // Create a dummy claimer — won't actually submit claims in tests
    // We need a valid 64-byte key. Use a test keypair.
    let test_keypair = {
        use ed25519_dalek::SigningKey;
        let sk = SigningKey::from_bytes(&[1u8; 32]);
        let mut kp = [0u8; 64];
        kp[..32].copy_from_slice(&[1u8; 32]);
        kp[32..].copy_from_slice(sk.verifying_key().as_bytes());
        bs58::encode(&kp).into_string()
    };

    let escrow_claimer = x402::escrow::EscrowClaimer::new(
        "https://api.devnet.solana.com".to_string(),
        &test_keypair,
        "GTs7ik3NbW3xwSXq33jyVRGgmshNEyW1h9rxDNATiFLy",
        "11111111111111111111111111111111",
        "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
    )
    .expect("test claimer must be valid");

    let state = Arc::new(AppState {
        config,
        model_registry,
        service_registry,
        providers: ProviderRegistry::from_env(),
        facilitator,
        usage: gateway::usage::UsageTracker::noop(),
        cache: None,
        provider_health: ProviderHealthTracker::new(CircuitBreakerConfig::default()),
        escrow_claimer: Some(Arc::new(escrow_claimer)),
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
            escrow_program_id: None,
        },
        payload: x402::types::PayloadData::Direct(x402::types::SolanaPayload {
            transaction: base64::engine::general_purpose::STANDARD.encode(b"mock_signed_tx_bytes"),
        }),
    };
    let json = serde_json::to_vec(&payload).unwrap();
    base64::engine::general_purpose::STANDARD.encode(&json)
}

/// Build a valid escrow PaymentPayload header.
fn valid_escrow_payment_header(resource_url: &str) -> String {
    let payload = x402::types::PaymentPayload {
        x402_version: 2,
        resource: x402::types::Resource {
            url: resource_url.to_string(),
            method: "POST".to_string(),
        },
        accepted: x402::types::PaymentAccept {
            scheme: "escrow".to_string(),
            network: SOLANA_NETWORK.to_string(),
            amount: "2625".to_string(),
            asset: x402::types::USDC_MINT.to_string(),
            pay_to: "GatewayRecipientWallet111111111111111111111111".to_string(),
            max_timeout_seconds: 300,
            escrow_program_id: Some("GTs7ik3NbW3xwSXq33jyVRGgmshNEyW1h9rxDNATiFLy".to_string()),
        },
        payload: x402::types::PayloadData::Escrow(x402::types::EscrowPayload {
            deposit_tx: base64::engine::general_purpose::STANDARD.encode(b"mock_deposit_tx_bytes"),
            service_id: base64::engine::general_purpose::STANDARD.encode([0u8; 32]),
            agent_pubkey: "11111111111111111111111111111111".to_string(),
        }),
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
            escrow_program_id: None,
        },
        payload: x402::types::PayloadData::Direct(x402::types::SolanaPayload {
            transaction: "dGVzdHRyYW5zYWN0aW9u".to_string(), // base64("testtransaction")
        }),
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

// ---------------------------------------------------------------------------
// POST /v1/images/generations — scaffold (501 until provider added)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// GET /pricing
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// GET /v1/services  (Phase 6 — x402 Service Marketplace)
// ---------------------------------------------------------------------------

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
    // TEST_SERVICES_TOML has 2 services
    assert_eq!(data.len(), 2);
    assert_eq!(json["total"], 2);
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

    // Only web-search is external in TEST_SERVICES_TOML
    assert_eq!(data.len(), 1);
    assert_eq!(data[0]["id"], "web-search");
    assert_eq!(data[0]["internal"], false);
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

// ---------------------------------------------------------------------------
// Escrow integration tests  (Phase 4.2)
// ---------------------------------------------------------------------------

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
                    valid_escrow_payment_header("/v1/chat/completions"),
                )
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // Should return 200 with a stub response (escrow verifier passes)
    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["object"], "chat.completion");
    assert!(json["choices"].is_array());
}

#[tokio::test]
async fn test_escrow_scheme_dispatches_to_escrow_verifier() {
    // Build a facilitator with both verifiers and verify routing
    let exact_verifier = Arc::new(AlwaysPassVerifier);
    let escrow_verifier = Arc::new(AlwaysPassEscrowVerifier);

    let facilitator = x402::facilitator::Facilitator::new(vec![exact_verifier, escrow_verifier]);

    // Build an escrow payload
    let payload = x402::types::PaymentPayload {
        x402_version: 2,
        resource: x402::types::Resource {
            url: "/v1/chat/completions".to_string(),
            method: "POST".to_string(),
        },
        accepted: x402::types::PaymentAccept {
            scheme: "escrow".to_string(),
            network: SOLANA_NETWORK.to_string(),
            amount: "2625".to_string(),
            asset: x402::types::USDC_MINT.to_string(),
            pay_to: "TestRecipient".to_string(),
            max_timeout_seconds: 300,
            escrow_program_id: Some("GTs7ik3NbW3xwSXq33jyVRGgmshNEyW1h9rxDNATiFLy".to_string()),
        },
        payload: x402::types::PayloadData::Escrow(x402::types::EscrowPayload {
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
