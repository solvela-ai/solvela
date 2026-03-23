//! Response construction helpers: debug headers, session tokens, session IDs.

use axum::http::{HeaderName, HeaderValue};
use axum::response::Response;

use crate::routes::debug_headers::{CacheStatus, DebugInfo, PaymentStatus};

/// Maximum length for a client-provided session ID.
const MAX_SESSION_ID_LEN: usize = 128;

/// Validate a session ID: max 128 chars, `[a-zA-Z0-9\-_]` only.
pub(crate) fn validate_session_id(value: &str) -> Option<String> {
    if value.is_empty() || value.len() > MAX_SESSION_ID_LEN {
        return None;
    }
    if value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        Some(value.to_string())
    } else {
        None
    }
}

/// Build a [`DebugInfo`] from the routing data collected during request processing.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_debug_info(
    model: &str,
    tier: &str,
    score: f64,
    profile: &str,
    provider: &str,
    cache_status: CacheStatus,
    latency_ms: u64,
    payment_status: PaymentStatus,
    token_estimate_in: u32,
    token_estimate_out: u32,
) -> DebugInfo {
    DebugInfo {
        model: model.to_string(),
        tier: tier.to_string(),
        score,
        profile: profile.to_string(),
        provider: provider.to_string(),
        cache_status,
        latency_ms,
        payment_status,
        token_estimate_in,
        token_estimate_out,
    }
}

/// Build a session token for the given wallet, valid for 1 hour.
///
/// Returns `None` if token creation fails -- callers should silently skip the
/// header rather than failing the request.
pub(crate) fn build_session_token(wallet: &str, secret: &[u8]) -> Option<String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs();

    let claims = crate::session::SessionClaims {
        wallet: wallet.to_string(),
        budget_remaining: 0, // exact-scheme payment -- no remaining budget
        issued_at: now,
        expires_at: now + 3600, // 1 hour
        allowed_models: vec![], // all models allowed
    };

    crate::session::create_session_token(&claims, secret).ok()
}

/// Attach the `X-Session-Id` response header if a valid session ID was provided.
pub(crate) fn attach_session_id(resp: &mut Response, session_id: &Option<String>) {
    if let Some(sid) = session_id {
        if let Ok(hv) = HeaderValue::from_str(sid) {
            resp.headers_mut()
                .insert(HeaderName::from_static("x-session-id"), hv);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // validate_session_id
    // =========================================================================

    #[test]
    fn test_validate_session_id_valid_alphanumeric() {
        assert_eq!(
            validate_session_id("abc123"),
            Some("abc123".to_string()),
            "alphanumeric session ID should be accepted"
        );
    }

    #[test]
    fn test_validate_session_id_valid_with_dashes_and_underscores() {
        assert_eq!(
            validate_session_id("my-session_id-123"),
            Some("my-session_id-123".to_string()),
            "dashes and underscores should be accepted"
        );
    }

    #[test]
    fn test_validate_session_id_empty_rejected() {
        assert_eq!(
            validate_session_id(""),
            None,
            "empty session ID should be rejected"
        );
    }

    #[test]
    fn test_validate_session_id_oversized_rejected() {
        let long_id = "a".repeat(129);
        assert_eq!(
            validate_session_id(&long_id),
            None,
            "session ID > 128 chars should be rejected"
        );
    }

    #[test]
    fn test_validate_session_id_exactly_128_chars_accepted() {
        let id = "a".repeat(128);
        assert_eq!(
            validate_session_id(&id),
            Some(id),
            "session ID of exactly 128 chars should be accepted"
        );
    }

    #[test]
    fn test_validate_session_id_invalid_chars_rejected() {
        assert_eq!(
            validate_session_id("has spaces"),
            None,
            "spaces should be rejected"
        );
        assert_eq!(
            validate_session_id("has!special@chars"),
            None,
            "special characters should be rejected"
        );
        assert_eq!(
            validate_session_id("path/traversal"),
            None,
            "slashes should be rejected"
        );
    }

    // =========================================================================
    // attach_session_id
    // =========================================================================

    #[test]
    fn test_attach_session_id_present() {
        let mut resp = axum::http::Response::builder()
            .status(200)
            .body(axum::body::Body::empty())
            .unwrap();
        let session_id = Some("test-session".to_string());
        attach_session_id(&mut resp, &session_id);
        assert_eq!(
            resp.headers()
                .get("x-session-id")
                .unwrap()
                .to_str()
                .unwrap(),
            "test-session"
        );
    }

    #[test]
    fn test_attach_session_id_absent() {
        let mut resp = axum::http::Response::builder()
            .status(200)
            .body(axum::body::Body::empty())
            .unwrap();
        attach_session_id(&mut resp, &None);
        assert!(
            resp.headers().get("x-session-id").is_none(),
            "x-session-id should not be set when session_id is None"
        );
    }

    // =========================================================================
    // build_session_token
    // =========================================================================

    #[test]
    fn test_build_session_token_returns_valid_token() {
        let secret = b"test-session-secret-32-bytes!!!!";
        let token = build_session_token("7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU", secret);
        assert!(token.is_some(), "should produce a token");

        let token_str = token.unwrap();
        assert_eq!(
            token_str.matches('.').count(),
            1,
            "token should have exactly one dot separator"
        );

        let claims =
            crate::session::verify_session_token(&token_str, secret).expect("token should verify");
        assert_eq!(
            claims.wallet,
            "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"
        );
        assert_eq!(claims.budget_remaining, 0);
        assert!(claims.allowed_models.is_empty());
        assert!(claims.expires_at > claims.issued_at);
        assert_eq!(claims.expires_at - claims.issued_at, 3600);
    }

    #[test]
    fn test_build_session_token_with_empty_secret() {
        let token = build_session_token("wallet123", b"");
        assert!(token.is_some());
    }
}
