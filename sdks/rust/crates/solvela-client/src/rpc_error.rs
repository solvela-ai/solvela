//! Helpers for discriminating Solana JSON-RPC error responses.
//!
//! Solana RPC's `getTokenAccountBalance` returns a JSON-RPC error with
//! `code = -32602` and a message containing `"could not find account"` when
//! the SPL token account does not exist on chain. That case is benign for
//! balance lookups — a missing ATA means a zero balance, and callers can
//! treat it as `Ok(0)`. Every other error code (rate limit, auth failure,
//! malformed request, internal server error, etc.) is a real failure that
//! must be propagated; silencing them to `Ok(0)` makes a malfunctioning RPC
//! indistinguishable from an empty wallet. See issue #323.

use serde_json::Value;

/// Returns true if the JSON-RPC response represents a missing-account error
/// for `getTokenAccountBalance` (or sibling balance-fetch methods) — that is,
/// `code == -32602` with a message indicating the account does not exist.
///
/// The message-substring check is necessary because `-32602` ("Invalid params")
/// is a catch-all code that Solana RPC also returns for malformed pubkeys,
/// unsupported commitment levels, etc. Only the canonical "could not find
/// account" / "Account does not exist" variants are benign.
pub(crate) fn is_account_not_found(json: &Value) -> bool {
    let Some(error) = json.get("error") else {
        return false;
    };
    let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
    if code != -32602 {
        return false;
    }
    let msg = error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    msg.contains("could not find account") || msg.contains("Account does not exist")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn canonical_account_not_found_message_matches() {
        let json = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": { "code": -32602, "message": "Invalid param: could not find account" }
        });
        assert!(is_account_not_found(&json));
    }

    #[test]
    fn helius_variant_message_matches() {
        let json = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": { "code": -32602, "message": "Account does not exist" }
        });
        assert!(is_account_not_found(&json));
    }

    #[test]
    fn other_neg_32602_does_not_match() {
        let json = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": { "code": -32602, "message": "Invalid param: unrecognized commitment" }
        });
        assert!(!is_account_not_found(&json));
    }

    #[test]
    fn rate_limit_does_not_match() {
        let json = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": { "code": -32005, "message": "Server is busy" }
        });
        assert!(!is_account_not_found(&json));
    }

    #[test]
    fn no_error_field_does_not_match() {
        let json = json!({ "jsonrpc": "2.0", "id": 1, "result": { "value": null } });
        assert!(!is_account_not_found(&json));
    }

    #[test]
    fn missing_code_does_not_match() {
        let json = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": { "message": "could not find account" }
        });
        assert!(!is_account_not_found(&json));
    }
}
