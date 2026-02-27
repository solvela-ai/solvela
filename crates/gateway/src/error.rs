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
            GatewayError::SettlementFailed(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "settlement_failed",
                msg.clone(),
            ),
            GatewayError::BadRequest(msg) => (StatusCode::BAD_REQUEST, "bad_request", msg.clone()),
            GatewayError::RateLimited => (
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "Too many requests".to_string(),
            ),
            GatewayError::Internal(msg) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal_error",
                msg.clone(),
            ),
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
