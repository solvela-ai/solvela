//! Shared Solana JSON-RPC helpers used by payment verifiers.
//!
//! This module centralizes `sendTransaction`, `getSignatureStatuses`, and
//! confirmation polling so both `SolanaVerifier` (direct) and `EscrowVerifier`
//! use the exact same behavior.

use std::time::Duration;

use reqwest::Client;
use tracing::{debug, info, warn};

use crate::traits::Error;
use crate::types::SettlementFailureKind;

/// Submit a base64-encoded signed transaction to Solana RPC.
///
/// Returns the base58 signature string from the RPC response. Retries on the
/// RPC side (maxRetries: 3). Callers should handle the "already processed"
/// idempotency case via `is_already_processed_error`.
pub async fn send_transaction(
    client: &Client,
    rpc_url: &str,
    base64_tx: &str,
) -> Result<String, Error> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "sendTransaction",
        "params": [
            base64_tx,
            {
                "encoding": "base64",
                "skipPreflight": false,
                "preflightCommitment": "confirmed",
                "maxRetries": 3,
            }
        ],
    });

    let response = client
        .post(rpc_url)
        .json(&body)
        .send()
        .await
        .map_err(|e| Error::Rpc(e.to_string()))?;

    let result: serde_json::Value = response
        .json()
        .await
        .map_err(|e| Error::Rpc(e.to_string()))?;

    if let Some(error) = result.get("error") {
        return Err(Error::Rpc(error.to_string()));
    }

    result
        .get("result")
        .and_then(|r| r.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| Error::Rpc("sendTransaction did not return a signature".to_string()))
}

/// Check if an error from `send_transaction` indicates the transaction is
/// already on-chain (idempotency — a successful resubmission).
///
/// Covers known variants: "already processed" (lowercase/titlecase/spaced) and
/// the `AlreadyProcessed` enum form.
///
/// NB: this deliberately does NOT match on the bare JSON-RPC code `-32002`.
/// `-32002` is `SendTransactionPreflightFailure`, the umbrella code for ALL
/// preflight rejections — including deterministic program errors such as
/// `ConstraintAddress` (2012). Treating any `-32002` as "already processed"
/// made the gateway swallow a hard rejection as an idempotent resubmission,
/// then dead-end in the 30s confirmation poll and report it as a transient
/// "could not be confirmed, please retry" timeout (issue #435). A genuine
/// already-processed response always carries the "already processed" /
/// "already been processed" message text, which the matches above cover.
pub fn is_already_processed_error(err: &Error) -> bool {
    let lower = err.to_string().to_lowercase();
    lower.contains("already processed")
        || lower.contains("already been processed")
        || lower.contains("alreadyprocessed")
}

/// Classify a raw settlement error string into a retry verdict.
///
/// Pure (no I/O). The input is whatever error text the settlement path
/// produced — a `send_transaction` preflight error, a `getSignatureStatuses`
/// `err` value, or a confirmation-budget timeout message. The output is the
/// safe, typed [`SettlementFailureKind`] the gateway switches on; this function
/// extracts only the numeric program error code, never raw RPC internals.
pub fn classify_settlement_error(raw: &str) -> SettlementFailureKind {
    // A program error means the program ran and rejected the transaction. The
    // same signed transaction can never confirm on retry. Solana surfaces this
    // two ways: the structured `InstructionError` (JSON / Debug form) and the
    // human-readable "custom program error: 0x..." preflight message.
    if raw.contains("InstructionError") || raw.contains("custom program error") {
        return SettlementFailureKind::Rejected {
            program_error_code: extract_program_error_code(raw),
        };
    }
    // Confirmation budget exhausted with no on-chain error observed — the
    // blockhash may simply have been slow to land. Worth a retry.
    if raw.contains("not confirmed within") {
        return SettlementFailureKind::Timeout;
    }
    // Everything else — RPC transport failure, expired blockhash, generic send
    // failure. Transient, so the client may retry.
    SettlementFailureKind::Submission
}

/// Extract the numeric on-chain program error code from a raw error string.
///
/// Handles the structured JSON form (`"Custom":2012`), the Debug form
/// (`Custom(2012)`), and the preflight text form (`custom program error: 0x7dc`).
/// Returns `None` when the instruction error is non-`Custom` (e.g. a builtin
/// like `InsufficientFunds`) or no code can be parsed. The markers are anchored
/// (`"Custom":` / `Custom(`) rather than a bare `Custom` substring so an
/// unrelated occurrence of the word can't anchor the scan on the wrong digits.
fn extract_program_error_code(raw: &str) -> Option<u32> {
    // Form 1: structured/Debug — `"Custom":2012` or `Custom(2012)`.
    for marker in ["\"Custom\":", "Custom("] {
        if let Some(idx) = raw.find(marker) {
            let digits: String = raw[idx + marker.len()..]
                .chars()
                .skip_while(|c| c.is_whitespace())
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(code) = digits.parse::<u32>() {
                return Some(code);
            }
        }
    }
    // Form 2: `custom program error: 0x7dc` — hex run after the marker.
    const HEX_MARKER: &str = "custom program error: 0x";
    if let Some(idx) = raw.find(HEX_MARKER) {
        let hex: String = raw[idx + HEX_MARKER.len()..]
            .chars()
            .take_while(|c| c.is_ascii_hexdigit())
            .collect();
        if let Ok(code) = u32::from_str_radix(&hex, 16) {
            return Some(code);
        }
    }
    None
}

/// Poll `getSignatureStatuses` until the transaction reaches `confirmed` or
/// `finalized` status, or until the budget expires.
///
/// `processed` is intentionally NOT accepted: it means the tx has been
/// observed by a single validator but is not yet in a quorum-voted bank
/// and can still be rolled back. Accepting `processed` for payment
/// settlement risks returning success to the gateway before the on-chain
/// transfer is durable. `confirmed` (2/3 stake voted) is the minimum that
/// won't roll back under normal cluster operation.
///
/// Uses exponential backoff (500ms → 4s cap). Treats transient RPC errors
/// as retryable (does NOT abort the polling loop on network blips).
///
/// Returns:
/// - `Ok(())` if confirmed/finalized
/// - `Err(Error::SettlementFailed)` if the tx landed with an error
/// - `Err(Error::SettlementFailed("not confirmed within ..."))` if not
///   confirmed within budget
pub async fn poll_for_confirmation(
    client: &Client,
    rpc_url: &str,
    signature_b58: &str,
    budget: Duration,
) -> Result<(), Error> {
    let start = tokio::time::Instant::now();
    let mut interval = Duration::from_millis(500);
    let max_interval = Duration::from_secs(4);
    let mut attempt: u32 = 0;

    loop {
        if start.elapsed() > budget {
            return Err(Error::SettlementFailed(format!(
                "transaction not confirmed within {budget:?}"
            )));
        }

        if attempt > 0 {
            tokio::time::sleep(interval).await;
            interval = (interval * 2).min(max_interval);
        }
        attempt += 1;

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getSignatureStatuses",
            "params": [[signature_b58]],
        });

        // Treat RPC/network errors as transient — keep polling
        let response = match client.post(rpc_url).json(&body).send().await {
            Ok(r) => r,
            Err(e) => {
                debug!(error = %e, attempt, "getSignatureStatuses RPC error, retrying");
                continue;
            }
        };

        let result: serde_json::Value = match response.json().await {
            Ok(j) => j,
            Err(e) => {
                debug!(error = %e, attempt, "getSignatureStatuses JSON parse error, retrying");
                continue;
            }
        };

        if result.get("error").is_some() {
            debug!(error = ?result.get("error"), attempt, "RPC-level error, retrying");
            continue;
        }

        let Some(status_arr) = result
            .get("result")
            .and_then(|r| r.get("value"))
            .and_then(|v| v.as_array())
        else {
            continue;
        };

        let Some(status) = status_arr.first() else {
            continue;
        };

        if status.is_null() {
            // Not yet found — keep polling
            continue;
        }

        if let Some(err_val) = status.get("err") {
            if !err_val.is_null() {
                return Err(Error::SettlementFailed(format!(
                    "transaction failed on-chain: {err_val}"
                )));
            }
        }

        if let Some(confirmation) = status.get("confirmationStatus").and_then(|s| s.as_str()) {
            match confirmation {
                "confirmed" | "finalized" => {
                    info!(
                        signature = signature_b58,
                        status = confirmation,
                        attempt,
                        "transaction confirmed"
                    );
                    return Ok(());
                }
                "processed" => {
                    // Tx is in a leader's bank but not yet quorum-voted —
                    // keep polling until it reaches `confirmed`.
                }
                other => {
                    warn!(
                        status = other,
                        signature = signature_b58,
                        "unknown confirmationStatus from RPC"
                    );
                }
            }
        }
    }
}

/// Fetch the current confirmed slot via Solana JSON-RPC `getSlot`.
///
/// Used by escrow verification to enforce a minimum buffer between the
/// claimed `expiry_slot` in a deposit instruction and the slot at which
/// the gateway is verifying — see `EscrowVerifier::verify_payment` and
/// the matching on-chain `MIN_EXPIRY_BUFFER` guard in
/// `programs/escrow/src/instructions/deposit.rs`.
pub async fn get_current_slot(client: &Client, rpc_url: &str) -> Result<u64, Error> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getSlot",
        "params": [{"commitment": "confirmed"}],
    });

    let response = client
        .post(rpc_url)
        .json(&body)
        .send()
        .await
        .map_err(|e| Error::Rpc(e.to_string()))?;

    let result: serde_json::Value = response
        .json()
        .await
        .map_err(|e| Error::Rpc(e.to_string()))?;

    if let Some(error) = result.get("error") {
        return Err(Error::Rpc(error.to_string()));
    }

    result
        .get("result")
        .and_then(|r| r.as_u64())
        .ok_or_else(|| Error::Rpc("getSlot did not return a u64 result".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::traits::Error;

    #[test]
    fn test_is_already_processed_error_lowercase() {
        let e = Error::Rpc("transaction has already processed".to_string());
        assert!(is_already_processed_error(&e));
    }

    #[test]
    fn test_is_already_processed_error_titlecase() {
        let e = Error::Rpc("Transaction has already been processed".to_string());
        assert!(is_already_processed_error(&e));
    }

    #[test]
    fn test_is_already_processed_error_enum_variant() {
        let e = Error::Rpc("AlreadyProcessed".to_string());
        assert!(is_already_processed_error(&e));
    }

    #[test]
    fn test_is_already_processed_error_jsonrpc_code_with_text() {
        // A genuine already-processed response carries the message text even
        // when it also carries the -32002 code — that text is what we match on.
        let e = Error::Rpc(
            r#"{"code":-32002,"message":"Transaction simulation failed: This transaction has already been processed"}"#
                .to_string(),
        );
        assert!(is_already_processed_error(&e));
    }

    #[test]
    fn test_is_already_processed_error_bare_code_is_not_already_processed() {
        // Bare -32002 (SendTransactionPreflightFailure) must NOT be treated as
        // already-processed — it is the umbrella code for deterministic program
        // rejections like ConstraintAddress. Regression guard for issue #435.
        let e = Error::Rpc(
            r#"{"code":-32002,"message":"Transaction simulation failed: Error processing Instruction 0: custom program error: 0x7dc","data":{"err":{"InstructionError":[0,{"Custom":2012}]}}}"#
                .to_string(),
        );
        assert!(!is_already_processed_error(&e));
    }

    #[test]
    fn test_is_already_processed_error_unrelated() {
        let e = Error::Rpc("blockhash not found".to_string());
        assert!(!is_already_processed_error(&e));
    }

    #[test]
    fn classify_preflight_custom_program_error_is_rejected() {
        // Real shape of a mainnet preflight ConstraintAddress rejection: carries
        // both the "custom program error: 0x7dc" text and the structured
        // InstructionError/Custom(2012). 0x7dc == 2012.
        let raw = r#"submission failed: rpc error: {"code":-32002,"message":"Transaction simulation failed: Error processing Instruction 0: custom program error: 0x7dc","data":{"err":{"InstructionError":[0,{"Custom":2012}]}}}"#;
        assert_eq!(
            classify_settlement_error(raw),
            SettlementFailureKind::Rejected {
                program_error_code: Some(2012)
            }
        );
    }

    #[test]
    fn classify_landed_on_chain_error_is_rejected() {
        // A tx that landed and then failed execution (getSignatureStatuses err).
        let raw = r#"transaction failed on-chain: {"InstructionError":[0,{"Custom":6001}]}"#;
        assert_eq!(
            classify_settlement_error(raw),
            SettlementFailureKind::Rejected {
                program_error_code: Some(6001)
            }
        );
    }

    #[test]
    fn classify_builtin_instruction_error_has_no_code() {
        // Non-Custom builtin errors still classify as Rejected, just without a code.
        let raw =
            r#"transaction failed on-chain: {"InstructionError":[1,"InsufficientFundsForRent"]}"#;
        assert_eq!(
            classify_settlement_error(raw),
            SettlementFailureKind::Rejected {
                program_error_code: None
            }
        );
    }

    #[test]
    fn classify_debug_form_custom_code_is_extracted() {
        // Rust Debug rendering of the err (e.g. via `{err:?}`): `Custom(2012)`.
        let raw = "transaction failed on-chain: InstructionError(0, Custom(2012))";
        assert_eq!(
            classify_settlement_error(raw),
            SettlementFailureKind::Rejected {
                program_error_code: Some(2012)
            }
        );
    }

    #[test]
    fn classify_custom_as_string_value_yields_no_spurious_code() {
        // "Custom" appearing as a string value (not the `"Custom":` key / `Custom(`
        // form) must not anchor the scan onto unrelated digits — code stays None,
        // still classified Rejected via the InstructionError marker.
        let raw = r#"transaction failed on-chain: {"InstructionError":[7,"Custom"]}"#;
        assert_eq!(
            classify_settlement_error(raw),
            SettlementFailureKind::Rejected {
                program_error_code: None
            }
        );
    }

    #[test]
    fn classify_hex_only_program_error_is_rejected_with_code() {
        // Only the text form present (no structured InstructionError) — extract hex.
        let raw = "submission failed: Error processing Instruction 0: custom program error: 0x1771";
        assert_eq!(
            classify_settlement_error(raw),
            SettlementFailureKind::Rejected {
                program_error_code: Some(6001)
            }
        );
    }

    #[test]
    fn classify_confirmation_timeout_is_timeout() {
        let raw = "settlement failed: transaction not confirmed within 30s";
        assert_eq!(
            classify_settlement_error(raw),
            SettlementFailureKind::Timeout
        );
    }

    #[test]
    fn classify_transport_failure_is_submission() {
        // Expired blockhash / RPC blip — genuinely retryable, not a rejection.
        let raw = "submission failed: rpc error: blockhash not found";
        assert_eq!(
            classify_settlement_error(raw),
            SettlementFailureKind::Submission
        );
    }
}
