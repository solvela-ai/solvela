use std::sync::Arc;

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use base64::Engine;
use tracing::{info, warn};

use solvela_x402::types::PaymentPayload;

use crate::AppState;

/// Decoded payment information stored in request extensions.
#[derive(Debug, Clone)]
pub struct PaymentInfo {
    /// The decoded payment payload from the PAYMENT-SIGNATURE header.
    pub payload: PaymentPayload,
    /// The raw header value (for re-encoding if needed).
    pub raw_header: String,
}

/// x402 payment extraction middleware.
///
/// This middleware extracts and decodes the PAYMENT-SIGNATURE header,
/// storing the result in request extensions for downstream handlers.
/// It does NOT enforce payment — that's done by the route handler
/// which can return 402 if payment is required but missing.
pub async fn extract_payment(
    State(_state): State<Arc<AppState>>,
    mut request: Request,
    next: Next,
) -> Response {
    // Try to extract and decode the payment header
    if let Some(header_value) = request
        .headers()
        .get("payment-signature")
        .and_then(|v| v.to_str().ok())
    {
        let raw = header_value.to_string();

        // Security: limit header size to prevent DoS via oversized base64 payloads
        const MAX_PAYMENT_HEADER_BYTES: usize = 50_000; // 50KB
        if raw.len() > MAX_PAYMENT_HEADER_BYTES {
            warn!(
                size = raw.len(),
                max = MAX_PAYMENT_HEADER_BYTES,
                "payment signature header exceeds size limit"
            );
            // Don't decode — let handler return 402
        } else {
            // The header value is base64-encoded JSON
            match decode_payment_header(&raw) {
                Ok(payload) => {
                    info!(
                        network = %payload.accepted.network,
                        resource = %payload.resource.url,
                        "payment signature extracted"
                    );
                    request.extensions_mut().insert(PaymentInfo {
                        payload,
                        raw_header: raw,
                    });
                }
                Err(e) => {
                    // If header is present but invalid, still let the request through.
                    // The route handler will see no PaymentInfo and return 402.
                    warn!(error = %e, "failed to decode payment signature header");
                }
            }
        }
    }

    next.run(request).await
}

/// Decode a PAYMENT-SIGNATURE header value.
///
/// The header is base64-encoded JSON containing a `PaymentPayload`.
/// Some clients may send raw JSON (not base64-encoded), so we try both.
///
/// Returns `Ok(payload)` on success, `Err(reason)` if the header cannot be decoded.
/// Used by both the extraction middleware and the chat route handler.
pub fn decode_payment_header(header: &str) -> Result<PaymentPayload, String> {
    // Try standard base64 decode first
    if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(header) {
        if let Ok(json_str) = String::from_utf8(decoded) {
            if let Ok(payload) = serde_json::from_str::<PaymentPayload>(&json_str) {
                return Ok(payload);
            }
        }
    }

    // Try URL-safe base64
    if let Ok(decoded) = base64::engine::general_purpose::URL_SAFE.decode(header) {
        if let Ok(json_str) = String::from_utf8(decoded) {
            if let Ok(payload) = serde_json::from_str::<PaymentPayload>(&json_str) {
                return Ok(payload);
            }
        }
    }

    // Try raw JSON
    if let Ok(payload) = serde_json::from_str::<PaymentPayload>(header) {
        return Ok(payload);
    }

    Err("unable to decode payment header: not valid base64 or JSON".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use solvela_x402::types::{PayloadData, PaymentAccept, PaymentPayload, Resource, SolanaPayload};

    /// Build a valid test PaymentPayload.
    fn sample_payload() -> PaymentPayload {
        PaymentPayload {
            x402_version: 2,
            resource: Resource {
                url: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
            },
            accepted: PaymentAccept {
                scheme: "exact".to_string(),
                network: "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp".to_string(),
                amount: "2625".to_string(),
                asset: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
                pay_to: "RecipientWallet123".to_string(),
                max_timeout_seconds: 300,
                escrow_program_id: None,
            },
            payload: PayloadData::Direct(SolanaPayload {
                transaction: "base64encodedtx".to_string(),
            }),
        }
    }

    #[test]
    fn test_decode_base64_encoded_payload() {
        let payload = sample_payload();
        let json = serde_json::to_string(&payload).unwrap();
        let encoded = base64::engine::general_purpose::STANDARD.encode(json.as_bytes());

        let decoded = decode_payment_header(&encoded).expect("should decode base64 payload");
        assert_eq!(decoded.x402_version, 2);
        assert_eq!(decoded.resource.url, "/v1/chat/completions");
        assert_eq!(decoded.accepted.pay_to, "RecipientWallet123");
    }

    #[test]
    fn test_decode_raw_json_payload() {
        let payload = sample_payload();
        let json = serde_json::to_string(&payload).unwrap();

        let decoded = decode_payment_header(&json).expect("should decode raw JSON payload");
        assert_eq!(decoded.x402_version, 2);
        assert_eq!(
            decoded.accepted.network,
            "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp"
        );
    }

    #[test]
    fn test_decode_invalid_header_returns_error() {
        let result = decode_payment_header("not-valid-anything!!!");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("unable to decode payment header"));
    }

    #[test]
    fn test_decode_oversized_header_rejected() {
        // Create a header larger than 50KB
        let oversized = "A".repeat(50_001);
        // The decode function itself doesn't enforce the limit — the middleware does.
        // But we verify that an oversized garbage string does not decode successfully.
        let result = decode_payment_header(&oversized);
        assert!(result.is_err(), "oversized header should not decode");
    }

    #[test]
    fn test_decode_url_safe_base64_payload() {
        let payload = sample_payload();
        let json = serde_json::to_string(&payload).unwrap();
        let encoded = base64::engine::general_purpose::URL_SAFE.encode(json.as_bytes());

        let decoded =
            decode_payment_header(&encoded).expect("should decode URL-safe base64 payload");
        assert_eq!(decoded.x402_version, 2);
        match &decoded.payload {
            PayloadData::Direct(p) => assert_eq!(p.transaction, "base64encodedtx"),
            PayloadData::Escrow(_) => panic!("expected Direct variant"),
        }
    }
}
