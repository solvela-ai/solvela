//! Session tokens for authenticated repeat access.
//!
//! After a successful x402 payment, the gateway issues a session token
//! (HMAC-SHA256 signed). Subsequent requests with this token skip the
//! 402 handshake entirely, drawing down from a pre-authorized budget.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::time::{SystemTime, UNIX_EPOCH};

type HmacSha256 = Hmac<Sha256>;

/// A session token's claims, issued after successful payment.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionClaims {
    /// Wallet address (base58) — identifies the payer.
    pub wallet: String,
    /// Remaining budget in atomic USDC.
    pub budget_remaining: u64,
    /// Unix timestamp when token was issued.
    pub issued_at: u64,
    /// Unix timestamp when token expires.
    pub expires_at: u64,
    /// Allowed models (empty = all models allowed).
    pub allowed_models: Vec<String>,
}

/// Errors that can occur during session token operations.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("malformed session token")]
    MalformedToken,
    #[error("invalid session token signature")]
    InvalidSignature,
    #[error("session token expired")]
    Expired,
    #[error("session budget exhausted")]
    BudgetExhausted,
    #[error("session storage error")]
    StorageError,
}

/// Create a signed session token.
///
/// Token format: `{base64url(json_claims)}.{base64url(hmac_sha256(payload))}`
///
/// # Errors
///
/// Returns [`SessionError::MalformedToken`] if claims cannot be serialized.
pub fn create_session_token(claims: &SessionClaims, secret: &[u8]) -> Result<String, SessionError> {
    let json = serde_json::to_vec(claims).map_err(|_| SessionError::MalformedToken)?;
    let payload = URL_SAFE_NO_PAD.encode(&json);

    let mut mac = HmacSha256::new_from_slice(secret).map_err(|_| SessionError::MalformedToken)?;
    mac.update(payload.as_bytes());
    let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());

    Ok(format!("{payload}.{signature}"))
}

/// Verify and decode a session token.
///
/// 1. Splits on `"."` to extract payload and signature.
/// 2. Recomputes HMAC-SHA256 over the payload and compares (constant-time).
/// 3. Decodes and deserializes the claims.
/// 4. Checks that the token has not expired.
///
/// # Errors
///
/// Returns an appropriate [`SessionError`] variant on failure.
pub fn verify_session_token(token: &str, secret: &[u8]) -> Result<SessionClaims, SessionError> {
    let (payload, sig_b64) = token.split_once('.').ok_or(SessionError::MalformedToken)?;

    // Verify signature
    let mut mac = HmacSha256::new_from_slice(secret).map_err(|_| SessionError::MalformedToken)?;
    mac.update(payload.as_bytes());

    let sig_bytes = URL_SAFE_NO_PAD
        .decode(sig_b64)
        .map_err(|_| SessionError::MalformedToken)?;

    mac.verify_slice(&sig_bytes)
        .map_err(|_| SessionError::InvalidSignature)?;

    // Decode claims
    let json = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| SessionError::MalformedToken)?;

    let claims: SessionClaims =
        serde_json::from_slice(&json).map_err(|_| SessionError::MalformedToken)?;

    // Check expiry
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| SessionError::MalformedToken)?
        .as_secs();

    if claims.expires_at <= now {
        return Err(SessionError::Expired);
    }

    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_claims(expires_at: u64) -> SessionClaims {
        SessionClaims {
            wallet: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
            budget_remaining: 5_000_000, // 5 USDC
            issued_at: 1_700_000_000,
            expires_at,
            allowed_models: vec!["gpt-4o".to_string(), "claude-sonnet-4-20250514".to_string()],
        }
    }

    fn far_future() -> u64 {
        // Year ~2040 — well past any test run
        2_200_000_000
    }

    #[test]
    fn test_create_and_verify_token() {
        let secret = b"test-hmac-secret-key-32-bytes!!!";
        let claims = sample_claims(far_future());

        let token = create_session_token(&claims, secret).expect("token creation should succeed");

        // Token must contain exactly one dot separator
        assert_eq!(
            token.matches('.').count(),
            1,
            "token should have exactly one dot"
        );

        let verified = verify_session_token(&token, secret).expect("verification should succeed");
        assert_eq!(verified, claims);
    }

    #[test]
    fn test_invalid_signature_rejected() {
        let secret_a = b"secret-key-alpha-32-bytes!!!!!!!";
        let secret_b = b"secret-key-bravo-32-bytes!!!!!!!";
        let claims = sample_claims(far_future());

        let token = create_session_token(&claims, secret_a).expect("creation should succeed");

        let result = verify_session_token(&token, secret_b);
        assert!(result.is_err());
        assert!(
            matches!(result, Err(SessionError::InvalidSignature)),
            "expected InvalidSignature, got {result:?}"
        );
    }

    #[test]
    fn test_expired_token_rejected() {
        let secret = b"test-hmac-secret-key-32-bytes!!!";
        // Already expired (Unix epoch + 1 second)
        let claims = sample_claims(1);

        let token = create_session_token(&claims, secret).expect("creation should succeed");

        let result = verify_session_token(&token, secret);
        assert!(result.is_err());
        assert!(
            matches!(result, Err(SessionError::Expired)),
            "expected Expired, got {result:?}"
        );
    }

    #[test]
    fn test_malformed_token_rejected() {
        let secret = b"test-hmac-secret-key-32-bytes!!!";

        // No dot separator
        let result = verify_session_token("garbage-no-dot", secret);
        assert!(matches!(result, Err(SessionError::MalformedToken)));

        // Empty parts
        let result = verify_session_token(".", secret);
        assert!(result.is_err());

        // Random base64-ish but invalid
        let result = verify_session_token("aGVsbG8.d29ybGQ", secret);
        assert!(result.is_err());
    }
}
