//! AgentCard endpoint — AP2 discovery for AI agents.
//!
//! Served at the A2A v0.3 canonical path `GET /.well-known/agent-card.json`
//! (RFC 8615), with `GET /.well-known/agent.json` kept as a backward-compatible
//! alias for pre-v0.3 clients. Both routes resolve to the same handler.
//! Advertises AP2 merchant role and x402 Solana settlement support.

use std::sync::Arc;

use axum::extract::State;
use axum::response::IntoResponse;
use axum::Json;
use serde_json::json;

use crate::a2a::types::{AP2_EXTENSION_URI, X402_EXTENSION_URI};
use crate::AppState;

/// `GET /.well-known/agent-card.json` (and the `/.well-known/agent.json` alias)
/// — Return the A2A AgentCard.
pub async fn agent_card(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let mut schemes = vec!["exact"];
    if state.escrow_claimer.is_some() {
        schemes.push("escrow");
    }

    // The AgentCard `url` is the A2A service endpoint external agents/registries
    // POST to. Prefer the configured public URL; the host:port fallback is for
    // local dev only (the default bind host is 0.0.0.0, not publicly routable).
    let url = match &state.config.server.public_url {
        Some(u) => u.trim_end_matches('/').to_string(),
        None => format!(
            "http://{}:{}",
            state.config.server.host, state.config.server.port
        ),
    };

    Json(json!({
        "name": "Solvela",
        "description": "Solana-native AI agent payment gateway — pay for LLM API calls with USDC-SPL via x402",
        "url": url,
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
                        // The CONFIGURED mint (what the verifier enforces),
                        // never the compile-time constant.
                        "asset": &state.config.solana.usdc_mint,
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
    }))
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
        make_state_with_config(AppConfig::default())
    }

    fn make_state_with_config(config: AppConfig) -> Arc<AppState> {
        Arc::new(AppState {
            config,
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
            semantic_cache: None,
            provider_health: ProviderHealthTracker::new(CircuitBreakerConfig::default()),
            escrow_claimer: None,
            fee_payer_pool: None,
            nonce_pool: None,
            db_pool: None,
            faucet: None,
            session_secret: b"test-secret".to_vec(),
            http_client: reqwest::Client::new(),
            replay_set: AppState::new_replay_set(),
            slot_cache: new_slot_cache(),
            escrow_metrics: None,
            admin_token: None,
            api_key_hmac_secret: None,
            prometheus_handle: None,
            dev_bypass_payment: false,
            free_rate_limiter: crate::middleware::rate_limit::RateLimiter::new(
                crate::middleware::rate_limit::RateLimitConfig::free_default(),
            ),
            receipts_rate_limiter: crate::middleware::rate_limit::RateLimiter::new(
                crate::middleware::rate_limit::RateLimitConfig::receipts_default(),
            ),
            free_global_cap: crate::middleware::rate_limit::FreeTierGlobalCap::new(
                crate::middleware::rate_limit::FREE_TIER_GLOBAL_RPM_DEFAULT,
            ),
        })
    }

    fn test_app() -> axum::Router {
        app_with_state(make_state())
    }

    /// Mirror production routing: the canonical A2A v0.3 path plus the
    /// backward-compat alias, both bound to the same handler.
    fn app_with_state(state: Arc<AppState>) -> axum::Router {
        axum::Router::new()
            .route(
                "/.well-known/agent-card.json",
                axum::routing::get(super::agent_card),
            )
            .route(
                "/.well-known/agent.json",
                axum::routing::get(super::agent_card),
            )
            .with_state(state)
    }

    async fn get_card(app: axum::Router, uri: &str) -> serde_json::Value {
        let resp = app
            .oneshot(
                http::Request::builder()
                    .method("GET")
                    .uri(uri)
                    .body(Body::empty())
                    .expect("valid request"),
            )
            .await
            .expect("request should succeed");
        assert_eq!(resp.status(), http::StatusCode::OK);
        // Generous cap: the card grows as skills/extensions are added; a tight
        // cap would silently truncate and fail with a misleading JSON error.
        let body = axum::body::to_bytes(resp.into_body(), 65_536)
            .await
            .expect("read body");
        serde_json::from_slice(&body).expect("valid JSON")
    }

    /// The canonical v0.3 path serves the card, byte-identical to the alias.
    #[tokio::test]
    async fn test_agent_card_canonical_path_matches_alias() {
        let canonical = get_card(test_app(), "/.well-known/agent-card.json").await;
        let alias = get_card(test_app(), "/.well-known/agent.json").await;
        assert_eq!(canonical["name"], "Solvela");
        assert_eq!(
            canonical, alias,
            "canonical agent-card.json and agent.json alias must return identical cards"
        );
    }

    /// When `public_url` is configured, the card's `url` is that value
    /// (an external agent/registry follows it to reach `/a2a`).
    #[tokio::test]
    async fn test_agent_card_url_uses_public_url_when_set() {
        let mut config = AppConfig::default();
        config.server.public_url = Some("https://api.solvela.ai/".to_string());
        let json = get_card(
            app_with_state(make_state_with_config(config)),
            "/.well-known/agent-card.json",
        )
        .await;
        // Trailing slash trimmed.
        assert_eq!(json["url"], "https://api.solvela.ai");
    }

    /// With no `public_url`, the card falls back to the host:port form (dev only).
    #[tokio::test]
    async fn test_agent_card_url_falls_back_to_host_port() {
        let json = get_card(test_app(), "/.well-known/agent-card.json").await;
        assert_eq!(json["url"], "http://0.0.0.0:8402");
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

    /// With a NON-default configured USDC mint, the AgentCard's x402 extension
    /// advertises the configured mint — an A2A agent following the card must
    /// build a payment the verifier will actually accept.
    #[tokio::test]
    async fn test_agent_card_advertises_configured_usdc_mint() {
        let devnet_mint = "4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU";
        let mut config = AppConfig::default();
        config.solana.usdc_mint = devnet_mint.to_string();
        let state = make_state_with_config(config);
        let app = axum::Router::new()
            .route(
                "/.well-known/agent.json",
                axum::routing::get(super::agent_card),
            )
            .with_state(state);

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

        assert_eq!(
            json["capabilities"]["extensions"][1]["params"]["asset"], devnet_mint,
            "agent card must advertise the configured mint"
        );
    }

    /// Default-config wire stability: with no mint override, the AgentCard
    /// advertises the mainnet USDC mint exactly (pinned as a literal).
    #[tokio::test]
    async fn test_agent_card_advertises_default_usdc_mint() {
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

        assert_eq!(
            json["capabilities"]["extensions"][1]["params"]["asset"],
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
        );
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
