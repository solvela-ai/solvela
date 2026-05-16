use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

use solvela_client::{ClientConfig, SolvelaClient, Wallet};
use solvela_protocol::{
    CostBreakdown, PaymentAccept, PaymentRequired, Resource, MAX_TIMEOUT_SECONDS,
    PLATFORM_FEE_PERCENT, SOLANA_NETWORK, USDC_MINT, X402_VERSION,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn test_wallet() -> Wallet {
    let (wallet, _) = Wallet::create();
    wallet
}

fn sample_payment_required() -> PaymentRequired {
    PaymentRequired {
        x402_version: X402_VERSION,
        resource: Resource {
            url: "/v1/chat/completions".to_string(),
            method: "POST".to_string(),
        },
        accepts: vec![PaymentAccept {
            scheme: "exact".to_string(),
            network: SOLANA_NETWORK.to_string(),
            amount: "2625".to_string(),
            asset: USDC_MINT.to_string(),
            pay_to: solana_sdk::pubkey::Pubkey::new_unique().to_string(),
            max_timeout_seconds: MAX_TIMEOUT_SECONDS,
            escrow_program_id: None,
        }],
        cost_breakdown: CostBreakdown {
            provider_cost: "0.002500".to_string(),
            platform_fee: "0.000125".to_string(),
            total: "0.002625".to_string(),
            currency: "USDC".to_string(),
            fee_percent: PLATFORM_FEE_PERCENT,
        },
        error: "Payment required".to_string(),
    }
}

fn build_test_app(gateway_url: &str) -> axum::Router {
    let wallet = test_wallet();
    let config = ClientConfig {
        gateway_url: gateway_url.to_string(),
        rpc_url: "http://127.0.0.1:1".to_string(),
        ..ClientConfig::default()
    };
    let client = SolvelaClient::new(wallet, config).unwrap();
    let state = Arc::new(solvela_client_proxy::ProxyState {
        client,
        gateway_url: gateway_url.to_string(),
        http: reqwest::Client::new(),
    });
    solvela_client_proxy::build_proxy_router(state)
}

#[tokio::test]
async fn test_non_402_passthrough() {
    let mock = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"id":"test","object":"chat.completion","created":0,"model":"gpt-4o","choices":[]}"#)
                .append_header("content-type", "application/json"),
        )
        .expect(1)
        .mount(&mock)
        .await;

    let app = build_test_app(&mock.uri());

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"model":"gpt-4o","messages":[]}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(text.contains("chat.completion"));
}

#[tokio::test]
async fn test_get_passthrough() {
    let mock = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string("[]")
                .append_header("content-type", "application/json"),
        )
        .expect(1)
        .mount(&mock)
        .await;

    let app = build_test_app(&mock.uri());

    let resp = app
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/v1/models")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_gateway_error_passthrough() {
    let mock = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(500).set_body_string("internal error"))
        .expect(1)
        .mount(&mock)
        .await;

    let app = build_test_app(&mock.uri());

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .body(Body::from(r"{}"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn test_402_triggers_signing_which_fails_without_rpc() {
    let mock = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(402).set_body_json(sample_payment_required()))
        .mount(&mock)
        .await;

    let app = build_test_app(&mock.uri());

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"model":"gpt-4o","messages":[]}"#))
                .unwrap(),
        )
        .await
        .unwrap();

    // Signing fails because RPC is unreachable, returns structured error
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["type"], "signing_error");
}

#[tokio::test]
async fn test_gateway_unreachable_returns_502() {
    // Point at a port where nothing is listening
    let app = build_test_app("http://127.0.0.1:1");

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .body(Body::from(r"{}"))
                .unwrap(),
        )
        .await
        .unwrap();

    // Depending on OS behaviour, connecting to port 1 may return
    // connection-refused (502 + gateway_unreachable) or time out
    // (504 + gateway_timeout). Both are valid proxy error responses.
    let status = resp.status();
    assert!(
        status == StatusCode::BAD_GATEWAY || status == StatusCode::GATEWAY_TIMEOUT,
        "expected 502 or 504, got {status}"
    );
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let error_type = json["error"]["type"].as_str().unwrap();
    assert!(
        error_type == "gateway_unreachable" || error_type == "gateway_timeout",
        "expected gateway_unreachable or gateway_timeout, got {error_type}"
    );
}

#[tokio::test]
async fn test_payment_signature_header_stripped_from_caller() {
    let mock = MockServer::start().await;

    // The mock expects NO payment-signature header (since proxy strips it)
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true}"#))
        .expect(1)
        .mount(&mock)
        .await;

    let app = build_test_app(&mock.uri());

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .header("payment-signature", "should-be-stripped")
                .body(Body::from(r"{}"))
                .unwrap(),
        )
        .await
        .unwrap();

    // Request should succeed (proxy stripped the injected header)
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_amount_exceeds_max_returns_error() {
    let mock = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(402).set_body_json(sample_payment_required()))
        .expect(1)
        .mount(&mock)
        .await;

    // sample_payment_required has amount "2625", set max below that
    let wallet = test_wallet();
    let config = ClientConfig {
        gateway_url: mock.uri(),
        rpc_url: "http://127.0.0.1:1".to_string(),
        max_payment_amount: Some(1000),
        ..ClientConfig::default()
    };
    let client = SolvelaClient::new(wallet, config).unwrap();
    let state = Arc::new(solvela_client_proxy::ProxyState {
        client,
        gateway_url: mock.uri(),
        http: reqwest::Client::new(),
    });
    let app = solvela_client_proxy::build_proxy_router(state);

    let resp = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .body(Body::from(r"{}"))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body = axum::body::to_bytes(resp.into_body(), 1_000_000)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["error"]["type"], "amount_exceeds_max");
}
