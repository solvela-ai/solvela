//! A2A JSON-RPC 2.0 dispatcher.
//!
//! Parses the JSON-RPC envelope, routes the A2A v0.3 methods (`message/send`,
//! `tasks/get`, `tasks/cancel`, the four unsupported
//! `tasks/pushNotificationConfig/*` stubs) to the handler, and echoes the
//! `X-A2A-Extensions` header for extension activation.

use std::sync::Arc;

use crate::a2a::types::{
    JsonRpcError, JsonRpcErrorData, JsonRpcRequest, JsonRpcResponse, A2A_EXTENSIONS_HEADER,
    X402_EXTENSION_URI,
};
use crate::middleware::rate_limit::{connect_info_client_id, rate_limited_response, PeerAddr};
use crate::AppState;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::Json;

/// JSON-RPC 2.0 standard error codes.
const INVALID_REQUEST: i32 = -32600;
const METHOD_NOT_FOUND: i32 = -32601;

/// `-32003 PushNotificationNotSupportedError` (A2A v0.3 §8.2) for the four
/// `tasks/pushNotificationConfig/*` methods: the card declares
/// `pushNotifications: false`, and the spec mandates THIS code for them —
/// NOT the generic `-32601`. No parsing beyond the envelope, no state
/// contact.
fn push_not_supported() -> JsonRpcErrorData {
    JsonRpcErrorData {
        code: crate::a2a::handler::ERR_PUSH_NOT_SUPPORTED,
        message: "Push notifications are not supported by this agent \
                  (AgentCard capabilities.pushNotifications is false)"
            .to_string(),
        data: None,
    }
}

/// `POST /a2a` — A2A JSON-RPC 2.0 endpoint.
pub async fn a2a_endpoint(
    State(state): State<Arc<AppState>>,
    // Infallible peer-address extractor (same as the receipts route): `None`
    // when `ConnectInfo` is absent, degrading to the stricter "unknown"
    // rate-limit bucket rather than 500-ing.
    peer_addr: PeerAddr,
    headers: HeaderMap,
    Json(request): Json<JsonRpcRequest>,
) -> Response {
    // Validate JSON-RPC version. A well-formed body with a wrong `jsonrpc`
    // value is an INVALID REQUEST (-32600), not a parse error: the JSON parsed
    // fine (JSON-RPC 2.0 §5.1; A2A conformance plan Slice 2b). A genuinely
    // malformed body / missing `id` never reaches here — it dies in the axum
    // `Json` extractor as an HTTP 4xx with no JSON-RPC envelope (documented
    // behavior; see dashboard/content/docs/concepts/a2a.mdx).
    if request.jsonrpc != "2.0" {
        return Json(JsonRpcError::new(
            request.id.clone(),
            JsonRpcErrorData {
                code: INVALID_REQUEST,
                message: "Invalid JSON-RPC version".to_string(),
                data: None,
            },
        ))
        .into_response();
    }

    // `tasks/get` anti-enumeration limiter, enforced BEFORE dispatch at the
    // cheapest point (mirrors `GET /v1/receipts/{id}`): the unguessable task
    // id is a bearer capability and every lookup is a Redis read, so the
    // method gets a dedicated per-IP cap stricter than the generic outer
    // limiter. Keyed on the TCP peer IP, never a client-supplied header
    // (GHSA-6ggq-cvwx-4f67). The HTTP-level 429 (with Retry-After) is
    // transport-legal for JSON-RPC-over-HTTP — the generic outer limiter
    // already answers 429 on this route.
    if request.method == "tasks/get" {
        let client_id = connect_info_client_id(peer_addr.0);
        if state
            .a2a_tasks_rate_limiter
            .check(&client_id)
            .await
            .is_err()
        {
            metrics::counter!("solvela_a2a_tasks_get_rate_limited_total").increment(1);
            tracing::warn!(client_id = %client_id, "A2A tasks/get rate limit exceeded");
            return rate_limited_response(state.a2a_tasks_rate_limiter.config());
        }
    }

    // Route by method (A2A v0.3 §7; the push-notification methods are FOUR
    // explicit arms so a spec rename shows up here, not in a glob).
    let result = match request.method.as_str() {
        "message/send" => crate::a2a::handler::handle_message_send(state, &headers, &request).await,
        "tasks/get" => crate::a2a::handler::handle_tasks_get(&state, &request).await,
        "tasks/cancel" => crate::a2a::handler::handle_tasks_cancel(&state, &request).await,
        "tasks/pushNotificationConfig/set" => Err(push_not_supported()),
        "tasks/pushNotificationConfig/get" => Err(push_not_supported()),
        "tasks/pushNotificationConfig/list" => Err(push_not_supported()),
        "tasks/pushNotificationConfig/delete" => Err(push_not_supported()),
        _ => Err(JsonRpcErrorData {
            code: METHOD_NOT_FOUND,
            message: format!("Method not found: {}", request.method),
            data: None,
        }),
    };

    // Build response with extension echo header
    let mut response = match result {
        Ok(value) => Json(JsonRpcResponse::success(request.id, value)).into_response(),
        Err(error) => Json(JsonRpcError::new(request.id, error)).into_response(),
    };

    // Echo X-A2A-Extensions header if client sent it
    if headers.contains_key(A2A_EXTENSIONS_HEADER) {
        if let Ok(val) = HeaderValue::from_str(X402_EXTENSION_URI) {
            response.headers_mut().insert(A2A_EXTENSIONS_HEADER, val);
        }
    }

    response
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::body::Body;
    use axum::http;
    use serde_json::json;
    use tokio::sync::RwLock;
    use tower::ServiceExt;

    use super::*;
    use crate::config::AppConfig;
    use crate::providers::health::{CircuitBreakerConfig, ProviderHealthTracker};
    use crate::providers::ProviderRegistry;
    use crate::routes::escrow::new_slot_cache;
    use crate::services::ServiceRegistry;
    use crate::usage::UsageTracker;
    use crate::AppState;
    use solvela_router::models::ModelRegistry;
    use solvela_x402::facilitator::Facilitator;

    fn test_app() -> axum::Router {
        let state = Arc::new(AppState {
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
            native_anthropic: None,
            search_provider: None,
            price_provider: None,
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
            auth_provider: None,
            prometheus_handle: None,
            dev_bypass_payment: false,
            free_rate_limiter: crate::middleware::rate_limit::RateLimiter::new(
                crate::middleware::rate_limit::RateLimitConfig::free_default(),
            ),
            receipts_rate_limiter: crate::middleware::rate_limit::RateLimiter::new(
                crate::middleware::rate_limit::RateLimitConfig::receipts_default(),
            ),
            a2a_tasks_rate_limiter: crate::middleware::rate_limit::RateLimiter::new(
                crate::middleware::rate_limit::RateLimitConfig::a2a_tasks_default(),
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
        });

        axum::Router::new()
            .route("/a2a", axum::routing::post(a2a_endpoint))
            .with_state(state)
    }

    /// 2b-8 (conformance plan): a well-formed envelope whose `jsonrpc` field
    /// is not `"2.0"` is an INVALID REQUEST (`-32600`), not a parse error
    /// (`-32700`) — the body parsed fine; it is the request object that is
    /// invalid (JSON-RPC 2.0 §5.1).
    #[tokio::test]
    async fn wrong_jsonrpc_version_returns_invalid_request() {
        let app = test_app();
        let resp = app
            .oneshot(
                http::Request::builder()
                    .method("POST")
                    .uri("/a2a")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "jsonrpc": "1.0",
                            "method": "message/send",
                            "id": "1",
                            "params": {}
                        })
                        .to_string(),
                    ))
                    .expect("valid request"), // safe: known-good test data
            )
            .await
            .expect("request should succeed");

        assert_eq!(resp.status(), http::StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 4096)
            .await
            .expect("read body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");
        assert_eq!(json["error"]["code"], INVALID_REQUEST);
    }

    #[tokio::test]
    async fn test_unknown_method() {
        let app = test_app();
        let resp = app
            .oneshot(
                http::Request::builder()
                    .method("POST")
                    .uri("/a2a")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "jsonrpc": "2.0",
                            "method": "unknown/method",
                            "id": "1",
                            "params": {}
                        })
                        .to_string(),
                    ))
                    .expect("valid request"), // safe: known-good test data
            )
            .await
            .expect("request should succeed");

        assert_eq!(resp.status(), http::StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 4096)
            .await
            .expect("read body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");
        assert_eq!(json["error"]["code"], METHOD_NOT_FOUND);
    }

    #[tokio::test]
    async fn test_message_send_without_redis_returns_error() {
        // Redis (cache: None) is required to persist task state before issuing a
        // task ID. Without it we must return an error — clients must not be able
        // to pay USDC against a task that cannot be loaded later.
        let app = test_app(); // test_app() has cache: None
        let resp = app
            .oneshot(
                http::Request::builder()
                    .method("POST")
                    .uri("/a2a")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "jsonrpc": "2.0",
                            "method": "message/send",
                            "id": "req-1",
                            "params": {
                                "message": {
                                    "role": "user",
                                    "parts": [{"kind": "text", "text": "Hello"}],
                                    "metadata": {"model": "test-model"}
                                }
                            }
                        })
                        .to_string(),
                    ))
                    .expect("valid request"), // safe: known-good test data
            )
            .await
            .expect("request should succeed");

        assert_eq!(resp.status(), http::StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 4096)
            .await
            .expect("read body");
        let json: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON");
        // ERR_INTERNAL = -32603: task store unavailable
        assert_eq!(
            json["error"]["code"], -32603_i32,
            "should return ERR_INTERNAL"
        );
        assert!(json["result"].is_null(), "result should be null on error");
    }

    #[tokio::test]
    async fn test_extension_header_echo() {
        let app = test_app();
        let resp = app
            .oneshot(
                http::Request::builder()
                    .method("POST")
                    .uri("/a2a")
                    .header("content-type", "application/json")
                    .header("x-a2a-extensions", X402_EXTENSION_URI)
                    .body(Body::from(
                        json!({
                            "jsonrpc": "2.0",
                            "method": "message/send",
                            "id": "1",
                            "params": {
                                "message": {
                                    "role": "user",
                                    "parts": [{"kind": "text", "text": "test"}]
                                }
                            }
                        })
                        .to_string(),
                    ))
                    .expect("valid request"), // safe: known-good test data
            )
            .await
            .expect("request should succeed");

        assert!(resp.headers().contains_key("x-a2a-extensions"));
        assert_eq!(
            resp.headers()
                .get("x-a2a-extensions")
                .expect("header present") // safe: just asserted contains_key
                .to_str()
                .expect("valid UTF-8 header"), // safe: X402_EXTENSION_URI is valid UTF-8
            X402_EXTENSION_URI
        );
    }

    async fn call_method(app: &axum::Router, method: &str) -> serde_json::Value {
        let resp = app
            .clone()
            .oneshot(
                http::Request::builder()
                    .method("POST")
                    .uri("/a2a")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "jsonrpc": "2.0",
                            "method": method,
                            "id": "1",
                            "params": {"id": "a2a_deadbeef"}
                        })
                        .to_string(),
                    ))
                    .expect("valid request"), // safe: known-good test data
            )
            .await
            .expect("request should succeed");
        assert_eq!(resp.status(), http::StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 4096)
            .await
            .expect("read body");
        serde_json::from_slice(&body).expect("valid JSON")
    }

    /// All four `tasks/pushNotificationConfig/*` methods return exactly
    /// `-32003 PushNotificationNotSupportedError` (A2A v0.3 §8.2 — the card
    /// declares `pushNotifications: false`; the spec mandates this code, NOT
    /// the generic `-32601`).
    #[tokio::test]
    async fn push_notification_methods_return_not_supported() {
        let app = test_app();
        for method in [
            "tasks/pushNotificationConfig/set",
            "tasks/pushNotificationConfig/get",
            "tasks/pushNotificationConfig/list",
            "tasks/pushNotificationConfig/delete",
        ] {
            let json = call_method(&app, method).await;
            assert_eq!(
                json["error"]["code"], -32003,
                "{method} must return PushNotificationNotSupportedError, got: {json}"
            );
        }
    }

    /// `tasks/get` and `tasks/cancel` are ROUTED (never `-32601`). With this
    /// fixture's `cache: None` both fail closed at the task store with
    /// `-32603` — the retry signal, never a spurious not-found (invariant 6).
    #[tokio::test]
    async fn tasks_get_and_cancel_route_and_fail_closed_without_redis() {
        let app = test_app();
        for method in ["tasks/get", "tasks/cancel"] {
            let json = call_method(&app, method).await;
            assert_eq!(
                json["error"]["code"], -32603,
                "{method} without Redis must fail closed with -32603, got: {json}"
            );
        }
    }

    #[tokio::test]
    async fn test_no_extension_header_when_not_sent() {
        let app = test_app();
        let resp = app
            .oneshot(
                http::Request::builder()
                    .method("POST")
                    .uri("/a2a")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({
                            "jsonrpc": "2.0",
                            "method": "message/send",
                            "id": "1",
                            "params": {
                                "message": {
                                    "role": "user",
                                    "parts": [{"kind": "text", "text": "test"}]
                                }
                            }
                        })
                        .to_string(),
                    ))
                    .expect("valid request"), // safe: known-good test data
            )
            .await
            .expect("request should succeed");

        assert!(!resp.headers().contains_key("x-a2a-extensions"));
    }
}
