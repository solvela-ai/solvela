//! Secret-bearing types for the gateway.
//!
//! Wraps secrets like the admin token in newtypes that:
//! - Redact the value in `Debug` output
//! - Zeroize the underlying bytes on drop
//! - Force callers through a constant-time comparison helper
//!
//! These are belt-and-braces defenses against accidental log leakage,
//! heap dumps, and timing side-channels. See issue #173 (L4 GW).

use std::fmt;

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
}
