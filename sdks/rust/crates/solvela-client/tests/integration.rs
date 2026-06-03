use solvela_client::{ClientConfig, ClientError, SolvelaClient, Wallet};
use solvela_protocol::{
    ChatChoice, ChatMessage, ChatResponse, CostBreakdown, ModelInfo, PaymentAccept,
    PaymentRequired, Resource, Role, Usage, MAX_TIMEOUT_SECONDS, PLATFORM_FEE_PERCENT,
    SOLANA_NETWORK, USDC_MINT, X402_VERSION,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn test_wallet() -> Wallet {
    let (wallet, _) = Wallet::create();
    wallet
}

fn test_config(gateway_url: &str) -> ClientConfig {
    ClientConfig {
        gateway_url: gateway_url.to_string(),
        rpc_url: "http://127.0.0.1:1".to_string(), // unreachable
        ..ClientConfig::default()
    }
}

fn sample_chat_response() -> ChatResponse {
    ChatResponse {
        id: "chatcmpl-integration".to_string(),
        object: "chat.completion".to_string(),
        created: 1_700_000_000,
        model: "gpt-4o".to_string(),
        choices: vec![ChatChoice {
            index: 0,
            message: ChatMessage {
                role: Role::Assistant,
                content: "Hello from integration test!".into(),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            },
            finish_reason: Some("stop".to_string()),
        }],
        usage: Some(Usage {
            prompt_tokens: 5,
            completion_tokens: 6,
            total_tokens: 11,
        }),
    }
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

fn sample_chat_request() -> solvela_protocol::ChatRequest {
    solvela_protocol::ChatRequest {
        model: "openai/gpt-4o".to_string(),
        messages: vec![ChatMessage {
            role: Role::User,
            content: "Hello".into(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }],
        max_tokens: Some(100),
        temperature: None,
        top_p: None,
        stream: false,
        tools: None,
        tool_choice: None,
    }
}

#[tokio::test]
async fn test_full_free_model_flow() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(sample_chat_response()))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = SolvelaClient::new(test_wallet(), test_config(&mock_server.uri())).unwrap();
    let resp = client.chat(sample_chat_request()).await.unwrap();

    assert_eq!(resp.id, "chatcmpl-integration");
    assert_eq!(resp.choices.len(), 1);
    assert_eq!(
        resp.choices[0].message.content.as_text(),
        "Hello from integration test!"
    );
    assert_eq!(resp.choices[0].finish_reason.as_deref(), Some("stop"));
    assert_eq!(resp.usage.as_ref().unwrap().total_tokens, 11);
}

#[tokio::test]
async fn test_402_flow_fails_without_rpc() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(402).set_body_json(sample_payment_required()))
        .mount(&mock_server)
        .await;

    let client = SolvelaClient::new(
        test_wallet(),
        ClientConfig {
            gateway_url: mock_server.uri(),
            rpc_url: "http://127.0.0.1:1".to_string(),
            timeout: std::time::Duration::from_secs(5),
            ..ClientConfig::default()
        },
    )
    .unwrap();

    let result = client.chat(sample_chat_request()).await;
    assert!(result.is_err());
    assert!(
        matches!(result.unwrap_err(), ClientError::Signing(_)),
        "expected Signing error when RPC is unreachable"
    );
}

#[tokio::test]
async fn test_no_compatible_scheme_error() {
    let mock_server = MockServer::start().await;

    let mut pr = sample_payment_required();
    pr.accepts = vec![PaymentAccept {
        scheme: "unsupported_future_scheme".to_string(),
        network: SOLANA_NETWORK.to_string(),
        amount: "1000".to_string(),
        asset: USDC_MINT.to_string(),
        pay_to: solana_sdk::pubkey::Pubkey::new_unique().to_string(),
        max_timeout_seconds: MAX_TIMEOUT_SECONDS,
        escrow_program_id: None,
    }];

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(402).set_body_json(&pr))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = SolvelaClient::new(test_wallet(), test_config(&mock_server.uri())).unwrap();
    let result = client.chat(sample_chat_request()).await;

    assert!(result.is_err());
    assert!(
        matches!(result.unwrap_err(), ClientError::NoCompatibleScheme),
        "expected NoCompatibleScheme error for unsupported scheme"
    );
}

#[tokio::test]
async fn test_gateway_error_propagation() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(503).set_body_string("service unavailable"))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = SolvelaClient::new(test_wallet(), test_config(&mock_server.uri())).unwrap();
    let result = client.chat(sample_chat_request()).await;

    match result.unwrap_err() {
        ClientError::Gateway { status, message } => {
            assert_eq!(status, 503);
            assert_eq!(message, "service unavailable");
        }
        other => panic!("expected Gateway error, got {other:?}"),
    }
}

#[tokio::test]
async fn test_estimate_cost_returns_cost_breakdown() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(402).set_body_json(sample_payment_required()))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = SolvelaClient::new(test_wallet(), test_config(&mock_server.uri())).unwrap();
    let cost = client.estimate_cost("openai/gpt-4o").await.unwrap();

    assert_eq!(cost.provider_cost, "0.002500");
    assert_eq!(cost.platform_fee, "0.000125");
    assert_eq!(cost.total, "0.002625");
    assert_eq!(cost.currency, "USDC");
    assert_eq!(cost.fee_percent, PLATFORM_FEE_PERCENT);
}

#[tokio::test]
async fn test_models_endpoint() {
    let mock_server = MockServer::start().await;

    let models = vec![
        ModelInfo {
            id: "openai/gpt-4o".to_string(),
            provider: "openai".to_string(),
            display_name: "GPT-4o".to_string(),
            input_cost_per_million: 2.5,
            output_cost_per_million: 10.0,
            context_window: 128_000,
            supports_streaming: true,
            supports_tools: true,
            supports_vision: true,
            reasoning: false,
        },
        ModelInfo {
            id: "anthropic/claude-sonnet-4-20250514".to_string(),
            provider: "anthropic".to_string(),
            display_name: "Claude Sonnet 4".to_string(),
            input_cost_per_million: 3.0,
            output_cost_per_million: 15.0,
            context_window: 200_000,
            supports_streaming: true,
            supports_tools: true,
            supports_vision: true,
            reasoning: false,
        },
    ];

    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&models))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = SolvelaClient::new(test_wallet(), test_config(&mock_server.uri())).unwrap();
    let result = client.models().await.unwrap();

    assert_eq!(result.len(), 2);
    assert_eq!(result[0].id, "openai/gpt-4o");
    assert_eq!(result[0].provider, "openai");
    assert_eq!(result[1].id, "anthropic/claude-sonnet-4-20250514");
    assert_eq!(result[1].display_name, "Claude Sonnet 4");
}

#[tokio::test]
async fn test_wallet_debug_in_client_debug() {
    let wallet = test_wallet();
    let addr = wallet.address();
    let client = SolvelaClient::new(wallet, ClientConfig::default()).unwrap();

    let debug_output = format!("{client:?}");

    assert!(
        debug_output.contains("SolvelaClient"),
        "debug should show struct name"
    );
    assert!(
        debug_output.contains("Wallet("),
        "debug should show Wallet("
    );
    assert!(
        debug_output.contains(&addr),
        "debug should show wallet address"
    );
    assert!(
        !debug_output.contains("keypair"),
        "debug must NOT leak keypair"
    );
}

#[tokio::test]
async fn test_recipient_mismatch_rejected() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(402).set_body_json(sample_payment_required()))
        .expect(1)
        .mount(&mock_server)
        .await;

    let client = SolvelaClient::new(
        test_wallet(),
        ClientConfig {
            gateway_url: mock_server.uri(),
            expected_recipient: Some("DifferentWalletAddress".to_string()),
            ..ClientConfig::default()
        },
    )
    .unwrap();

    let result = client.chat(sample_chat_request()).await;
    assert!(result.is_err());
    assert!(
        matches!(result.unwrap_err(), ClientError::RecipientMismatch { .. }),
        "expected RecipientMismatch error"
    );
}

#[tokio::test]
async fn test_amount_exceeds_max_rejected() {
    let mock_server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(402).set_body_json(sample_payment_required()))
        .expect(1)
        .mount(&mock_server)
        .await;

    // sample_payment_required has amount "2625", so set max below that
    let client = SolvelaClient::new(
        test_wallet(),
        ClientConfig {
            gateway_url: mock_server.uri(),
            max_payment_amount: Some(1000),
            ..ClientConfig::default()
        },
    )
    .unwrap();

    let result = client.chat(sample_chat_request()).await;
    assert!(result.is_err());
    assert!(
        matches!(result.unwrap_err(), ClientError::AmountExceedsMax { .. }),
        "expected AmountExceedsMax error"
    );
}

// Wire-format guards.
//
// Assert that `solvela-protocol` wire types serialize to snake_case JSON keys
// and NOT camelCase, plus that to_string -> from_str -> to_string is identity.
// Guards against silent drift between protocol type definitions and the wire
// format every sibling SDK and the gateway are built against.
//
// Upstream docs disagree with the code about the default casing — see
// solvela-ai/solvela#223. If that issue is resolved by changing the wire
// format to camelCase (rather than the docs), these tests will fail loudly
// and force a coordinated SDK update instead of a silent break.
mod wire_format {
    use super::*;

    fn assert_keys(json: &str, present_snake: &[&str], absent_camel: &[&str]) {
        for k in present_snake {
            let needle = format!("\"{k}\"");
            assert!(
                json.contains(&needle),
                "expected snake_case key {k:?} in JSON: {json}"
            );
        }
        for k in absent_camel {
            let needle = format!("\"{k}\"");
            assert!(
                !json.contains(&needle),
                "unexpected camelCase key {k:?} in JSON: {json}"
            );
        }
    }

    #[test]
    fn payment_required_emits_snake_case() {
        let json = serde_json::to_string(&sample_payment_required()).unwrap();
        assert_keys(
            &json,
            &[
                "x402_version",
                "cost_breakdown",
                "pay_to",
                "max_timeout_seconds",
                "provider_cost",
                "platform_fee",
                "fee_percent",
            ],
            &[
                "x402Version",
                "costBreakdown",
                "payTo",
                "maxTimeoutSeconds",
                "providerCost",
                "platformFee",
                "feePercent",
            ],
        );
    }

    #[test]
    fn payment_required_round_trip_is_identity() {
        let once = serde_json::to_string(&sample_payment_required()).unwrap();
        let back: PaymentRequired = serde_json::from_str(&once).unwrap();
        let twice = serde_json::to_string(&back).unwrap();
        assert_eq!(once, twice);
    }

    // Wire shape is nested as of solvela-protocol 0.2 (#229): top-level keys are
    // `id`/`object`/`provider`/`display_name`/`context_window`/`capabilities`/
    // `pricing`, with capabilities + pricing as nested objects. Internal-only
    // fields (`model_id`, `supports_structured_output`, `supports_batch`,
    // `max_output_tokens`) are not carried on the wire — and as of the
    // ModelRegistration/ModelInfo type split they no longer exist on the wire
    // `ModelInfo` at all (they live on the gateway-internal ModelRegistration).
    // See crates/protocol/src/model.rs.
    #[test]
    fn model_info_emits_snake_case() {
        let m = ModelInfo {
            id: "openai/gpt-4o".to_string(),
            provider: "openai".to_string(),
            display_name: "GPT-4o".to_string(),
            input_cost_per_million: 2.5,
            output_cost_per_million: 10.0,
            context_window: 128_000,
            supports_streaming: true,
            supports_tools: true,
            supports_vision: true,
            reasoning: false,
        };
        let json = serde_json::to_string(&m).unwrap();
        assert_keys(
            &json,
            &[
                "id",
                "object",
                "provider",
                "display_name",
                "context_window",
                "capabilities",
                "pricing",
                "input_per_million",
                "output_per_million",
                "fee_percent",
            ],
            &[
                "modelId",
                "displayName",
                "contextWindow",
                "inputPerMillion",
                "outputPerMillion",
                "feePercent",
            ],
        );
        // Internal-only fields must never reach the wire.
        for absent in [
            "model_id",
            "supports_structured_output",
            "supports_batch",
            "max_output_tokens",
        ] {
            let needle = format!("\"{absent}\"");
            assert!(
                !json.contains(&needle),
                "internal-only key {absent:?} leaked to wire: {json}"
            );
        }
    }

    #[test]
    fn model_info_round_trip_is_identity() {
        let m = ModelInfo {
            id: "openai/gpt-4o".to_string(),
            provider: "openai".to_string(),
            display_name: "GPT-4o".to_string(),
            input_cost_per_million: 2.5,
            output_cost_per_million: 10.0,
            context_window: 128_000,
            supports_streaming: true,
            supports_tools: true,
            supports_vision: true,
            reasoning: false,
        };
        let once = serde_json::to_string(&m).unwrap();
        let back: ModelInfo = serde_json::from_str(&once).unwrap();
        let twice = serde_json::to_string(&back).unwrap();
        assert_eq!(once, twice);
    }
}
