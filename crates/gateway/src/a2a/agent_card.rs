//! AgentCard endpoint — AP2 discovery for AI agents.
//!
//! Serves at `GET /.well-known/agent.json` per the A2A discovery convention.
//! Advertises AP2 merchant role and x402 Solana settlement support.

use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use axum::Json;
use serde_json::json;

use crate::a2a::types::{AP2_EXTENSION_URI, X402_EXTENSION_URI};
use crate::AppState;

/// `GET /.well-known/agent.json` — Return the A2A AgentCard.
///
/// Tool/skill schemas embedded here are passed through
/// `crate::util::schema_sanitize::sanitize_array_items` before serialization
/// so that strict downstream consumers (e.g., OpenAI o3-family models that
/// reject array-typed schemas without `items`) accept the card without
/// rewriting it. Pattern from Franklin `src/mcp/client.ts:53-80`.
pub async fn agent_card(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let mut schemes = vec!["exact"];
    if state.escrow_claimer.is_some() {
        schemes.push("escrow");
    }

    let mut card = json!({
        "name": "Solvela",
        "description": "Solana-native AI agent payment gateway — pay for LLM API calls with USDC-SPL via x402",
        "url": format!("http://{}:{}", state.config.server.host, state.config.server.port),
        "version": "0.1.0",
        "capabilities": {
            "streaming": true,
            "pushNotifications": false,
            "extensions": [
                {
                    "uri": AP2_EXTENSION_URI,
                    "description": "AP2 merchant for AI agent LLM payments",
                    "required": true,
                    "params": { "roles": ["merchant"] }
                },
                {
                    "uri": X402_EXTENSION_URI,
                    "description": "x402 on-chain settlement via Solana USDC-SPL",
                    "required": true,
                    "params": {
                        "network": "solana",
                        "asset": solvela_protocol::USDC_MINT,
                        "schemes": schemes
                    }
                }
            ]
        },
        "skills": [
            {
                "id": "chat-completion",
                "name": "Chat Completion",
                "description": "Proxy AI chat completions to multiple LLM providers (OpenAI, Anthropic, Google, xAI, DeepSeek)",
                "inputModes": ["text"],
                "outputModes": ["text"]
            }
        ]
    });

    // Walk the rendered card and ensure any embedded tool/skill schemas
    // satisfy the "every array has `items`" invariant. The current static
    // body contains no array-typed schemas, so this is defensive — when
    // skills grow tool parameter schemas they'll be sanitized automatically.
    crate::util::schema_sanitize::sanitize_array_items(&mut card);

    Json(card)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http;
    use tokio::sync::RwLock;
    use tower::ServiceExt;

    use crate::a2a::types::{AP2_EXTENSION_URI, X402_EXTENSION_URI};
    use crate::config::AppConfig;
    use crate::providers::health::{CircuitBreakerConfig, ProviderHealthTracker};
    use crate::providers::ProviderRegistry;
    use crate::routes::escrow::new_slot_cache;
    use crate::services::ServiceRegistry;
    use crate::usage::UsageTracker;
    use crate::AppState;
    use solvela_router::models::ModelRegistry;
    use solvela_x402::facilitator::Facilitator;

    fn make_state() -> Arc<AppState> {
        Arc::new(AppState {
            config: AppConfig::default(),
            model_registry: ModelRegistry::from_toml(
                r#"
[models.test-model]
provider = "test"
model_id = "test-model"
display_name = "Test"
input_cost_per_million = 1.0
output_cost_per_million = 2.0
context_window = 4096
supports_streaming = false
supports_tools = false
supports_vision = false
                "#,
            )
            .expect("valid test model TOML"),
            service_registry: RwLock::new(ServiceRegistry::empty()),
            providers: ProviderRegistry::from_env(reqwest::Client::new()),
            facilitator: Facilitator::new(vec![]),
            usage: UsageTracker::noop(),
            cache: None,
            provider_health: ProviderHealthTracker::new(CircuitBreakerConfig::default()),
            escrow_claimer: None,
            fee_payer_pool: None,
            nonce_pool: None,
            db_pool: None,
            session_secret: b"test-secret".to_vec(),
            http_client: reqwest::Client::new(),
            replay_set: AppState::new_replay_set(),
            slot_cache: new_slot_cache(),
            escrow_metrics: None,
            admin_token: None,
            prometheus_handle: None,
            dev_bypass_payment: false,
        })
    }

    fn test_app() -> axum::Router {
        let state = make_state();
        axum::Router::new()
            .route(
                "/.well-known/agent.json",
                axum::routing::get(super::agent_card),
            )
            .with_state(state)
    }

    #[tokio::test]
    async fn test_agent_card_returns_valid_json() {
        let app = test_app();
        let resp = app
            .oneshot(
                http::Request::builder()
                    .method("GET")
                    .uri("/.well-known/agent.json")
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("request should succeed");

        assert_eq!(resp.status(), http::StatusCode::OK);

        let body = axum::body::to_bytes(resp.into_body(), 4096)
            .await
            .expect("read body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");

        assert_eq!(json["name"], "Solvela");
        assert_eq!(json["version"], "0.1.0");
    }

    #[tokio::test]
    async fn test_agent_card_contains_extensions() {
        let app = test_app();
        let resp = app
            .oneshot(
                http::Request::builder()
                    .method("GET")
                    .uri("/.well-known/agent.json")
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("request should succeed");

        let body = axum::body::to_bytes(resp.into_body(), 4096)
            .await
            .expect("read body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");

        let extensions = json["capabilities"]["extensions"]
            .as_array()
            .expect("extensions is array");
        assert_eq!(extensions.len(), 2);
        assert_eq!(extensions[0]["uri"], AP2_EXTENSION_URI);
        assert_eq!(extensions[1]["uri"], X402_EXTENSION_URI);
    }

    #[tokio::test]
    async fn test_agent_card_exact_scheme_only_without_escrow() {
        let app = test_app();
        let resp = app
            .oneshot(
                http::Request::builder()
                    .method("GET")
                    .uri("/.well-known/agent.json")
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("request should succeed");

        let body = axum::body::to_bytes(resp.into_body(), 4096)
            .await
            .expect("read body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");

        let schemes = json["capabilities"]["extensions"][1]["params"]["schemes"]
            .as_array()
            .expect("schemes is array");
        assert_eq!(schemes, &[serde_json::json!("exact")]);
    }
}
