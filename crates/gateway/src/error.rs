use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

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
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        let (status, error_type, message) = match &self {
            GatewayError::ModelNotFound(msg) => {
                (StatusCode::NOT_FOUND, "model_not_found", msg.clone())
            }
            GatewayError::ProviderError(msg) => {
                (StatusCode::BAD_GATEWAY, "provider_error", msg.clone())
            }
            GatewayError::PaymentRequired => (
                StatusCode::PAYMENT_REQUIRED,
                "payment_required",
                "Payment required. Include PAYMENT-SIGNATURE header.".to_string(),
            ),
            GatewayError::InvalidPayment(msg) => {
                (StatusCode::PAYMENT_REQUIRED, "invalid_payment", msg.clone())
            }
            GatewayError::SettlementFailed(msg) => {
                tracing::error!(error = %msg, "payment settlement failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "settlement_failed",
                    "Payment settlement failed".to_string(),
                )
            }
            GatewayError::BadRequest(msg) => (StatusCode::BAD_REQUEST, "bad_request", msg.clone()),
            GatewayError::RateLimited => (
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "Too many requests".to_string(),
            ),
            GatewayError::Internal(msg) => {
                tracing::error!(error = %msg, "internal server error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "Internal server error".to_string(),
                )
            }
        };

        let body = json!({
            "error": {
                "type": error_type,
                "message": message,
            }
        });

        (status, axum::Json(body)).into_response()
    }
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
        assert_eq!(json["error"]["type"], "model_not_found");
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
        assert_eq!(json["error"]["type"], "provider_error");
        assert!(json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("upstream timeout"));
    }

    #[tokio::test]
    async fn test_payment_required_returns_402() {
        let (status, json) = error_response(GatewayError::PaymentRequired).await;
        assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
        assert_eq!(json["error"]["type"], "payment_required");
    }

    #[tokio::test]
    async fn test_invalid_payment_returns_402() {
        let (status, json) =
            error_response(GatewayError::InvalidPayment("bad signature".to_string())).await;
        assert_eq!(status, StatusCode::PAYMENT_REQUIRED);
        assert_eq!(json["error"]["type"], "invalid_payment");
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
        assert_eq!(json["error"]["type"], "settlement_failed");
        // Must NOT leak raw error details (RPC URLs, tx context) to clients
        assert_eq!(json["error"]["message"], "Payment settlement failed");
    }

    #[tokio::test]
    async fn test_bad_request_returns_400() {
        let (status, json) =
            error_response(GatewayError::BadRequest("missing field".to_string())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(json["error"]["type"], "bad_request");
    }

    #[tokio::test]
    async fn test_rate_limited_returns_429() {
        let (status, json) = error_response(GatewayError::RateLimited).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(json["error"]["type"], "rate_limited");
    }

    #[tokio::test]
    async fn test_internal_error_returns_500() {
        let (status, json) =
            error_response(GatewayError::Internal("panic recovered".to_string())).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(json["error"]["type"], "internal_error");
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
}
