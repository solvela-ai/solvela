//! OpenAPI contract drift guard.
//!
//! `docs/openapi.json` is the canonical machine-readable contract for the
//! public API (and the pay.sh catalog sidecar), but nothing else in CI
//! validates it against what the gateway actually serves — the repo's
//! documented failure mode is "declared-but-unenforced quality gates"
//! (see `.claude/plan/solvela.md` doc-audit notes). These tests pin live
//! responses — driven in-process via `tower::ServiceExt::oneshot`, no live
//! server (CLAUDE.md rule #10) — against the spec's JSON Schemas, so either
//! side drifting fails CI with a diagnosable error.
//!
//! OpenAPI 3.1 schemas are JSON Schema draft 2020-12. To validate against a
//! component schema whose internal `$ref`s point at
//! `#/components/schemas/...`, each validator is built from the FULL spec
//! document with a top-level `"$ref"` injected — internal references then
//! resolve against the root document.
//!
//! The 402 challenge is the money-path wire contract: these tests encode the
//! CURRENT contract. If a live response genuinely fails the spec, the fix is
//! a reviewed change to whichever side is wrong — never a quiet edit of the
//! expected schema to match output.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use http_body_util::BodyExt;
use serde_json::{json, Value};
use tokio::sync::RwLock;
use tower::ServiceExt;

use gateway::config::AppConfig;
use gateway::middleware::rate_limit::{
    FreeTierGlobalCap, RateLimitConfig, RateLimiter, FREE_TIER_GLOBAL_RPM_DEFAULT,
};
use gateway::providers::health::{CircuitBreakerConfig, ProviderHealthTracker};
use gateway::providers::{ChatStream, LLMProvider, ProviderRegistry};
use gateway::services::ServiceRegistry;
use gateway::{build_router, AppState};
use solvela_protocol::{ChatChoice, ChatMessage, ChatResponse, ModelRegistration, Role, Usage};
use solvela_router::models::ModelRegistry;
use solvela_x402::traits::{Error as X402Error, PaymentVerifier};
use solvela_x402::types::{
    PayloadData, PaymentAccept, PaymentPayload, Resource, SettlementResult, SolanaPayload,
    VerificationResult, SOLANA_NETWORK, USDC_MINT,
};

/// The canonical spec, pinned at compile time — same pattern as the
/// `models.toml` guards in `crates/gateway/src/providers/fallback.rs`
/// (`every_fallback_chain_id_is_registered`).
const OPENAPI_JSON: &str = include_str!("../../../docs/openapi.json");

/// Recipient wallet used for the test AppState and payment headers.
const TEST_RECIPIENT_WALLET: &str = "GatewayRecipientWallet111111111111111111111111";

/// Payment amount (atomic USDC) exceeding any test model cost estimate.
const TEST_PAYMENT_AMOUNT: &str = "1000000";

/// One PAID model so the no-payment request exercises the 402 challenge path.
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
"#;

// ---------------------------------------------------------------------------
// Spec / schema helpers
// ---------------------------------------------------------------------------

/// Parse the spec, failing loudly on a malformed `docs/openapi.json` edit.
fn spec_value() -> Value {
    serde_json::from_str(OPENAPI_JSON).expect("docs/openapi.json must parse as JSON")
}

/// Compile a draft 2020-12 validator from `root`, failing the test (not
/// silently passing) if the schema itself does not compile.
fn validator_for(root: &Value, what: &str) -> jsonschema::Validator {
    jsonschema::draft202012::options()
        .build(root)
        .unwrap_or_else(|e| panic!("{what}: schema failed to compile: {e}"))
}

/// Build a validator for `#/components/schemas/<name>` by injecting a
/// top-level `$ref` into the full spec document, so internal `$ref`s in the
/// component resolve against the root document.
fn component_validator(name: &str) -> jsonschema::Validator {
    let mut root = spec_value();
    assert!(
        root.pointer(&format!("/components/schemas/{name}"))
            .is_some(),
        "docs/openapi.json has no #/components/schemas/{name}"
    );
    root.as_object_mut()
        .expect("spec root must be a JSON object")
        .insert(
            "$ref".to_string(),
            json!(format!("#/components/schemas/{name}")),
        );
    validator_for(&root, name)
}

/// Assert `instance` conforms to `validator`, printing the validation errors
/// AND the actual response body so drift is diagnosable from CI logs alone.
fn assert_conforms(validator: &jsonschema::Validator, instance: &Value, what: &str) {
    let errors: Vec<String> = validator
        .iter_errors(instance)
        .map(|err| format!("  - {err} (instance path: `{}`)", err.instance_path()))
        .collect();
    assert!(
        errors.is_empty(),
        "{what} drifted from docs/openapi.json.\nvalidation errors:\n{}\nactual response body:\n{}",
        errors.join("\n"),
        serde_json::to_string_pretty(instance).unwrap_or_else(|_| instance.to_string()),
    );
}

/// Collect a response body and parse it as JSON, panicking with the raw bytes
/// on a non-JSON body.
async fn response_json(response: axum::response::Response) -> Value {
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("collect response body")
        .to_bytes();
    serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        panic!(
            "response body is not JSON ({e}): {}",
            String::from_utf8_lossy(&bytes)
        )
    })
}

// ---------------------------------------------------------------------------
// In-process app fixtures (mirrors `tests/integration.rs` — full router via
// `build_router`, mock verifier/provider, no network)
// ---------------------------------------------------------------------------

/// A mock verifier that accepts all structurally-valid `exact` payloads, so
/// the paid 200 path can be exercised without a live Solana RPC connection.
struct AlwaysPassVerifier;

#[async_trait]
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
            failure_kind: None,
        })
    }
}

/// A mock LLM provider returning a canned non-streaming completion.
struct MockProvider;

#[async_trait]
impl LLMProvider for MockProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn supported_models(&self) -> Vec<ModelRegistration> {
        vec![]
    }

    async fn chat_completion(
        &self,
        req: solvela_protocol::ChatRequest,
    ) -> Result<ChatResponse, Box<dyn std::error::Error + Send + Sync>> {
        Ok(ChatResponse {
            id: "mock-chatcmpl-001".to_string(),
            object: "chat.completion".to_string(),
            created: 1_700_000_000,
            model: req.model,
            choices: vec![ChatChoice {
                index: 0,
                message: ChatMessage {
                    role: Role::Assistant,
                    content: "[mock response]".into(),
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
        })
    }

    async fn chat_completion_stream(
        &self,
        _req: solvela_protocol::ChatRequest,
    ) -> Result<ChatStream, Box<dyn std::error::Error + Send + Sync>> {
        Err("streaming is not exercised by the OpenAPI contract tests".into())
    }
}

/// Build the full router with a caller-supplied provider registry.
fn contract_app(providers: ProviderRegistry) -> axum::Router {
    contract_app_with_db(providers, None)
}

/// [`contract_app`] with a caller-supplied `db_pool`, for routes whose
/// contract depends on storage being configured (the receipts 404).
fn contract_app_with_db(
    providers: ProviderRegistry,
    db_pool: Option<sqlx::PgPool>,
) -> axum::Router {
    let model_registry = ModelRegistry::from_toml(TEST_MODELS_TOML).expect("test models toml");
    let facilitator =
        solvela_x402::facilitator::Facilitator::new(vec![Arc::new(AlwaysPassVerifier)]);

    let mut config = AppConfig::default();
    config.solana.recipient_wallet = TEST_RECIPIENT_WALLET.to_string();

    let state = Arc::new(AppState {
        config,
        model_registry,
        service_registry: RwLock::new(ServiceRegistry::empty()),
        providers,
        facilitator,
        usage: gateway::usage::UsageTracker::noop(),
        cache: None,
        semantic_cache: None,
        provider_health: ProviderHealthTracker::new(CircuitBreakerConfig::default()),
        escrow_claimer: None,
        fee_payer_pool: None,
        nonce_pool: None,
        db_pool,
        faucet: None,
        session_secret: b"test-secret".to_vec(),
        http_client: reqwest::Client::new(),
        replay_set: AppState::new_replay_set(),
        slot_cache: gateway::routes::escrow::new_slot_cache(),
        escrow_metrics: None,
        admin_token: None,
        api_key_hmac_secret: None,
        prometheus_handle: None,
        dev_bypass_payment: false,
        free_rate_limiter: RateLimiter::new(RateLimitConfig::free_default()),
        receipts_rate_limiter: RateLimiter::new(RateLimitConfig::receipts_default()),
        free_global_cap: FreeTierGlobalCap::new(FREE_TIER_GLOBAL_RPM_DEFAULT),
    });
    build_router(state, RateLimiter::new(RateLimitConfig::default()))
}

/// App with NO providers configured — exercises the 402 quote path.
fn quote_app() -> axum::Router {
    contract_app(ProviderRegistry::from_env(reqwest::Client::new()))
}

/// App with a mock provider so a paid request returns a 200 completion.
fn paid_app() -> axum::Router {
    let mut providers: HashMap<String, Arc<dyn LLMProvider>> = HashMap::new();
    providers.insert("openai".to_string(), Arc::new(MockProvider));
    contract_app(ProviderRegistry::from_providers(providers))
}

/// Build a minimal valid `exact` PaymentPayload header (base64 JSON).
fn valid_payment_header(resource_url: &str) -> String {
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
    let json = serde_json::to_vec(&payload).expect("serialize payment payload");
    base64::engine::general_purpose::STANDARD.encode(&json)
}

/// POST a chat completion request, optionally with a `PAYMENT-SIGNATURE`.
async fn post_chat(
    app: axum::Router,
    body: &Value,
    payment_header: Option<String>,
) -> axum::response::Response {
    let mut builder = Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header("content-type", "application/json");
    if let Some(header) = payment_header {
        builder = builder.header("PAYMENT-SIGNATURE", header);
    }
    app.oneshot(
        builder
            .body(Body::from(serde_json::to_vec(body).expect("request body")))
            .expect("build request"),
    )
    .await
    .expect("oneshot")
}

fn chat_body(model: &str) -> Value {
    json!({
        "model": model,
        "messages": [{"role": "user", "content": "What is 2+2?"}],
    })
}

// ---------------------------------------------------------------------------
// Spec self-checks — a malformed spec edit must FAIL, not silently pass
// ---------------------------------------------------------------------------

#[test]
fn spec_parses_and_every_component_schema_compiles() {
    let spec = spec_value();
    let schemas = spec
        .pointer("/components/schemas")
        .and_then(Value::as_object)
        .expect("docs/openapi.json must have components.schemas");
    assert!(!schemas.is_empty(), "components.schemas must not be empty");
    for name in schemas.keys() {
        // `component_validator` panics with the schema name on a compile failure.
        let _ = component_validator(name);
    }

    // The /health 200 schema is inline (not a component) — compile it too.
    let health_schema = spec
        .pointer("/paths/~1health/get/responses/200/content/application~1json/schema")
        .expect("docs/openapi.json must define the inline /health 200 schema");
    let _ = validator_for(health_schema, "inline /health 200 schema");
}

/// Guards against the whole suite becoming vacuous (e.g. a refactor that
/// makes every validator accept anything): the strict `PaymentRequired`
/// validator must reject obviously non-conforming bodies.
#[test]
fn payment_required_validator_rejects_nonconforming_bodies() {
    let validator = component_validator("PaymentRequired");
    assert!(
        !validator.is_valid(&json!({})),
        "PaymentRequired validator accepted an empty object — validation is vacuous"
    );
    assert!(
        !validator.is_valid(&json!({
            "x402_version": "2",
            "resource": {"url": "/v1/chat/completions", "method": "POST"},
            "accepts": [],
            "cost_breakdown": {},
            "error": "Payment required"
        })),
        "PaymentRequired validator accepted a body with wrong types and empty accepts"
    );
}

// ---------------------------------------------------------------------------
// Live-response contract checks
// ---------------------------------------------------------------------------

/// POST /v1/chat/completions on a PAID model with no `PAYMENT-SIGNATURE` →
/// HTTP 402, body conforms to the strict `PaymentRequired` challenge schema.
/// The spec's 402 content is `anyOf [PaymentRequired, Error]`; the challenge
/// form must match the stricter arm — that is the drift signal.
#[tokio::test]
async fn chat_402_challenge_conforms_to_payment_required_schema() {
    let response = post_chat(quote_app(), &chat_body("openai/gpt-4o"), None).await;
    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);

    let body = response_json(response).await;
    assert_conforms(
        &component_validator("PaymentRequired"),
        &body,
        "POST /v1/chat/completions 402 challenge body",
    );
    // The challenge must actually quote at least one scheme (an empty
    // `accepts` would be schema-invalid too via minItems, but assert the
    // money-relevant field explicitly for a clearer failure).
    assert!(
        body["accepts"]
            .as_array()
            .is_some_and(|accepts| !accepts.is_empty()),
        "402 challenge quoted no payment schemes: {body}"
    );
}

/// POST with a PAYMENT-SIGNATURE that cannot be decoded → HTTP 402 with the
/// standard error envelope — the `Error` arm of the spec's 402 `anyOf`.
#[tokio::test]
async fn chat_402_invalid_payment_envelope_conforms_to_error_schema() {
    let response = post_chat(
        quote_app(),
        &chat_body("openai/gpt-4o"),
        Some("!!!not-base64-not-json!!!".to_string()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::PAYMENT_REQUIRED);

    let body = response_json(response).await;
    assert_conforms(
        &component_validator("Error"),
        &body,
        "POST /v1/chat/completions 402 invalid-payment error body",
    );
    assert_eq!(
        body["error"]["type"], "invalid_payment",
        "spec documents `invalid_payment` for an undecodable payment header: {body}"
    );
}

/// GET /v1/models → 200, body conforms to `ModelList`.
#[tokio::test]
async fn models_response_conforms_to_model_list_schema() {
    let response = quote_app()
        .oneshot(
            Request::builder()
                .uri("/v1/models")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("oneshot");
    assert_eq!(response.status(), StatusCode::OK);

    let body = response_json(response).await;
    // An empty `data` array would conform vacuously — require at least one
    // model so the per-item schema is actually exercised.
    assert!(
        body["data"].as_array().is_some_and(|data| !data.is_empty()),
        "GET /v1/models returned no models; per-item schema not exercised: {body}"
    );
    assert_conforms(
        &component_validator("ModelList"),
        &body,
        "GET /v1/models 200 body",
    );
}

/// GET /health → 200, body conforms to the spec's inline response schema
/// (extracted from the parsed spec, not duplicated here).
#[tokio::test]
async fn health_response_conforms_to_inline_schema() {
    let response = quote_app()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("oneshot");
    assert_eq!(response.status(), StatusCode::OK);

    let spec = spec_value();
    let health_schema = spec
        .pointer("/paths/~1health/get/responses/200/content/application~1json/schema")
        .expect("docs/openapi.json must define the inline /health 200 schema");
    let validator = validator_for(health_schema, "inline /health 200 schema");

    let body = response_json(response).await;
    assert_conforms(&validator, &body, "GET /health 200 body");
}

/// A paid request through the full route (mock provider + always-pass
/// verifier) → 200, body conforms to `ChatCompletionResponse`.
#[tokio::test]
async fn paid_chat_completion_conforms_to_chat_completion_response_schema() {
    let response = post_chat(
        paid_app(),
        &chat_body("openai/gpt-4o"),
        Some(valid_payment_header("/v1/chat/completions")),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);

    let body = response_json(response).await;
    assert_conforms(
        &component_validator("ChatCompletionResponse"),
        &body,
        "POST /v1/chat/completions 200 completion body",
    );
}

/// Unknown model → HTTP 404 with the standard error envelope (`Error`).
#[tokio::test]
async fn unknown_model_404_conforms_to_error_schema() {
    let response = post_chat(quote_app(), &chat_body("no-such/model"), None).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let body = response_json(response).await;
    assert_conforms(
        &component_validator("Error"),
        &body,
        "POST /v1/chat/completions 404 unknown-model body",
    );
    assert_eq!(
        body["error"]["type"], "model_not_found",
        "spec documents `model_not_found` for an unknown model: {body}"
    );
}

// ---------------------------------------------------------------------------
// GET /v1/receipts/{receipt_id} — settlement-platform P2 contract checks
// ---------------------------------------------------------------------------

/// With no database configured the receipts route returns an honest 503 whose
/// body conforms to the standard `Error` envelope with `error.type` =
/// `service_unavailable` (the spec's documented DB-less behavior).
#[tokio::test]
async fn receipts_dbless_503_conforms_to_error_schema() {
    let response = quote_app()
        .oneshot(
            Request::builder()
                .uri(format!("/v1/receipts/{}", uuid::Uuid::new_v4()))
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("oneshot");
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let body = response_json(response).await;
    assert_conforms(
        &component_validator("Error"),
        &body,
        "GET /v1/receipts/{id} 503 DB-less body",
    );
    assert_eq!(
        body["error"]["type"], "service_unavailable",
        "spec documents `service_unavailable` for a DB-less receipts route: {body}"
    );
}

/// A malformed receipt id on a storage-configured gateway → HTTP 404 with the
/// standard error envelope (`error.type` = `not_found`), conforming to the
/// spec's `Error` schema (mirrors `unknown_model_404_conforms_to_error_schema`).
///
/// The spec documents one indistinguishable 404 for unknown AND malformed
/// ids; the malformed-id arm rejects before any query is issued, so a LAZY
/// pool (never connected — the URL points at a closed port) is enough to put
/// the route into its storage-configured mode without a live Postgres. This
/// keeps the contract check deterministic in every CI environment.
#[tokio::test]
async fn receipts_404_conforms_to_error_schema() {
    let lazy_pool = sqlx::postgres::PgPoolOptions::new()
        .connect_lazy("postgres://contract:contract@127.0.0.1:1/never_connected")
        .expect("lazy pool construction does not connect");
    let app = contract_app_with_db(
        ProviderRegistry::from_env(reqwest::Client::new()),
        Some(lazy_pool),
    );

    let response = app
        .oneshot(
            Request::builder()
                .uri("/v1/receipts/not-a-uuid")
                .body(Body::empty())
                .expect("build request"),
        )
        .await
        .expect("oneshot");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let body = response_json(response).await;
    assert_conforms(
        &component_validator("Error"),
        &body,
        "GET /v1/receipts/{id} 404 body",
    );
    assert_eq!(
        body["error"]["type"], "not_found",
        "spec documents `not_found` for an unknown/malformed receipt id: {body}"
    );
}

/// The `gateway::receipts::Receipt` wire type is exactly what the GET route
/// serializes; validating constructed samples (vendor and plain) against the
/// spec's `Receipt` schema pins the wire shape without a live database.
#[test]
fn receipt_wire_type_conforms_to_receipt_schema() {
    let validator = component_validator("Receipt");

    let plain = gateway::receipts::Receipt {
        receipt_id: uuid::Uuid::new_v4(),
        created_at: chrono::Utc::now(),
        model: "openai/gpt-4o".to_string(),
        payment_scheme: "exact".to_string(),
        tx_signature: Some("base64-signed-tx".to_string()),
        payer_wallet: "AgentWallet11111111111111111111111111111111".to_string(),
        amount_paid_atomic: 2625,
        amount_paid_usdc: "0.002625".to_string(),
        cost_breakdown: gateway::receipts::ReceiptCostBreakdown {
            provider_cost_atomic: 2500,
            provider_cost_usdc: "0.002500".to_string(),
            platform_fee_atomic: 125,
            platform_fee_usdc: "0.000125".to_string(),
            total_atomic: 2625,
            total_usdc: "0.002625".to_string(),
            currency: "USDC".to_string(),
        },
        vendor: None,
    };
    assert_conforms(
        &validator,
        &serde_json::to_value(&plain).expect("serialize plain receipt"),
        "plain (no-vendor) Receipt wire shape",
    );

    let vendor = gateway::receipts::Receipt {
        vendor: Some(gateway::receipts::ReceiptVendor {
            vendor_wallet: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM".to_string(),
            settled_atomic: 20_000,
            settled_usdc: "0.020000".to_string(),
            fee_receivable_atomic: 1_000,
            fee_receivable_usdc: "0.001000".to_string(),
        }),
        tx_signature: None,
        ..plain
    };
    assert_conforms(
        &validator,
        &serde_json::to_value(&vendor).expect("serialize vendor receipt"),
        "vendor-settled Receipt wire shape",
    );
}
