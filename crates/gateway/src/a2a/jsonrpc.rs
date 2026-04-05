//! A2A JSON-RPC 2.0 dispatcher.
//!
//! Parses the JSON-RPC envelope, routes `message/send` to the handler,
//! and echoes the `X-A2A-Extensions` header for extension activation.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::Value;

use crate::a2a::types::{
    JsonRpcError, JsonRpcErrorData, JsonRpcRequest, JsonRpcResponse, MessageSendParams,
    A2A_EXTENSIONS_HEADER, X402_EXTENSION_URI,
};
use crate::AppState;

/// JSON-RPC 2.0 standard error codes.
const PARSE_ERROR: i32 = -32700;
const METHOD_NOT_FOUND: i32 = -32601;

/// `POST /a2a` — A2A JSON-RPC 2.0 endpoint.
pub async fn a2a_endpoint(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<JsonRpcRequest>,
) -> Response {
    // Validate JSON-RPC version
    if request.jsonrpc != "2.0" {
        return Json(JsonRpcError {
            jsonrpc: "2.0".to_string(),
            id: request.id.clone(),
            error: JsonRpcErrorData {
                code: PARSE_ERROR,
                message: "Invalid JSON-RPC version".to_string(),
                data: None,
            },
        })
        .into_response();
    }

    // Route by method
    let result = match request.method.as_str() {
        "message/send" => handle_message_send(state, &headers, &request).await,
        _ => Err(JsonRpcErrorData {
            code: METHOD_NOT_FOUND,
            message: format!("Method not found: {}", request.method),
            data: None,
        }),
    };

    // Build response with extension echo header
    let mut response = match result {
        Ok(value) => Json(JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: request.id,
            result: value,
        })
        .into_response(),
        Err(error) => Json(JsonRpcError {
            jsonrpc: "2.0".to_string(),
            id: request.id,
            error,
        })
        .into_response(),
    };

    // Echo X-A2A-Extensions header if client sent it
    if headers.contains_key(A2A_EXTENSIONS_HEADER) {
        if let Ok(val) = HeaderValue::from_str(X402_EXTENSION_URI) {
            response.headers_mut().insert(A2A_EXTENSIONS_HEADER, val);
        }
    }

    response
}

/// Stub handler for message/send — replaced by real handler in Task 5.
async fn handle_message_send(
    _state: Arc<AppState>,
    _headers: &HeaderMap,
    request: &JsonRpcRequest,
) -> Result<Value, JsonRpcErrorData> {
    let _params: MessageSendParams =
        serde_json::from_value(request.params.clone()).map_err(|e| JsonRpcErrorData {
            code: -32602,
            message: format!("Invalid params: {e}"),
            data: None,
        })?;

    Ok(serde_json::json!({
        "id": "stub_task",
        "status": {
            "state": "input-required",
            "message": {
                "role": "agent",
                "parts": [{"kind": "text", "text": "Payment required (stub)"}]
            }
        }
    }))
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
    use router::models::ModelRegistry;
    use x402::facilitator::Facilitator;

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
        });

        axum::Router::new()
            .route("/a2a", axum::routing::post(a2a_endpoint))
            .with_state(state)
    }

    #[tokio::test]
    async fn test_invalid_jsonrpc_version() {
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
        assert_eq!(json["error"]["code"], PARSE_ERROR);
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
    async fn test_message_send_stub_returns_task() {
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
                            "id": "req-1",
                            "params": {
                                "message": {
                                    "role": "user",
                                    "parts": [{"kind": "text", "text": "Hello"}]
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
        assert_eq!(json["result"]["status"]["state"], "input-required");
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
