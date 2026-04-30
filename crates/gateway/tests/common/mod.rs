#![allow(dead_code)]

//! Integration tests for the Solvela gateway.
//!
//! These tests spin up the Axum app in-process using `tower::ServiceExt`
//! and exercise the HTTP endpoints without needing a running server.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use futures::stream;
use http_body_util::BodyExt;
use tokio::sync::RwLock;
use tower::ServiceExt;

use gateway::config::AppConfig;
use gateway::middleware::rate_limit::{RateLimitConfig, RateLimiter};
use gateway::providers::health::{CircuitBreakerConfig, ProviderHealthTracker};
use gateway::providers::{ChatStream, LLMProvider, ProviderRegistry};
use gateway::services::ServiceRegistry;
use gateway::{build_router, AppState};
use solvela_protocol::{
    ChatChoice, ChatChunk, ChatChunkChoice, ChatDelta, ChatMessage, ChatResponse, ModelInfo, Role,
    Usage,
};
use solvela_router::models::ModelRegistry;
use solvela_x402::traits::{Error as X402Error, PaymentVerifier};
use solvela_x402::types::{
    EscrowPayload, PayloadData, PaymentAccept, PaymentPayload, Resource, SettlementResult,
    SolanaPayload, VerificationResult, SOLANA_NETWORK, USDC_MINT,
};

// ---------------------------------------------------------------------------
// Test constants
// ---------------------------------------------------------------------------

/// Recipient wallet used across all integration test AppState and payment headers.
pub const TEST_RECIPIENT_WALLET: &str = "GatewayRecipientWallet111111111111111111111111";

/// Large payment amount (in atomic USDC) that exceeds any test model cost estimate.
pub const TEST_PAYMENT_AMOUNT: &str = "1000000";

/// Admin token for escrow health endpoint tests.
pub const TEST_ADMIN_TOKEN: &str = "test-admin-token-for-integration-tests";

/// Returns a shared `PrometheusHandle` for all integration tests.
///
/// The global `metrics` recorder can only be installed once per process, so
/// we use `OnceLock` to lazily install it on the first call and return the
/// same handle for every subsequent test.
pub fn test_prometheus_handle() -> metrics_exporter_prometheus::PrometheusHandle {
    use std::sync::OnceLock;
    static HANDLE: OnceLock<metrics_exporter_prometheus::PrometheusHandle> = OnceLock::new();
    HANDLE
        .get_or_init(|| {
            metrics_exporter_prometheus::PrometheusBuilder::new()
                .install_recorder()
                .expect("failed to install test Prometheus recorder")
        })
        .clone()
}

// ---------------------------------------------------------------------------
// Mock payment verifier for integration tests
// ---------------------------------------------------------------------------

/// A mock verifier that accepts all structurally-valid payment payloads (scheme="exact").
/// Used so integration tests can exercise the full request path without
/// a live Solana RPC connection.
pub struct AlwaysPassVerifier;

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
pub struct AlwaysPassEscrowVerifier;

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

pub const TEST_SERVICES_TOML: &str = r#"
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
price_per_request_usdc = 0.005

[services.legacy-api]
name = "Legacy API"
endpoint = "https://legacy.example.com/v1/data"
category = "data"
x402_enabled = false
internal = false
pricing_label = "$0.01/query"
price_per_request_usdc = 0.01
"#;

pub const TEST_MODELS_TOML: &str = r#"
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
model_id = "claude-sonnet-4-20250514"
display_name = "Claude Sonnet 4"
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
pub fn test_app() -> axum::Router {
    let (router, _state) = test_app_with_state();
    router
}

/// Build a test app and return both the router and shared state.
///
/// Useful when tests need to interact with `AppState` directly (e.g.,
/// recording failures on the `ProviderHealthTracker`).
pub fn test_app_with_state() -> (axum::Router, Arc<AppState>) {
    let model_registry = ModelRegistry::from_toml(TEST_MODELS_TOML).unwrap();
    let service_registry = ServiceRegistry::from_toml(TEST_SERVICES_TOML).unwrap();

    // Use the always-pass mock verifier so tests exercise the full request path
    let facilitator =
        solvela_x402::facilitator::Facilitator::new(vec![Arc::new(AlwaysPassVerifier)]);

    let mut config = AppConfig::default();
    config.solana.recipient_wallet = TEST_RECIPIENT_WALLET.to_string();

    let state = Arc::new(AppState {
        config,
        model_registry,
        service_registry: RwLock::new(service_registry),
        providers: ProviderRegistry::from_env(reqwest::Client::new()), // No keys set in test env
        facilitator,
        usage: gateway::usage::UsageTracker::noop(),
        cache: None, // No Redis in tests — replay check uses in-memory LRU fallback
        provider_health: ProviderHealthTracker::new(CircuitBreakerConfig::default()),
        escrow_claimer: None,
        fee_payer_pool: None,
        nonce_pool: None,
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
    let router = build_router(
        Arc::clone(&state),
        RateLimiter::new(RateLimitConfig::default()),
    );
    (router, state)
}

// ---------------------------------------------------------------------------
// Mock LLM provider for integration tests
// ---------------------------------------------------------------------------

/// A mock LLM provider that returns canned responses for any model.
/// Supports both streaming and non-streaming requests.
pub struct MockProvider {
    provider_name: String,
}

impl MockProvider {
    fn new(name: &str) -> Self {
        Self {
            provider_name: name.to_string(),
        }
    }

    fn mock_response(model: &str) -> ChatResponse {
        ChatResponse {
            id: "mock-chatcmpl-001".to_string(),
            object: "chat.completion".to_string(),
            created: 1_700_000_000,
            model: model.to_string(),
            choices: vec![ChatChoice {
                index: 0,
                message: ChatMessage {
                    role: Role::Assistant,
                    content: "[mock response]".to_string(),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: Some(Usage {
                prompt_tokens: 10,
                completion_tokens: 5,
                total_tokens: 15,
            }),
        }
    }
}

#[async_trait]
impl LLMProvider for MockProvider {
    fn name(&self) -> &str {
        &self.provider_name
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![]
    }

    async fn chat_completion(
        &self,
        req: solvela_protocol::ChatRequest,
    ) -> Result<ChatResponse, Box<dyn std::error::Error + Send + Sync>> {
        Ok(Self::mock_response(&req.model))
    }

    async fn chat_completion_stream(
        &self,
        req: solvela_protocol::ChatRequest,
    ) -> Result<ChatStream, Box<dyn std::error::Error + Send + Sync>> {
        let chunk = ChatChunk {
            id: "mock-chatcmpl-001".to_string(),
            object: "chat.completion.chunk".to_string(),
            created: 1_700_000_000,
            model: req.model.clone(),
            choices: vec![ChatChunkChoice {
                index: 0,
                delta: ChatDelta {
                    role: Some(Role::Assistant),
                    content: Some("[mock stream response]".to_string()),
                    tool_calls: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
        };
        let s = stream::iter(vec![Ok(chunk)]);
        Ok(Pin::from(
            Box::new(s) as Box<dyn futures::Stream<Item = _> + Send>
        ))
    }
}

/// Build a mock `ProviderRegistry` that has providers for all models in TEST_MODELS_TOML.
pub fn mock_provider_registry() -> ProviderRegistry {
    let mut providers: HashMap<String, Arc<dyn LLMProvider>> = HashMap::new();
    providers.insert("openai".to_string(), Arc::new(MockProvider::new("openai")));
    providers.insert(
        "anthropic".to_string(),
        Arc::new(MockProvider::new("anthropic")),
    );
    providers.insert(
        "deepseek".to_string(),
        Arc::new(MockProvider::new("deepseek")),
    );
    ProviderRegistry::from_providers(providers)
}

/// Build a test app with mock providers so paid requests succeed.
pub fn test_app_with_mock_provider() -> axum::Router {
    let (router, _state) = test_app_with_mock_provider_and_state();
    router
}

/// Build a test app with mock providers and return both the router and state.
pub fn test_app_with_mock_provider_and_state() -> (axum::Router, Arc<AppState>) {
    let model_registry = ModelRegistry::from_toml(TEST_MODELS_TOML).unwrap();
    let service_registry = ServiceRegistry::from_toml(TEST_SERVICES_TOML).unwrap();
    let facilitator =
        solvela_x402::facilitator::Facilitator::new(vec![Arc::new(AlwaysPassVerifier)]);

    let mut config = AppConfig::default();
    config.solana.recipient_wallet = TEST_RECIPIENT_WALLET.to_string();

    let state = Arc::new(AppState {
        config,
        model_registry,
        service_registry: RwLock::new(service_registry),
        providers: mock_provider_registry(),
        facilitator,
        usage: gateway::usage::UsageTracker::noop(),
        cache: None,
        provider_health: ProviderHealthTracker::new(CircuitBreakerConfig::default()),
        escrow_claimer: None,
        fee_payer_pool: None,
        nonce_pool: None,
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
    let router = build_router(
        Arc::clone(&state),
        RateLimiter::new(RateLimitConfig::default()),
    );
    (router, state)
}

/// Build a test app with mock providers and escrow support enabled.
pub fn test_app_with_mock_provider_and_escrow() -> axum::Router {
    let model_registry = ModelRegistry::from_toml(TEST_MODELS_TOML).unwrap();
    let service_registry = ServiceRegistry::from_toml(TEST_SERVICES_TOML).unwrap();

    let facilitator = solvela_x402::facilitator::Facilitator::new(vec![
        Arc::new(AlwaysPassVerifier),
        Arc::new(AlwaysPassEscrowVerifier),
    ]);

    let mut config = AppConfig::default();
    config.solana.recipient_wallet = TEST_RECIPIENT_WALLET.to_string();
    config.solana.escrow_program_id =
        Some("9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU".to_string());

    let test_keypair = {
        use ed25519_dalek::SigningKey;
        let sk = SigningKey::from_bytes(&[1u8; 32]);
        let mut kp = [0u8; 64];
        kp[..32].copy_from_slice(&[1u8; 32]);
        kp[32..].copy_from_slice(sk.verifying_key().as_bytes());
        bs58::encode(&kp).into_string()
    };
    let test_fee_payer_pool = Arc::new(
        solvela_x402::fee_payer::FeePayerPool::from_keys(&[test_keypair])
            .expect("test pool must load"),
    );

    let escrow_claimer = solvela_x402::escrow::EscrowClaimer::new(
        "https://api.devnet.solana.com".to_string(),
        test_fee_payer_pool.clone(),
        "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU",
        "11111111111111111111111111111111",
        "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
        None,
    )
    .expect("test claimer must be valid");

    let state = Arc::new(AppState {
        config,
        model_registry,
        service_registry: RwLock::new(service_registry),
        providers: mock_provider_registry(),
        facilitator,
        usage: gateway::usage::UsageTracker::noop(),
        cache: None,
        provider_health: ProviderHealthTracker::new(CircuitBreakerConfig::default()),
        escrow_claimer: Some(Arc::new(escrow_claimer)),
        fee_payer_pool: Some(test_fee_payer_pool),
        nonce_pool: None,
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
    build_router(state, RateLimiter::new(RateLimitConfig::default()))
}

/// Build a test app with escrow support enabled.
pub fn test_app_with_escrow() -> axum::Router {
    let model_registry = ModelRegistry::from_toml(TEST_MODELS_TOML).unwrap();
    let service_registry = ServiceRegistry::from_toml(TEST_SERVICES_TOML).unwrap();

    // Include both exact and escrow verifiers
    let facilitator = solvela_x402::facilitator::Facilitator::new(vec![
        Arc::new(AlwaysPassVerifier),
        Arc::new(AlwaysPassEscrowVerifier),
    ]);

    let mut config = AppConfig::default();
    config.solana.recipient_wallet = TEST_RECIPIENT_WALLET.to_string();
    config.solana.escrow_program_id =
        Some("9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU".to_string());

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
    let test_fee_payer_pool = Arc::new(
        solvela_x402::fee_payer::FeePayerPool::from_keys(&[test_keypair])
            .expect("test pool must load"),
    );

    let escrow_claimer = solvela_x402::escrow::EscrowClaimer::new(
        "https://api.devnet.solana.com".to_string(),
        test_fee_payer_pool.clone(),
        "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU",
        "11111111111111111111111111111111",
        "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
        None,
    )
    .expect("test claimer must be valid");

    let state = Arc::new(AppState {
        config,
        model_registry,
        service_registry: RwLock::new(service_registry),
        providers: ProviderRegistry::from_env(reqwest::Client::new()),
        facilitator,
        usage: gateway::usage::UsageTracker::noop(),
        cache: None,
        provider_health: ProviderHealthTracker::new(CircuitBreakerConfig::default()),
        escrow_claimer: Some(Arc::new(escrow_claimer)),
        fee_payer_pool: Some(test_fee_payer_pool),
        nonce_pool: None,
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
    build_router(state, RateLimiter::new(RateLimitConfig::default()))
}

/// Build a minimal valid PaymentPayload base64-encoded header for a given model path.
pub fn valid_payment_header(resource_url: &str) -> String {
    let payload = PaymentPayload {
        x402_version: 2,
        resource: Resource {
            url: resource_url.to_string(),
            method: "POST".to_string(),
        },
        accepted: PaymentAccept {
            scheme: "exact".to_string(),
            network: SOLANA_NETWORK.to_string(),
            amount: TEST_PAYMENT_AMOUNT.to_string(),
            asset: USDC_MINT.to_string(),
            pay_to: TEST_RECIPIENT_WALLET.to_string(),
            max_timeout_seconds: 300,
            escrow_program_id: None,
        },
        payload: PayloadData::Direct(SolanaPayload {
            transaction: base64::engine::general_purpose::STANDARD.encode(b"mock_signed_tx_bytes"),
        }),
    };
    let json = serde_json::to_vec(&payload).unwrap();
    base64::engine::general_purpose::STANDARD.encode(&json)
}

/// Build a valid escrow PaymentPayload header.
pub fn valid_escrow_payment_header(resource_url: &str) -> String {
    let payload = PaymentPayload {
        x402_version: 2,
        resource: Resource {
            url: resource_url.to_string(),
            method: "POST".to_string(),
        },
        accepted: PaymentAccept {
            scheme: "escrow".to_string(),
            network: SOLANA_NETWORK.to_string(),
            amount: TEST_PAYMENT_AMOUNT.to_string(),
            asset: USDC_MINT.to_string(),
            pay_to: TEST_RECIPIENT_WALLET.to_string(),
            max_timeout_seconds: 300,
            escrow_program_id: Some("9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU".to_string()),
        },
        payload: PayloadData::Escrow(EscrowPayload {
            deposit_tx: base64::engine::general_purpose::STANDARD.encode(b"mock_deposit_tx_bytes"),
            service_id: base64::engine::general_purpose::STANDARD.encode([0u8; 32]),
            agent_pubkey: "11111111111111111111111111111111".to_string(),
        }),
    };
    let json = serde_json::to_vec(&payload).unwrap();
    base64::engine::general_purpose::STANDARD.encode(&json)
}

/// Build a mismatched PaymentPayload header: scheme="exact" but with escrow payload data.
pub fn mismatched_exact_scheme_escrow_payload_header(resource_url: &str) -> String {
    let payload = PaymentPayload {
        x402_version: 2,
        resource: Resource {
            url: resource_url.to_string(),
            method: "POST".to_string(),
        },
        accepted: PaymentAccept {
            scheme: "exact".to_string(),
            network: SOLANA_NETWORK.to_string(),
            amount: TEST_PAYMENT_AMOUNT.to_string(),
            asset: USDC_MINT.to_string(),
            pay_to: TEST_RECIPIENT_WALLET.to_string(),
            max_timeout_seconds: 300,
            escrow_program_id: None,
        },
        payload: PayloadData::Escrow(EscrowPayload {
            deposit_tx: base64::engine::general_purpose::STANDARD.encode(b"mock_deposit_tx_bytes"),
            service_id: base64::engine::general_purpose::STANDARD.encode([0u8; 32]),
            agent_pubkey: "11111111111111111111111111111111".to_string(),
        }),
    };
    let json = serde_json::to_vec(&payload).unwrap();
    base64::engine::general_purpose::STANDARD.encode(&json)
}

/// Build a mismatched PaymentPayload header: scheme="escrow" but with direct payload data.
pub fn mismatched_escrow_scheme_direct_payload_header(resource_url: &str) -> String {
    let payload = PaymentPayload {
        x402_version: 2,
        resource: Resource {
            url: resource_url.to_string(),
            method: "POST".to_string(),
        },
        accepted: PaymentAccept {
            scheme: "escrow".to_string(),
            network: SOLANA_NETWORK.to_string(),
            amount: TEST_PAYMENT_AMOUNT.to_string(),
            asset: USDC_MINT.to_string(),
            pay_to: TEST_RECIPIENT_WALLET.to_string(),
            max_timeout_seconds: 300,
            escrow_program_id: Some("9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU".to_string()),
        },
        payload: PayloadData::Direct(SolanaPayload {
            transaction: base64::engine::general_purpose::STANDARD.encode(b"mock_signed_tx_bytes"),
        }),
    };
    let json = serde_json::to_vec(&payload).unwrap();
    base64::engine::general_purpose::STANDARD.encode(&json)
}
