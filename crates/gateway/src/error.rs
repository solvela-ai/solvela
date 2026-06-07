use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use solvela_x402::types::PaymentRequired;

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

    /// Emit a 402 with the x402-spec-compliant `PaymentRequired` body
    /// at the top level (NOT wrapped in the OpenAI-style error envelope).
    /// Used when the client hasn't presented a payment header yet and the
    /// gateway needs to advertise the cost + accepted schemes. Issue #217
    /// fix: previously this payload was stringified into
    /// `InvalidPayment(json_string)` and double-encoded inside
    /// `{ error: { message: "<json>" } }`, which violated the x402 spec.
    ///
    /// `InvalidPayment(String)` remains for short messages tied to a
    /// specific failure ("bad signature", "expired", "resource mismatch")
    /// — those keep the OpenAI envelope shape because they carry a single
    /// diagnostic string, not a structured challenge.
    ///
    /// `Box`ed to keep the enum small: `PaymentRequired` is ~208 bytes
    /// (Vec + multiple Strings), and every `Result<_, GatewayError>` along
    /// the request path would otherwise carry that footprint. The boxed
    /// allocation only happens on the 402 cold path.
    #[error("payment challenge")]
    PaymentChallenge(Box<PaymentRequired>),

    /// Inner string is forwarded to the client verbatim in the 402 response
    /// body — used for user-friendly error messages tied to a specific
    /// failure ("transaction has already been used", "Payment verification
    /// failed", "resource mismatch", etc.).
    ///
    /// For the *challenge* payload (no payment presented yet), use
    /// `PaymentChallenge(PaymentRequired)` so the body is emitted at the
    /// top level per the x402 spec, not stringified into this envelope.
    ///
    /// Constructors MUST NOT pass through:
    /// - raw provider/facilitator/RPC error bodies (those go through
    ///   `ProviderError`/`SettlementFailed` which sanitize),
    /// - internal hostnames/URLs,
    /// - parser exception details that fingerprint the implementation.
    ///
    /// When in doubt, log the internal detail at `warn!` and return a
    /// static safe-to-forward message here.
    #[error("invalid payment: {0}")]
    InvalidPayment(String),

    #[error("payment settlement failed: {0}")]
    SettlementFailed(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    /// The caller is authenticated/paying but not PERMITTED on this path. Maps
    /// to 403 Forbidden. Used by the service-marketplace proxy to reject a
    /// `require_tenant = TRUE` wallet (issue #499) BEFORE any settlement — such
    /// a wallet may only spend through `POST /v1/chat/completions`, which runs
    /// the per-tenant budget matrix. The inner message is forwarded verbatim and
    /// must stay a static, safe-to-share string (no internal detail).
    #[error("forbidden: {0}")]
    Forbidden(String),

    /// The request carried content the gateway accepts on the wire but does
    /// not yet process — currently image/multimodal content parts. Maps to
    /// 415 Unsupported Media Type. The inner message is forwarded verbatim
    /// (it is a static, safe-to-share capability message, not an internal
    /// detail).
    #[error("unsupported media type: {0}")]
    UnsupportedMediaType(String),

    #[error("rate limited")]
    RateLimited,

    /// No provider could fulfil the request (every provider in the fallback
    /// chain failed or its circuit is open). Maps to 503 Service Unavailable —
    /// a RETRYABLE status, distinct from the 500 used for genuine internal
    /// faults. On the paid path this is returned ONLY when no charge has been
    /// taken (`exact`: settlement was deferred and never broadcast) or no claim
    /// will be made (`escrow`: the deposit refunds at expiry) — i.e. the
    /// customer is never billed for an undelivered completion (#486). The inner
    /// message is forwarded verbatim and must stay free of internal detail
    /// (provider error bodies, RPC URLs) — log those at `warn!` instead.
    #[error("upstream unavailable: {0}")]
    UpstreamUnavailable(String),

    #[error("internal error: {0}")]
    Internal(String),
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        // PaymentChallenge emits the x402-spec body directly (no OpenAI
        // envelope), so it bypasses the (status, error_type, message)
        // tuple machinery the other variants share. See variant docs.
        if let GatewayError::PaymentChallenge(payment_required) = &self {
            return (
                StatusCode::PAYMENT_REQUIRED,
                axum::Json(payment_required.as_ref()),
            )
                .into_response();
        }

        let (status, error_type, message) = match &self {
            GatewayError::ModelNotFound(msg) => {
                (StatusCode::NOT_FOUND, "model_not_found", msg.clone())
            }
            GatewayError::ProviderError(msg) => {
                tracing::error!(error = %msg, "provider error");
                (
                    StatusCode::BAD_GATEWAY,
                    "provider_error",
                    "Upstream provider error".to_string(),
                )
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
            GatewayError::Forbidden(msg) => (StatusCode::FORBIDDEN, "forbidden", msg.clone()),
            GatewayError::UnsupportedMediaType(msg) => (
                StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "unsupported_media_type",
                msg.clone(),
            ),
            GatewayError::RateLimited => (
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "Too many requests".to_string(),
            ),
            GatewayError::UpstreamUnavailable(msg) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "upstream_unavailable",
                msg.clone(),
            ),
            GatewayError::Internal(msg) => {
                tracing::error!(error = %msg, "internal server error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "Internal server error".to_string(),
                )
            }
            // Handled by the `if let` short-circuit above. The arm exists
            // purely so the inner match stays exhaustive without an `_`
            // wildcard that would silently swallow any future variant.
            GatewayError::PaymentChallenge(_) => {
                unreachable!("PaymentChallenge is handled by the early return above")
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
    async fn test_forbidden_returns_403() {
        // Issue #499: require_tenant wallets rejected on the proxy path get 403.
        let (status, json) = error_response(GatewayError::Forbidden(
            "this wallet requires per-tenant budgeting; use POST /v1/chat/completions".to_string(),
        ))
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(json["error"]["type"], "forbidden");
        assert!(json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("per-tenant budgeting"));
    }

    #[tokio::test]
    async fn test_unsupported_media_type_returns_415() {
        let (status, json) = error_response(GatewayError::UnsupportedMediaType(
            "image/multimodal content is not yet supported".to_string(),
        ))
        .await;
        assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(json["error"]["type"], "unsupported_media_type");
        assert!(json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not yet supported"));
    }

    #[tokio::test]
    async fn test_rate_limited_returns_429() {
        let (status, json) = error_response(GatewayError::RateLimited).await;
        assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(json["error"]["type"], "rate_limited");
    }

    #[tokio::test]
    async fn test_upstream_unavailable_returns_503() {
        // #486: paid request that no provider could fulfil maps to a RETRYABLE
        // 503, distinct from the 500 used for genuine internal faults. The inner
        // message is forwarded verbatim, so the *caller* is contractually
        // responsible for keeping it free of internal detail (model IDs, RPC
        // URLs) — verified at the construction site in routes/chat. Here we lock
        // the status code and the envelope shape.
        let (status, json) = error_response(GatewayError::UpstreamUnavailable(
            "No provider could serve your request right now. Please retry shortly.".to_string(),
        ))
        .await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(json["error"]["type"], "upstream_unavailable");
        assert!(json["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Please retry shortly"));
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
        assert_eq!(
            GatewayError::UnsupportedMediaType("images".to_string()).to_string(),
            "unsupported media type: images"
        );
        assert_eq!(GatewayError::RateLimited.to_string(), "rate limited");
        assert_eq!(
            GatewayError::UpstreamUnavailable("down".to_string()).to_string(),
            "upstream unavailable: down"
        );
        assert_eq!(
            GatewayError::Internal("crash".to_string()).to_string(),
            "internal error: crash"
        );
        assert_eq!(
            GatewayError::PaymentChallenge(Box::new(build_test_payment_required())).to_string(),
            "payment challenge"
        );
    }

    /// Build a minimal valid `PaymentRequired` for tests that need to
    /// construct the `PaymentChallenge` variant. Synthetic values only.
    fn build_test_payment_required() -> PaymentRequired {
        use solvela_x402::types::{CostBreakdown, PaymentAccept, Resource};
        PaymentRequired {
            x402_version: 2,
            resource: Resource {
                url: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
            },
            accepts: vec![PaymentAccept {
                scheme: "exact".to_string(),
                network: "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp".to_string(),
                amount: "10500".to_string(),
                asset: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
                pay_to: "TestRecipient11111111111111111111111111111".to_string(),
                max_timeout_seconds: 300,
                escrow_program_id: None,
            }],
            cost_breakdown: CostBreakdown {
                provider_cost: "0.010000".to_string(),
                platform_fee: "0.000500".to_string(),
                total: "0.010500".to_string(),
                currency: "USDC".to_string(),
                fee_percent: 5,
            },
            error: "Payment required".to_string(),
        }
    }

    /// Issue #217: the `PaymentChallenge` variant emits `PaymentRequired`
    /// at the top level (x402-spec compliant) — NOT wrapped in the
    /// OpenAI-style `{ error: { type, message } }` envelope that every
    /// other variant uses. Lock that contract here so a future refactor
    /// that "uniformizes" the response shape is loud.
    #[tokio::test]
    async fn test_payment_challenge_emits_top_level_payment_required() {
        let (status, json) = error_response(GatewayError::PaymentChallenge(Box::new(
            build_test_payment_required(),
        )))
        .await;

        assert_eq!(status, StatusCode::PAYMENT_REQUIRED);

        // Top-level fields per x402 spec.
        assert_eq!(json["x402_version"], 2);
        assert!(json["accepts"].is_array());
        assert_eq!(json["accepts"][0]["scheme"], "exact");
        assert_eq!(json["accepts"][0]["amount"], "10500");
        assert_eq!(json["cost_breakdown"]["total"], "0.010500");
        assert_eq!(json["cost_breakdown"]["currency"], "USDC");
        assert_eq!(json["cost_breakdown"]["fee_percent"], 5);
        assert_eq!(json["resource"]["url"], "/v1/chat/completions");
        assert_eq!(json["error"], "Payment required");

        // Crucial: no OpenAI-style envelope wrapping. The `error` field is
        // a plain string at top level, not an object. A future regression
        // that re-wraps the body in `{ error: { type, message: "<json>" } }`
        // would make `error` an object and trip this assertion.
        assert!(
            !json["error"].is_object(),
            "PaymentChallenge must NOT wrap the PaymentRequired in the OpenAI envelope; \
             got top-level `error` as object: {json}"
        );
    }

    // -------------------------------------------------------------------------
    // Response body structure
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn test_all_errors_have_consistent_structure() {
        // NOTE: `PaymentChallenge` is *deliberately* excluded — it emits the
        // x402-spec `PaymentRequired` body at the top level, which by design
        // does not carry the `error.type`/`error.message` envelope every
        // other variant shares. See `test_payment_challenge_emits_top_level_payment_required`.
        let errors: Vec<GatewayError> = vec![
            GatewayError::ModelNotFound("x".to_string()),
            GatewayError::ProviderError("x".to_string()),
            GatewayError::PaymentRequired,
            GatewayError::InvalidPayment("x".to_string()),
            GatewayError::SettlementFailed("x".to_string()),
            GatewayError::BadRequest("x".to_string()),
            GatewayError::UnsupportedMediaType("x".to_string()),
            GatewayError::RateLimited,
            GatewayError::UpstreamUnavailable("x".to_string()),
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
