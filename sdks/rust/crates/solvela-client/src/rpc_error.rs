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

/// Parse the USDC balance (UI units) from a `getTokenAccountBalance` response
/// whose top-level `error` has already been handled by the caller.
///
/// Shared by `SolvelaClient::usdc_balance_of` and `BalanceMonitor` so both
/// sites parse identically. Requires `result.value`; prefers the deprecated
/// `uiAmount` float but falls back to the authoritative `amount` (atomic
/// string) + `decimals`, which the RPC always returns for an existing account.
///
/// Returns `Err(message)` — never a silent `0.0` — for a structurally
/// unexpected body (missing `result.value`, missing both balance fields,
/// non-numeric `amount`, out-of-range `decimals`, or a negative balance). A
/// silent zero here would wrongly trip the free-tier fallback and suppress
/// payments. A *real* zero balance (`amount: "0"`) still parses as `Ok(0.0)`.
pub(crate) fn parse_token_balance(json: &Value) -> Result<f64, String> {
    let value = json
        .get("result")
        .and_then(|r| r.get("value"))
        .ok_or_else(|| "RPC response missing result.value (unexpected shape)".to_string())?;

    let ui_amount = if let Some(n) = value.get("uiAmount").and_then(Value::as_f64) {
        n
    } else if let (Some(amount), Some(decimals)) = (
        value.get("amount").and_then(Value::as_str),
        value.get("decimals").and_then(Value::as_u64),
    ) {
        let raw: u64 = amount
            .parse()
            .map_err(|e| format!("invalid token amount '{amount}': {e}"))?;
        let exp = i32::try_from(decimals)
            .map_err(|_| format!("RPC returned out-of-range decimals: {decimals}"))?;
        #[allow(clippy::cast_precision_loss)] // USDC supply fits within the f64 mantissa
        let ui = raw as f64 / 10f64.powi(exp);
        ui
    } else {
        return Err("RPC response missing balance fields (uiAmount/amount)".to_string());
    };

    if ui_amount < 0.0 {
        return Err(format!("RPC returned a negative balance: {ui_amount}"));
    }
    Ok(ui_amount)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_balance_prefers_ui_amount_when_present() {
        // uiAmount disagrees with amount on purpose — uiAmount must win.
        let j = json!({"result": {"value": {"uiAmount": 1.5, "amount": "9999999", "decimals": 6}}});
        assert!((parse_token_balance(&j).unwrap() - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_balance_falls_back_to_amount_when_ui_amount_null() {
        let j =
            json!({"result": {"value": {"uiAmount": null, "amount": "2500000", "decimals": 6}}});
        assert!((parse_token_balance(&j).unwrap() - 2.5).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_balance_real_zero_is_ok_not_error() {
        let j = json!({"result": {"value": {"uiAmount": null, "amount": "0", "decimals": 6}}});
        assert!(parse_token_balance(&j).unwrap().abs() < f64::EPSILON);
    }

    #[test]
    fn parse_balance_non_six_decimals() {
        let j =
            json!({"result": {"value": {"uiAmount": null, "amount": "1000000000", "decimals": 9}}});
        assert!((parse_token_balance(&j).unwrap() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn parse_balance_missing_value_errors() {
        let j = json!({"result": {"context": {"slot": 1}}});
        assert!(parse_token_balance(&j).is_err());
    }

    #[test]
    fn parse_balance_non_numeric_amount_errors() {
        let j = json!({"result": {"value": {"uiAmount": null, "amount": "NaN", "decimals": 6}}});
        assert!(parse_token_balance(&j).is_err());
    }

    #[test]
    fn parse_balance_negative_errors() {
        let j = json!({"result": {"value": {"uiAmount": -1.5}}});
        assert!(parse_token_balance(&j).is_err());
    }

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
