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

    // The AgentCard `url` IS the A2A JSON-RPC endpoint a conformant client POSTs
    // `message/send` directly to (A2A v0.3: `url` MUST support the
    // `preferredTransport`, which is `JSONRPC`). That handler is mounted at
    // `/a2a` (see `lib.rs`), so the advertised url is `{base}/a2a` — NOT the
    // bare host, which serves no JSON-RPC and would 404 a conformant probe.
    // Prefer the configured public URL; the host:port fallback is for local dev
    // only (the default bind host is 0.0.0.0, not publicly routable).
    let base = match &state.config.server.public_url {
        Some(u) => u.trim_end_matches('/').to_string(),
        None => format!(
            "http://{}:{}",
            state.config.server.host, state.config.server.port
        ),
    };
    let url = format!("{base}/a2a");

    Json(json!({
        "name": "Solvela",
        "description": "Solana-native AI agent payment gateway — pay for LLM API calls with USDC-SPL via x402",
        "url": url,
        "version": "0.1.0",
        // A2A v0.3 required descriptors: protocol version, the transport the
        // `url` speaks (our /a2a endpoint is JSON-RPC 2.0), and the default I/O
        // media types. Required for a card to pass a strict v0.3 schema validator.
        "protocolVersion": "0.3.0",
        "preferredTransport": "JSONRPC",
        "defaultInputModes": ["text/plain"],
        "defaultOutputModes": ["text/plain"],
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
                // `tags` is required on every AgentSkill in A2A v0.3.
                "tags": ["llm", "chat", "completions", "ai", "x402"],
                "inputModes": ["text/plain"],
                "outputModes": ["text/plain"]
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
            faucet_rate_limiter: crate::middleware::rate_limit::RateLimiter::new(
                crate::middleware::rate_limit::RateLimitConfig::faucet_default(),
            ),
            deposit_tx_rate_limiter: crate::middleware::rate_limit::RateLimiter::new(
                crate::middleware::rate_limit::RateLimitConfig::deposit_tx_default(),
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

    /// When `public_url` is configured, the card's `url` is the full A2A
    /// JSON-RPC endpoint (`{public_url}/a2a`) — the URL a conformant A2A client
    /// POSTs `message/send` directly to (A2A v0.3: `url` MUST support the
    /// `preferredTransport`, and ours is `JSONRPC`, served only at `/a2a`).
    #[tokio::test]
    async fn test_agent_card_url_uses_public_url_when_set() {
        let mut config = AppConfig::default();
        config.server.public_url = Some("https://api.solvela.ai/".to_string());
        let json = get_card(
            app_with_state(make_state_with_config(config)),
            "/.well-known/agent-card.json",
        )
        .await;
        // Trailing slash on the public URL is trimmed, then `/a2a` appended.
        assert_eq!(json["url"], "https://api.solvela.ai/a2a");
    }

    /// With no `public_url`, the card falls back to the host:port form (dev
    /// only), still pointing at the `/a2a` JSON-RPC endpoint.
    #[tokio::test]
    async fn test_agent_card_url_falls_back_to_host_port() {
        let json = get_card(test_app(), "/.well-known/agent-card.json").await;
        assert_eq!(json["url"], "http://0.0.0.0:8402/a2a");
    }

    /// Regression for the A2A conformance bug a2aregistry.org's probe caught:
    /// the AgentCard `url` MUST address a route that actually serves the
    /// `preferredTransport` (JSONRPC). Previously `url` was the bare public
    /// host, so a conformant client POSTing `message/send` directly to it hit
    /// the root and got 404 ("the message/send endpoint does not exist at the
    /// URL declared in the agent card"). Our prior a2a tests hardcoded
    /// `.uri("/a2a")`, so they never exercised the advertised URL — this test
    /// closes that gap by wiring BOTH the agent-card route AND the real
    /// production `/a2a` handler, then driving traffic to the path the card
    /// advertises.
    #[tokio::test]
    async fn test_advertised_url_path_reaches_real_a2a_endpoint() {
        // Router carrying the agent card AND the production JSON-RPC route.
        let app = axum::Router::new()
            .route(
                "/.well-known/agent-card.json",
                axum::routing::get(super::agent_card),
            )
            .route(
                "/a2a",
                axum::routing::post(crate::a2a::jsonrpc::a2a_endpoint),
            )
            .with_state(make_state());

        // 1. Fetch the card and extract the path component of the advertised URL.
        let json = get_card(app.clone(), "/.well-known/agent-card.json").await;
        let advertised = json["url"].as_str().expect("url is a string");
        let path = advertised
            .strip_prefix("http://0.0.0.0:8402")
            .expect("dev fallback url should be host:port-prefixed");
        // The advertised path must be the real JSON-RPC route, not the root.
        assert_eq!(
            path, "/a2a",
            "AgentCard url must advertise the /a2a JSON-RPC endpoint, not the bare host"
        );

        // 2. POST a minimal `message/send` to the advertised path and assert the
        //    response is NOT 404 — i.e. the path the card advertises is a real
        //    route that reaches the handler. (With cache: None the handler
        //    returns 200 + a JSON-RPC error body; any non-404 proves routing.)
        let resp = app
            .oneshot(
                http::Request::builder()
                    .method("POST")
                    .uri(path)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "jsonrpc": "2.0",
                            "method": "message/send",
                            "id": "conformance-1",
                            "params": {
                                "message": {
                                    "role": "user",
                                    "parts": [{"kind": "text", "text": "ping"}],
                                    "metadata": {"model": "test-model"}
                                }
                            }
                        })
                        .to_string(),
                    ))
                    .expect("valid request"),
            )
            .await
            .expect("request should succeed");
        assert_ne!(
            resp.status(),
            http::StatusCode::NOT_FOUND,
            "POSTing message/send to the advertised AgentCard url path must not 404 — \
             that 404 is the exact conformance failure this test guards against"
        );
    }

    /// The card carries every field a strict A2A v0.3 schema validator requires.
    #[tokio::test]
    async fn test_agent_card_has_v03_required_fields() {
        let json = get_card(test_app(), "/.well-known/agent-card.json").await;
        assert_eq!(json["protocolVersion"], "0.3.0");
        assert_eq!(json["preferredTransport"], "JSONRPC");
        assert_eq!(json["defaultInputModes"], serde_json::json!(["text/plain"]));
        assert_eq!(
            json["defaultOutputModes"],
            serde_json::json!(["text/plain"])
        );
        // Every AgentSkill must have a non-empty `tags` array in v0.3.
        let skills = json["skills"].as_array().expect("skills is array");
        assert!(!skills.is_empty(), "must advertise at least one skill");
        for skill in skills {
            let tags = skill["tags"].as_array().expect("skill.tags is array");
            assert!(!tags.is_empty(), "skill {} must have tags", skill["id"]);
        }
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
