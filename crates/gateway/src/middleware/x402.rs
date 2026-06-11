use std::sync::Arc;

use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
};
use base64::Engine;
use tracing::{debug, info, warn};

use solvela_x402::types::{CanonicalPaymentEnvelope, PaymentPayload};

use crate::AppState;

/// Decoded payment information stored in request extensions.
///
/// `Debug` is implemented manually to redact the raw signed-transaction
/// payload. The default derive would expose the full base64 transaction
/// (and any embedded signatures) any time this struct is formatted via
/// `{:?}` — e.g. through `tracing::debug!(?ext, ...)` on request
/// extensions. Routes/handlers should never need the raw transaction in
/// logs; if they do, log the parsed payload's fields explicitly.
#[derive(Clone)]
pub struct PaymentInfo {
    /// The decoded payment payload from the PAYMENT-SIGNATURE header.
    pub payload: PaymentPayload,
    /// The raw header value (for re-encoding if needed).
    pub raw_header: String,
}

impl std::fmt::Debug for PaymentInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Show enough metadata to be useful for debugging (network, scheme,
        // resource URL) while redacting the raw transaction. The first 8
        // chars of `raw_header` are kept as a correlation prefix so log
        // entries can be matched up without exposing the full payload.
        let raw_prefix: String = self.raw_header.chars().take(8).collect();
        f.debug_struct("PaymentInfo")
            .field("network", &self.payload.accepted.network)
            .field("scheme", &self.payload.accepted.scheme)
            .field("resource", &self.payload.resource.url)
            .field(
                "raw_header",
                &format_args!("{raw_prefix}…[{} bytes redacted]", self.raw_header.len()),
            )
            .finish()
    }
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
                    //
                    // The decode-failure detail can embed (length-capped)
                    // attacker-controlled bytes — e.g. the canonical
                    // envelope's rejected scheme — so the indexed warn! stays
                    // a fixed string and the detail is demoted to debug!. The
                    // paying routes (routes/chat, routes/proxy) log the same
                    // failure WITH its reason at warn! when they hard-fail
                    // the request, so the reason is never lost.
                    warn!("failed to decode payment signature header");
                    debug!(error = %e, "payment signature decode failure detail");
                }
            }
        }
    }

    next.run(request).await
}

/// Decode a PAYMENT-SIGNATURE header value.
///
/// The header is base64-encoded JSON containing either the legacy Solvela
/// `PaymentPayload` (snake_case — what the published SDKs send) or a
/// canonical x402 v2 `PaymentSignatureEnvelope` (camelCase — what standard
/// x402 clients like the solana-foundation `pay` CLI send). Some clients may
/// send raw JSON (not base64-encoded), so we try both encodings.
///
/// The legacy shape is always tried FIRST and is unchanged; the two shapes
/// are mutually unparseable (legacy uses `deny_unknown_fields` and the
/// canonical envelope requires `x402Version`), so neither can shadow the
/// other. A canonical envelope that parses but fails the fail-closed mapping
/// (escrow scheme, signature proof, non-v2 version, missing fields) returns
/// that specific error — it is never silently retried as something else.
///
/// Returns `Ok(payload)` on success, `Err(reason)` if the header cannot be decoded.
/// Used by the extraction middleware and the chat/proxy route handlers.
pub fn decode_payment_header(header: &str) -> Result<PaymentPayload, String> {
    // Try standard base64 decode first
    if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(header) {
        if let Ok(json_str) = String::from_utf8(decoded) {
            if let Some(result) = parse_payment_json(&json_str) {
                return result;
            }
        }
    }

    // Try URL-safe base64
    if let Ok(decoded) = base64::engine::general_purpose::URL_SAFE.decode(header) {
        if let Ok(json_str) = String::from_utf8(decoded) {
            if let Some(result) = parse_payment_json(&json_str) {
                return result;
            }
        }
    }

    // Try raw JSON
    if let Some(result) = parse_payment_json(header) {
        return result;
    }

    Err("unable to decode payment header: not valid base64 or JSON".to_string())
}

/// Parse a candidate JSON string as a payment payload.
///
/// Returns:
/// - `None` — the string is neither shape; the caller falls through to the
///   next decoding attempt.
/// - `Some(Ok(_))` — a legacy `PaymentPayload`, or a canonical v2 envelope
///   successfully mapped into one.
/// - `Some(Err(_))` — a canonical v2 envelope that parsed but was REJECTED by
///   the fail-closed mapping (escrow scheme, signature proof, unsupported
///   version, missing accepted/resource). Surfaced as a hard error so a
///   money-relevant rejection is never masked by further decode attempts.
fn parse_payment_json(json_str: &str) -> Option<Result<PaymentPayload, String>> {
    if let Ok(payload) = serde_json::from_str::<PaymentPayload>(json_str) {
        return Some(Ok(payload));
    }
    if let Ok(envelope) = serde_json::from_str::<CanonicalPaymentEnvelope>(json_str) {
        return Some(
            envelope
                .into_payment_payload()
                .map_err(|e| format!("canonical x402 payment rejected: {e}")),
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use solvela_x402::types::{
        PayloadData, PaymentAccept, PaymentPayload, Resource, SolanaPayload,
    };

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

    /// Build a canonical x402 v2 envelope JSON (pay-CLI shape) with the given
    /// scheme and proof object. Field names are literals so these tests pin
    /// the wire contract independently of the serde derives.
    fn canonical_envelope_json(scheme: &str, proof: serde_json::Value) -> String {
        serde_json::json!({
            "x402Version": 2,
            "accepted": {
                "scheme": scheme,
                "network": "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
                "amount": "2625",
                "asset": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                "payTo": "RecipientWallet123",
                "maxTimeoutSeconds": 300,
                "extra": { "decimals": 6 }
            },
            "resource": { "url": "/v1/chat/completions", "description": "API access" },
            "payload": proof
        })
        .to_string()
    }

    #[test]
    fn test_decode_canonical_v2_envelope_base64() {
        let json =
            canonical_envelope_json("exact", serde_json::json!({ "transaction": "dGVzdA==" }));
        let encoded = base64::engine::general_purpose::STANDARD.encode(json.as_bytes());

        let decoded = decode_payment_header(&encoded).expect("canonical envelope must decode");
        assert_eq!(decoded.x402_version, 2);
        assert_eq!(decoded.accepted.scheme, "exact");
        assert_eq!(decoded.accepted.pay_to, "RecipientWallet123");
        assert_eq!(decoded.resource.url, "/v1/chat/completions");
        assert_eq!(decoded.resource.method, "POST");
        match &decoded.payload {
            PayloadData::Direct(p) => assert_eq!(p.transaction, "dGVzdA=="),
            PayloadData::Escrow(_) => panic!("canonical payment must map to Direct"),
        }
    }

    #[test]
    fn test_decode_canonical_v2_envelope_raw_json() {
        let json =
            canonical_envelope_json("exact", serde_json::json!({ "transaction": "dGVzdA==" }));
        let decoded = decode_payment_header(&json).expect("raw canonical JSON must decode");
        assert_eq!(decoded.accepted.scheme, "exact");
    }

    /// A canonical envelope selecting escrow must be a hard error with the
    /// canonical-rejection message — never silently retried or defaulted.
    #[test]
    fn test_decode_canonical_escrow_scheme_is_hard_error() {
        let json =
            canonical_envelope_json("escrow", serde_json::json!({ "transaction": "dGVzdA==" }));
        let encoded = base64::engine::general_purpose::STANDARD.encode(json.as_bytes());

        let err = decode_payment_header(&encoded).expect_err("escrow must be rejected");
        assert!(
            err.contains("canonical x402 payment rejected"),
            "error must surface the canonical rejection: {err}"
        );
        assert!(
            err.contains("exact"),
            "error must name the accepted scheme: {err}"
        );
    }

    /// A canonical client-broadcast `signature` proof must be a hard error —
    /// the gateway controls broadcast (deferred settlement, #486).
    #[test]
    fn test_decode_canonical_signature_proof_is_hard_error() {
        let json = canonical_envelope_json(
            "exact",
            serde_json::json!({ "signature": "5wZ2UM8fFakeSignature" }),
        );
        let encoded = base64::engine::general_purpose::STANDARD.encode(json.as_bytes());

        let err = decode_payment_header(&encoded).expect_err("signature proof must be rejected");
        assert!(err.contains("canonical x402 payment rejected"), "{err}");
    }

    /// The legacy payload must still decode FIRST and unchanged — the
    /// canonical path is additive only.
    #[test]
    fn test_decode_legacy_payload_unchanged_alongside_canonical() {
        let payload = sample_payload();
        let json = serde_json::to_string(&payload).unwrap();
        let encoded = base64::engine::general_purpose::STANDARD.encode(json.as_bytes());

        let decoded = decode_payment_header(&encoded).expect("legacy payload must still decode");
        assert_eq!(decoded.x402_version, 2);
        assert_eq!(decoded.resource.method, "POST");
        assert_eq!(decoded.accepted.pay_to, "RecipientWallet123");
    }

    /// Regression: `Debug` on `PaymentInfo` must redact the raw signed
    /// transaction. The previous `#[derive(Debug)]` exposed the full
    /// base64 transaction (including signatures) any time the struct was
    /// formatted via `{:?}`.
    #[test]
    fn payment_info_debug_redacts_raw_header() {
        let secret_tx = "AAAAAAAAAAAAAAAAAAAAAAA_super_secret_signed_tx_payload_AAAAAAAAAA";
        let info = PaymentInfo {
            payload: sample_payload(),
            raw_header: secret_tx.to_string(),
        };
        let debug = format!("{info:?}");
        assert!(
            !debug.contains("super_secret_signed_tx_payload"),
            "Debug output must NOT contain full raw_header: {debug}"
        );
        let expected_marker = format!("[{} bytes redacted]", secret_tx.len());
        assert!(
            debug.contains(&expected_marker),
            "Debug output must indicate redaction with byte count {expected_marker:?}: {debug}"
        );
        // Useful metadata IS shown.
        assert!(debug.contains("network"));
        assert!(debug.contains("resource"));
        assert!(debug.contains("/v1/chat/completions"));
    }
}
