//! Integration tests — payments_misc.
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

/// Build a test app with a nonce pool configured (no RPC — pool only).
fn test_app_with_nonce_pool() -> axum::Router {
    use solvela_x402::nonce_pool::{NonceEntry, NoncePool};

    let model_registry = ModelRegistry::from_toml(TEST_MODELS_TOML).unwrap();
    let service_registry = ServiceRegistry::from_toml(TEST_SERVICES_TOML).unwrap();
    let facilitator =
        solvela_x402::facilitator::Facilitator::new(vec![Arc::new(AlwaysPassVerifier)]);

    // Create a pool with a well-known test pubkey (system program = 32 zero bytes in base58)
    let pool = NoncePool::from_entries(vec![NonceEntry {
        nonce_account: "11111111111111111111111111111111".to_string(),
        authority: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
    }])
    .expect("test pool must be valid");

    let state = Arc::new(gateway::AppState {
        config: AppConfig::default(),
        model_registry,
        service_registry: RwLock::new(service_registry),
        providers: ProviderRegistry::from_env(reqwest::Client::new()),
        facilitator,
        usage: gateway::usage::UsageTracker::noop(),
        cache: None,
        provider_health: ProviderHealthTracker::new(CircuitBreakerConfig::default()),
        escrow_claimer: None,
        fee_payer_pool: None,
        nonce_pool: Some(Arc::new(pool)),
        db_pool: None,
        session_secret: b"test-secret".to_vec(),
        http_client: reqwest::Client::new(),
        replay_set: AppState::new_replay_set(),
        slot_cache: gateway::routes::escrow::new_slot_cache(),
        escrow_metrics: None,
        admin_token: Some(TEST_ADMIN_TOKEN.to_string()),
        prometheus_handle: Some(test_prometheus_handle()),
        dev_bypass_payment: false,
        dedup_store: gateway::cache::request_dedup::InMemoryDedupStore::new(),
    });
    gateway::build_router(state, RateLimiter::new(RateLimitConfig::default()))
}

#[tokio::test]
async fn test_response_has_rate_limit_headers() {
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

    // GHSA-6ggq-cvwx-4f67: rate-limit is keyed on the *payer wallet* extracted
    // from the signed transaction (not on the client-supplied `pay_to`). This
    // test fixture uses a mock byte string for `transaction` that doesn't decode
    // as a real VersionedTransaction, so `extract_payer_wallet` returns "unknown"
    // and the request falls through to the unknown-clients bucket. That bucket
    // is intentionally smaller (`unknown_max_requests = 10`) so unidentified
    // traffic shares one stricter bucket. After 1 request: 9 remaining.
    //
    // The behavior with a properly signed tx (per-client 60-bucket → 59 remaining)
    // is covered by `test_response_has_rate_limit_headers_with_escrow_payer` below.
    let remaining = response
        .headers()
        .get("x-ratelimit-remaining")
        .expect("should have x-ratelimit-remaining header");
    let remaining_val: u32 = remaining.to_str().unwrap().parse().unwrap();
    assert_eq!(
        remaining_val, 9,
        "fake-tx falls through to unknown-bucket (max=10); 9 remaining after 1 request"
    );
}

#[tokio::test]
async fn test_supported_endpoint() {
    let app = test_app();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/supported")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    assert_eq!(json["gateway"], "Solvela");
    assert!(json["pricing_url"].is_string());

    let kinds = json["kinds"].as_array().unwrap();
    assert!(!kinds.is_empty());
    assert_eq!(kinds[0]["scheme"], "exact");
    assert!(kinds[0]["network"].as_str().unwrap().starts_with("solana:"));
    assert_eq!(
        kinds[0]["asset"],
        "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
    );
}

#[tokio::test]
async fn test_chat_wrong_resource_url_rejected() {
    let app = test_app();

    let body = serde_json::json!({
        "model": "openai/gpt-4o",
        "messages": [{"role": "user", "content": "Hello"}],
    });

    // Payment header targets a different resource path
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .header(
                    "payment-signature",
                    valid_payment_header("/v1/images/generations"), // Wrong resource!
                )
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // Should be rejected as invalid payment (resource mismatch)
    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["type"], "payment_required"); // code stayed the same ("invalid_payment"); type changed by error envelope normalization
    assert!(json["error"]["message"]
        .as_str()
        .unwrap()
        .contains("does not match"));
}

/// Test 6: no nonce pool configured → 404 with error message.
#[tokio::test]
async fn test_nonce_endpoint_no_pool() {
    let app = test_app(); // nonce_pool: None

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/nonce")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        json["error"]
            .as_str()
            .unwrap()
            .contains("no nonce accounts configured"),
        "error message should say no nonce accounts configured, got: {}",
        json["error"]
    );
}

/// Test 7: nonce pool configured → 200 with nonce account details.
/// Note: we cannot make a real RPC call in tests, so we verify the 200 path
/// indirectly by checking that the pool entry is returned and only the RPC
/// call itself is the external dependency. We test the 200 body shape here
/// and the 503 error path when RPC fails.
#[tokio::test]
async fn test_nonce_endpoint_with_pool_returns_correct_fields_or_503() {
    let app = test_app_with_nonce_pool();

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/nonce")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    // Either 200 (if devnet RPC is reachable and account exists) or 503 (RPC failed)
    // In CI without network access, we'll get 503. Either way, we must NOT get 404.
    assert_ne!(
        response.status(),
        StatusCode::NOT_FOUND,
        "with pool configured, must not return 404"
    );

    let status = response.status();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();

    if status == StatusCode::OK {
        // 200 path: verify all required fields are present
        assert!(json["nonce_account"].is_string(), "must have nonce_account");
        assert!(json["authority"].is_string(), "must have authority");
        assert!(json["nonce_value"].is_string(), "must have nonce_value");
        // rpc_url is intentionally NOT in the response (H-2: may contain embedded API key)
        assert!(
            json["rpc_url"].is_null(),
            "rpc_url must NOT be present in response (security: may contain API key)"
        );
        assert_eq!(
            json["nonce_account"], "11111111111111111111111111111111",
            "nonce_account must match pool entry"
        );
        assert_eq!(
            json["authority"], "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
            "authority must match pool entry"
        );
    } else {
        // 503 path (no live RPC in CI): verify error field is present
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(json["error"].is_string(), "503 must include error field");
    }
}
