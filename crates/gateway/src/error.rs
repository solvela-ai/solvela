use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use uuid::Uuid;

/// Gateway-level errors returned as HTTP responses.
///
/// Some variants are defined for future payment flow phases but not
/// yet constructed — suppress dead_code until they're wired up.
#[derive(Debug, thiserror::Error)]
#[allow(dead_code)]
pub enum GatewayError {
    #[error("model not found: {0}")]
    ModelNotFound(String),

    #[error("provider error: {0}")]
    ProviderError(String),

    #[error("payment required")]
    PaymentRequired,

    #[error("invalid payment: {0}")]
    InvalidPayment(String),

    #[error("payment settlement failed: {0}")]
    SettlementFailed(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("rate limited")]
    RateLimited,

    #[error("internal error: {0}")]
    Internal(String),

    /// Internal upstream failure with a correlation id surfaced to the client.
    ///
    /// Used by the chat route when all providers fail. The verbose detail is
    /// captured server-side via `tracing::error!` *before* the error is built;
    /// the public body returns a generic message plus a request id so support
    /// can correlate the failure without leaking internal model/tx context.
    #[error("upstream provider unavailable (request_id={0})")]
    UpstreamUnavailable(String),
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        // Optional request id surfaced in the response body. Populated by
        // `UpstreamUnavailable`; other variants leave it `None` and the field
        // is omitted from the response.
        let mut request_id: Option<String> = None;

        // Maps each variant to (status, OpenAI-compatible error type, code, message).
        //
        // Cited pattern: the error envelope mirrors Franklin's normalization at
        // `src/proxy/server.ts:569-604` — clients downstream of Solvela can rely
        // on a stable `error.type` discriminant rather than provider-specific
        // shapes. Type values are drawn from OpenAI's error taxonomy so SDKs
        // don't have to special-case Solvela.
        let (status, error_type, error_code, message) = match &self {
            GatewayError::ModelNotFound(msg) => (
                StatusCode::NOT_FOUND,
                "invalid_request_error",
                "model_not_found",
                msg.clone(),
            ),
            GatewayError::ProviderError(msg) => {
                tracing::error!(error = %msg, "provider error");
                (
                    StatusCode::BAD_GATEWAY,
                    "upstream_error",
                    "provider_error",
                    "Upstream provider error".to_string(),
                )
            }
            GatewayError::PaymentRequired => (
                StatusCode::PAYMENT_REQUIRED,
                "payment_required",
                "payment_required",
                "Payment required. Include PAYMENT-SIGNATURE header.".to_string(),
            ),
            GatewayError::InvalidPayment(msg) => (
                StatusCode::PAYMENT_REQUIRED,
                "payment_required",
                "invalid_payment",
                msg.clone(),
            ),
            GatewayError::SettlementFailed(msg) => {
                tracing::error!(error = %msg, "payment settlement failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "upstream_error",
                    "settlement_failed",
                    "Payment settlement failed".to_string(),
                )
            }
            GatewayError::BadRequest(msg) => (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "bad_request",
                msg.clone(),
            ),
            GatewayError::RateLimited => (
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limit_error",
                "rate_limited",
                "Too many requests".to_string(),
            ),
            GatewayError::Internal(msg) => {
                tracing::error!(error = %msg, "internal server error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "upstream_error",
                    "internal_error",
                    "Internal server error".to_string(),
                )
            }
            GatewayError::UpstreamUnavailable(rid) => {
                request_id = Some(rid.clone());
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "upstream_error",
                    "upstream_unavailable",
                    "upstream provider unavailable".to_string(),
                )
            }
        };

        let body = match request_id {
            Some(rid) => json!({
                "error": {
                    "type": error_type,
                    "code": error_code,
                    "message": message,
                    "request_id": rid,
                }
            }),
            None => json!({
                "error": {
                    "type": error_type,
                    "code": error_code,
                    "message": message,
                }
            }),
        };

        (status, axum::Json(body)).into_response()
    }
}

/// Generate a fresh request id when the caller does not have one available
/// (e.g., the request-id middleware was bypassed or the extension is missing).
pub fn fresh_request_id() -> String {
    Uuid::new_v4().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;
    use http_body_util::BodyExt;

    /// Helper to extract status and body JSON from a GatewayError response.
    async fn error_response(err: GatewayError) -> (StatusCode, serde_json::Value) {
        let response = err.into_response();
        let status = response.status();
        let body = response.into_body().collect().await.unwrap().to_bytes();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        (status, json)
    }

    #[tokio::test]
    async fn test_model_not_found_returns_404() {
        let (status, json) = error_response(GatewayError::ModelNotFound(
            "openai/gpt-nonexistent".to_string(),
        ))
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(json["error"]["type"], "invalid_request_error");
        assert_eq!(json["error"]["code"], "model_not_found");
        assert!(json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("gpt-nonexistent"));
    }

    #[tokio::test]
    async fn test_provider_error_returns_502() {
        let (status, json) =
            error_response(GatewayError::ProviderError("upstream timeout".to_string())).await;
        assert_eq!(status, StatusCode::BAD_GATEWAY);
        assert_eq!(json["error"]["type"], "upstream_error");
        assert_eq!(json["error"]["code"], "provider_error");
        // Must NOT leak raw provider error details (URLs, timeouts, stack info) to clients
        assert_eq!(json["error"]["message"], "Upstream provider error");
        assert!(!json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("upstream timeout"));
    }

    #[tokio::test]
    async fn test_payment_required_returns_402() {
        let (status, json) = error_response(GatewayError::PaymentRequired).await;
        assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
        assert_eq!(json["error"]["type"], "payment_required");
        assert_eq!(json["error"]["code"], "payment_required");
    }

    #[tokio::test]
    async fn test_invalid_payment_returns_402() {
        let (status, json) =
            error_response(GatewayError::InvalidPayment("bad signature".to_string())).await;
        assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
        assert_eq!(json["error"]["type"], "payment_required");
        assert_eq!(json["error"]["code"], "invalid_payment");
        assert!(json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("bad signature"));
    }

    #[tokio::test]
    async fn test_settlement_failed_returns_500() {
        let (status, json) = error_response(GatewayError::SettlementFailed(
            "send failed: rpc URL https://internal.example/abc".to_string(),
        ))
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(json["error"]["type"], "upstream_error");
        assert_eq!(json["error"]["code"], "settlement_failed");
        // Must NOT leak raw error details (RPC URLs, tx context) to clients
        assert_eq!(json["error"]["message"], "Payment settlement failed");
    }

    #[tokio::test]
    async fn test_bad_request_returns_400() {
        let (status, json) =
            error_response(GatewayError::BadRequest("missing field".to_string())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["error"]["type"], "invalid_request_error");
        assert_eq!(json["error"]["code"], "bad_request");
    }

    #[tokio::test]
    async fn test_rate_limited_returns_429() {
        let (status, json) = error_response(GatewayError::RateLimited).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(json["error"]["type"], "rate_limit_error");
        assert_eq!(json["error"]["code"], "rate_limited");
    }

    #[tokio::test]
    async fn test_internal_error_returns_500() {
        let (status, json) =
            error_response(GatewayError::Internal("panic recovered".to_string())).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(json["error"]["type"], "upstream_error");
        assert_eq!(json["error"]["code"], "internal_error");
        // Must NOT leak raw error details to clients
        assert_eq!(json["error"]["message"], "Internal server error");
    }

    // -------------------------------------------------------------------------
    // Error Display trait
    // -------------------------------------------------------------------------

    #[test]
    fn test_error_display_messages() {
        assert_eq!(
            GatewayError::ModelNotFound("gpt-99".to_string()).to_string(),
            "model not found: gpt-99"
        );
        assert_eq!(
            GatewayError::ProviderError("timeout".to_string()).to_string(),
            "provider error: timeout"
        );
        assert_eq!(
            GatewayError::PaymentRequired.to_string(),
            "payment required"
        );
        assert_eq!(
            GatewayError::InvalidPayment("bad".to_string()).to_string(),
            "invalid payment: bad"
        );
        assert_eq!(
            GatewayError::SettlementFailed("fail".to_string()).to_string(),
            "payment settlement failed: fail"
        );
        assert_eq!(
            GatewayError::BadRequest("oops".to_string()).to_string(),
            "bad request: oops"
        );
        assert_eq!(GatewayError::RateLimited.to_string(), "rate limited");
        assert_eq!(
            GatewayError::Internal("crash".to_string()).to_string(),
            "internal error: crash"
        );
    }

    // -------------------------------------------------------------------------
    // Response body structure
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn test_all_errors_have_consistent_structure() {
        let errors: Vec<GatewayError> = vec![
            GatewayError::ModelNotFound("x".to_string()),
            GatewayError::ProviderError("x".to_string()),
            GatewayError::PaymentRequired,
            GatewayError::InvalidPayment("x".to_string()),
            GatewayError::SettlementFailed("x".to_string()),
            GatewayError::BadRequest("x".to_string()),
            GatewayError::RateLimited,
            GatewayError::Internal("x".to_string()),
            GatewayError::UpstreamUnavailable("req-123".to_string()),
        ];

        for err in errors {
            let (_, json) = error_response(err).await;
            assert!(
                json["error"]["type"].is_string(),
                "missing error.type field"
            );
            assert!(
                json["error"]["message"].is_string(),
                "missing error.message field"
            );
        }
    }

    #[tokio::test]
    async fn test_upstream_unavailable_returns_500_with_request_id() {
        let (status, json) = error_response(GatewayError::UpstreamUnavailable(
            "req-abc-123".to_string(),
        ))
        .await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(json["error"]["type"], "upstream_error");
        assert_eq!(json["error"]["code"], "upstream_unavailable");
        assert_eq!(json["error"]["message"], "upstream provider unavailable");
        assert_eq!(json["error"]["request_id"], "req-abc-123");
        // Must NOT leak model or tx detail
        let body_str = json.to_string();
        assert!(!body_str.contains("\"model\""));
        assert!(!body_str.contains("\"tx_signature\""));
    }

    #[test]
    fn test_fresh_request_id_is_uuid() {
        let id = fresh_request_id();
        assert_eq!(id.len(), 36, "uuid v4 string is 36 chars");
        assert!(uuid::Uuid::parse_str(&id).is_ok());
    }
}
