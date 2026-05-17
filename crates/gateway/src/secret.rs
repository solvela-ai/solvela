//! Secret-bearing types for the gateway.
//!
//! Wraps secrets in newtypes that:
//! - Redact the value in `Debug` output
//! - Zeroize the underlying bytes on drop
//! - Expose only the narrow operation the secret is meant for (constant-time
//!   compare for bearer tokens, HMAC computation for keying secrets)
//!
//! These are belt-and-braces defenses against accidental log leakage,
//! heap dumps, and timing side-channels. See issue #173 (L1 GW, L4 GW).

use std::fmt;

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::security::constant_time_eq;

/// The admin token used to gate operator-only endpoints (`/v1/escrow/admin`,
/// `/metrics`, sensitive health subroutes).
///
/// Stored as `Vec<u8>` rather than `String` so the bytes can be zeroized
/// deterministically on drop without relying on `String`'s capacity. The
/// `Debug` impl redacts the value so the token never appears in panic
/// output, structured-log dumps, or accidental `dbg!` calls. Equality is
/// only exposed via the constant-time `verify` method — `PartialEq` is
/// intentionally omitted to make the timing-safe path the only path.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct AdminToken(Vec<u8>);

impl AdminToken {
    /// Wrap a string token. The original `String` is consumed; the only
    /// remaining copy of the bytes lives inside the `AdminToken`.
    #[must_use]
    pub fn new(token: String) -> Self {
        Self(token.into_bytes())
    }

    /// Constant-time comparison against a candidate byte slice. Returns
    /// `true` iff the candidate equals the wrapped token. The compare always
    /// iterates `max(self.len(), candidate.len())` bytes, so an attacker
    /// cannot infer prefix matches or correct length from response timing.
    #[must_use]
    pub fn verify(&self, candidate: &[u8]) -> bool {
        constant_time_eq(&self.0, candidate)
    }

    /// True if the wrapped token is empty. Operators who set
    /// `SOLVELA_ADMIN_TOKEN=` (no value) almost certainly mean "not
    /// configured" — callers should normalize that case to `None` rather
    /// than accept an empty-string match.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Debug for AdminToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("AdminToken").field(&"[REDACTED]").finish()
    }
}

/// Server-side keying secret for `HMAC-SHA256(secret, message)`.
///
/// Used to keying-hash API keys at rest so a stolen `api_keys.key_hash`
/// column dump alone is not sufficient to brute-force a candidate key — the
/// attacker also needs the server secret. See issue #173 (L1 GW).
///
/// Like [`AdminToken`], stored as `Vec<u8>` with `ZeroizeOnDrop` and a
/// redacting `Debug` impl. The only exposed operation is `hmac_sha256_hex`,
/// which computes the hex digest of the HMAC-SHA256 of a message under this
/// secret. `PartialEq` is intentionally omitted — there's no caller use case
/// for comparing two secrets, and adding the impl would invite misuse.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct HmacSecret(Vec<u8>);

impl HmacSecret {
    /// Wrap raw secret bytes. The caller is responsible for ensuring the
    /// bytes have at least 128 bits of entropy — NIST SP 800-107 recommends
    /// at least 16 bytes for HMAC keys. Any length is accepted, but a
    /// too-short secret defeats the purpose.
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// Compute `HMAC-SHA256(self, message)` and return the hex-encoded digest
    /// (64 chars, lowercase). The output shape matches the existing plain
    /// `SHA-256` hex digest stored in `api_keys.key_hash`, so the same column
    /// holds either form during the migration period.
    #[must_use]
    pub fn hmac_sha256_hex(&self, message: &[u8]) -> String {
        // `Hmac::new_from_slice` accepts any key length; per the type's
        // documentation it never returns Err for `Hmac<Sha256>` (the bound
        // is `KeyInit`, which is infallible for fixed-output MACs).
        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.0).expect("HMAC-SHA256 accepts any key length");
        mac.update(message);
        hex::encode(mac.finalize().into_bytes())
    }
}

impl fmt::Debug for HmacSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("HmacSecret").field(&"[REDACTED]").finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verify_matches_exact_token() {
        let token = AdminToken::new("hunter2".to_string());
        assert!(token.verify(b"hunter2"));
    }

    #[test]
    fn verify_rejects_wrong_token() {
        let token = AdminToken::new("hunter2".to_string());
        assert!(!token.verify(b"hunter3"));
    }

    #[test]
    fn verify_rejects_prefix() {
        // The wrapped token is longer than the candidate.
        let token = AdminToken::new("hunter2-extra".to_string());
        assert!(!token.verify(b"hunter2"));
    }

    #[test]
    fn verify_rejects_suffix() {
        // The candidate is longer than the wrapped token.
        let token = AdminToken::new("hunter2".to_string());
        assert!(!token.verify(b"hunter2-extra"));
    }

    #[test]
    fn verify_empty_matches_empty() {
        let token = AdminToken::new(String::new());
        assert!(token.verify(b""));
    }

    #[test]
    fn debug_redacts_value() {
        let token = AdminToken::new("topsecret123".to_string());
        let debug_output = format!("{token:?}");
        assert!(
            !debug_output.contains("topsecret123"),
            "Debug must not leak the token: got {debug_output}"
        );
        assert!(
            debug_output.contains("REDACTED"),
            "Debug must mark value as REDACTED: got {debug_output}"
        );
    }

    #[test]
    fn is_empty_reflects_inner_length() {
        let empty = AdminToken::new(String::new());
        let non_empty = AdminToken::new("x".to_string());
        assert!(empty.is_empty());
        assert!(!non_empty.is_empty());
    }

    // -----------------------------------------------------------------------
    // HmacSecret
    // -----------------------------------------------------------------------

    #[test]
    fn hmac_secret_is_deterministic_for_same_input() {
        let secret = HmacSecret::new(b"server-side-secret".to_vec());
        let h1 = secret.hmac_sha256_hex(b"solvela_k_abc123");
        let h2 = secret.hmac_sha256_hex(b"solvela_k_abc123");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64, "SHA-256 hex digest must be 64 chars");
        // Lowercase hex only — matches the existing `hex::encode` output.
        assert!(h1
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn hmac_secret_differs_per_message() {
        let secret = HmacSecret::new(b"server-side-secret".to_vec());
        let a = secret.hmac_sha256_hex(b"solvela_k_one");
        let b = secret.hmac_sha256_hex(b"solvela_k_two");
        assert_ne!(a, b);
    }

    #[test]
    fn hmac_secret_differs_per_key() {
        let s1 = HmacSecret::new(b"secret-one".to_vec());
        let s2 = HmacSecret::new(b"secret-two".to_vec());
        let msg = b"solvela_k_abc";
        // Different keys must produce different MACs for the same message —
        // this is the load-bearing property: a stolen DB dump alone is not
        // enough to verify candidate keys without the secret.
        assert_ne!(s1.hmac_sha256_hex(msg), s2.hmac_sha256_hex(msg));
    }

    #[test]
    fn hmac_secret_known_test_vector() {
        // RFC 4231 § 4.2 — HMAC-SHA-256 test vector 1:
        //   Key:  20 bytes of 0x0b
        //   Data: "Hi There"
        //   HMAC: b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7
        let secret = HmacSecret::new(vec![0x0b; 20]);
        let got = secret.hmac_sha256_hex(b"Hi There");
        assert_eq!(
            got,
            "b0344c61d8db38535ca8afceaf0bf12b881dc200c9833da726e9376c2e32cff7"
        );
    }

    #[test]
    fn hmac_secret_debug_redacts_value() {
        let secret = HmacSecret::new(b"correct-horse-battery-staple".to_vec());
        let debug_output = format!("{secret:?}");
        assert!(
            !debug_output.contains("correct-horse"),
            "Debug must not leak the secret: got {debug_output}"
        );
        assert!(
            debug_output.contains("REDACTED"),
            "Debug must mark value as REDACTED: got {debug_output}"
        );
    }
}
