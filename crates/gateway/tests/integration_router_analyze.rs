//! Integration tests for `POST /v1/router/analyze`.

#![allow(unused_imports)]

#[path = "common/mod.rs"]
mod common;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tower::ServiceExt;

use common::test_app;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// POST a JSON body to `/v1/router/analyze` and return (status, body JSON).
async fn analyze(body: serde_json::Value) -> (StatusCode, serde_json::Value) {
    let app = test_app();
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/v1/router/analyze")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    (status, json)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// A short greeting should classify as tier="simple".
#[tokio::test]
async fn test_router_analyze_simple_request() {
    let body = serde_json::json!({
        "model": "auto",
        "messages": [{"role": "user", "content": "Hello!"}]
    });

    let (status, json) = analyze(body).await;
    assert_eq!(status, StatusCode::OK);

    let tier = json["tier"].as_str().expect("tier field must be a string");
    assert_eq!(
        tier, "simple",
        "short greeting should classify as simple, got: {tier}"
    );

    // Response must include all required fields.
    assert!(json["score"].is_number(), "score must be present");
    assert!(json["dimensions"].is_array(), "dimensions must be an array");
    assert!(json["profiles"].is_array(), "profiles must be an array");
    assert!(
        json["has_tools"].is_boolean(),
        "has_tools must be a boolean"
    );
    assert!(
        !json["has_tools"].as_bool().unwrap(),
        "has_tools should be false"
    );

    // 15 dimension entries
    let dims = json["dimensions"].as_array().unwrap();
    assert_eq!(dims.len(), 15, "should have exactly 15 dimensions");

    // 5 profile recommendations
    let profiles = json["profiles"].as_array().unwrap();
    assert_eq!(profiles.len(), 5, "should have 5 profile recommendations");
}

/// A long, reasoning-heavy prompt should classify as complex or reasoning.
#[tokio::test]
async fn test_router_analyze_complex_request() {
    let body = serde_json::json!({
        "model": "auto",
        "messages": [{
            "role": "user",
            "content": "Prove step by step that the quicksort algorithm is correct. \
                        Analyze the time complexity and evaluate whether it is optimal. \
                        Compare and contrast with merge sort, then explain why quicksort \
                        is often preferred in practice. Think through edge cases and reason \
                        about correctness guarantees for the distributed system architecture."
        }]
    });

    let (status, json) = analyze(body).await;
    assert_eq!(status, StatusCode::OK);

    let tier = json["tier"].as_str().expect("tier must be a string");
    assert!(
        tier == "complex" || tier == "reasoning",
        "long reasoning prompt should be complex or reasoning, got: {tier}"
    );
}

/// A request that includes a `tools` array should have `has_tools: true`
/// and expose an "agentic" profile entry.
#[tokio::test]
async fn test_router_analyze_with_tools() {
    let body = serde_json::json!({
        "model": "auto",
        "messages": [{"role": "user", "content": "Search the web for latest Solana news"}],
        "tools": [
            {
                "type": "function",
                "function": {
                    "name": "web_search",
                    "description": "Search the web",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "query": {"type": "string"}
                        },
                        "required": ["query"]
                    }
                }
            }
        ]
    });

    let (status, json) = analyze(body).await;
    assert_eq!(status, StatusCode::OK);

    assert!(
        json["has_tools"].as_bool().unwrap(),
        "has_tools should be true when tools array is present"
    );

    // The profiles array must include an agentic entry.
    let profiles = json["profiles"].as_array().unwrap();
    let has_agentic = profiles
        .iter()
        .any(|p| p["profile"].as_str() == Some("agentic"));
    assert!(has_agentic, "profiles should include an 'agentic' entry");

    // Each profile entry must include a non-empty selected_model.
    for profile in profiles {
        let model = profile["selected_model"].as_str().unwrap_or("");
        assert!(!model.is_empty(), "selected_model must not be empty");
    }
}

/// An empty `messages` array should be rejected with 400 Bad Request.
#[tokio::test]
async fn test_router_analyze_empty_messages() {
    let body = serde_json::json!({
        "model": "auto",
        "messages": []
    });

    let (status, json) = analyze(body).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "empty messages should return 400"
    );

    let error_type = json["error"]["type"].as_str().unwrap_or("");
    assert_eq!(error_type, "invalid_request_error");

    let msg = json["error"]["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("messages must not be empty"),
        "error message should mention empty messages, got: {msg}"
    );
}
