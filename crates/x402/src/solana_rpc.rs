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

/// Fetch a recent blockhash via Solana JSON-RPC `getLatestBlockhash` at the
/// given `commitment`.
///
/// Returns the 32 raw blockhash bytes (base58-decoded from the RPC's string
/// form), suitable for the `recent_blockhash` field of a legacy message built
/// by [`crate::solana::build_system_transfer_message`].
///
/// `commitment` is the Solana commitment level for the query:
/// - `"finalized"` — maximally durable; the blockhash is well-settled before it
///   is used (the faucet's choice, where the round-trip to `sendTransaction` is
///   the only concern).
/// - `"confirmed"` — fresher and longer-lived (its ~150-slot validity window
///   starts ~32 slots later than a finalized blockhash's), at the cost of a
///   slightly-less-settled reference. Appropriate for an endpoint whose job is to
///   hand back a promptly-submittable transaction (the unsigned-deposit-tx route)
///   and for keeping the blockhash consistent with a `confirmed` slot reference.
///
/// Fails closed: an unparseable / wrong-length blockhash is an [`Error::Rpc`],
/// never a silent zero blockhash (which would build an unsubmittable tx).
pub async fn get_latest_blockhash(
    client: &Client,
    rpc_url: &str,
    commitment: &str,
) -> Result<[u8; 32], Error> {
    let (blockhash, _last_valid_block_height) =
        get_latest_blockhash_and_height(client, rpc_url, commitment).await?;
    Ok(blockhash)
}

/// Fetch a recent blockhash AND its `lastValidBlockHeight` via
/// `getLatestBlockhash` (same call as [`get_latest_blockhash`], which discards
/// the height).
///
/// `lastValidBlockHeight` is the LAST block height at which the returned
/// blockhash is still accepted — the producer for a conclusive-death check on
/// a broadcast transaction: once `getBlockHeight` (same `confirmed`
/// commitment) exceeds it AND a history-searching [`get_signature_status`]
/// finds no trace of the signature, the transaction can never land and it is
/// safe to re-sign. Fails closed on a missing/unparseable field — never a
/// silent zero height (which would declare every transaction instantly dead).
pub async fn get_latest_blockhash_and_height(
    client: &Client,
    rpc_url: &str,
    commitment: &str,
) -> Result<([u8; 32], u64), Error> {
    let body = latest_blockhash_request_body(commitment);

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

    parse_latest_blockhash_response(&result)
}

/// `getLatestBlockhash` request body. Extracted so the wire shape is
/// unit-assertable (round-2 review: an RPC param in the wrong position is the
/// silent class that already bit `poll_for_confirmation`).
pub(crate) fn latest_blockhash_request_body(commitment: &str) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getLatestBlockhash",
        "params": [{"commitment": commitment}],
    })
}

/// Parse a `getLatestBlockhash` response into (blockhash, lastValidBlockHeight),
/// fail-closed on any missing/malformed field (never a silent zero hash or
/// zero height).
pub(crate) fn parse_latest_blockhash_response(
    result: &serde_json::Value,
) -> Result<([u8; 32], u64), Error> {
    let value = result.get("result").and_then(|r| r.get("value"));

    let blockhash_b58 = value
        .and_then(|v| v.get("blockhash"))
        .and_then(|b| b.as_str())
        .ok_or_else(|| {
            Error::Rpc("getLatestBlockhash did not return a blockhash string".to_string())
        })?;

    let bytes = bs58::decode(blockhash_b58)
        .into_vec()
        .map_err(|e| Error::Rpc(format!("blockhash is not valid base58: {e}")))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|v: Vec<u8>| Error::Rpc(format!("blockhash must be 32 bytes, got {}", v.len())))?;

    let last_valid_block_height = value
        .and_then(|v| v.get("lastValidBlockHeight"))
        .and_then(|h| h.as_u64())
        .ok_or_else(|| {
            Error::Rpc("getLatestBlockhash did not return a lastValidBlockHeight".to_string())
        })?;

    Ok((arr, last_valid_block_height))
}

/// Fetch the current block height via `getBlockHeight` at `confirmed`
/// commitment — the reader half of the conclusive-death check (compare against
/// a transaction's `lastValidBlockHeight` from
/// [`get_latest_blockhash_and_height`]).
///
/// Fails closed on a missing/malformed result — never returns a silent 0
/// (which would read every blockhash as still-alive) and never a silent MAX
/// (which would read every transaction as dead).
pub async fn get_block_height(client: &Client, rpc_url: &str) -> Result<u64, Error> {
    let body = block_height_request_body();

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
        .ok_or_else(|| Error::Rpc("getBlockHeight did not return a u64 result".to_string()))
}

/// `getBlockHeight` request body (`confirmed` commitment — must match the
/// commitment of [`latest_blockhash_request_body`]'s producer for the
/// death-check comparison to be meaningful). Extracted for wire-shape tests.
pub(crate) fn block_height_request_body() -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getBlockHeight",
        "params": [{"commitment": "confirmed"}],
    })
}

/// A single transaction's status as reported by `getSignatureStatuses`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignatureStatus {
    /// The on-chain execution error, if the transaction landed AND failed.
    /// Stringified so callers can route it through
    /// [`classify_settlement_error`] without re-exposing raw RPC structures.
    pub err: Option<String>,
    /// `processed` / `confirmed` / `finalized` (or `None` on very old RPC
    /// responses).
    pub confirmation_status: Option<String>,
}

/// One-shot `getSignatureStatuses` lookup for a single signature.
///
/// Returns `Ok(None)` when the RPC has no record of the signature.
///
/// `search_transaction_history` selects which store the RPC consults:
/// - `false` — the recent-status cache ONLY (~150 slots / a few minutes).
///   Cheap; fine for a fresh-confirmation loop like [`poll_for_confirmation`].
/// - `true` — the full ledger history. **REQUIRED for any conclusive-death or
///   crash/restart-recovery check before re-signing a payment**: a transaction
///   that landed longer ago than the recent-cache window reads as `None` under
///   the default form, and treating that as "dead" re-signs an
///   already-landed transfer — a double-send of real funds.
pub async fn get_signature_status(
    client: &Client,
    rpc_url: &str,
    signature_b58: &str,
    search_transaction_history: bool,
) -> Result<Option<SignatureStatus>, Error> {
    let body = signature_status_request_body(signature_b58, search_transaction_history);

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

    parse_signature_status_response(&result)
}

/// `getSignatureStatuses` request body. Extracted so the HALT-4-critical wire
/// shape is unit-pinned: `searchTransactionHistory` MUST ride the config
/// object in params[1] — the bare `[[sig]]` form silently consults only the
/// ~150-slot recent cache, the exact double-refund class that already bit
/// `poll_for_confirmation`.
pub(crate) fn signature_status_request_body(
    signature_b58: &str,
    search_transaction_history: bool,
) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getSignatureStatuses",
        "params": [
            [signature_b58],
            {"searchTransactionHistory": search_transaction_history},
        ],
    })
}

/// Parse a `getSignatureStatuses` response for a single signature.
/// `Ok(None)` = the RPC has no record; a missing status ARRAY (malformed
/// response) fails closed rather than reading as not-found.
pub(crate) fn parse_signature_status_response(
    result: &serde_json::Value,
) -> Result<Option<SignatureStatus>, Error> {
    let status = result
        .get("result")
        .and_then(|r| r.get("value"))
        .and_then(|v| v.as_array())
        .and_then(|arr| arr.first())
        .ok_or_else(|| {
            Error::Rpc("getSignatureStatuses did not return a status array".to_string())
        })?
        .clone();

    if status.is_null() {
        return Ok(None);
    }

    let err = match status.get("err") {
        Some(e) if !e.is_null() => Some(e.to_string()),
        _ => None,
    };
    let confirmation_status = status
        .get("confirmationStatus")
        .and_then(|s| s.as_str())
        .map(|s| s.to_string());

    Ok(Some(SignatureStatus {
        err,
        confirmation_status,
    }))
}

/// Fetch a wallet's native SOL balance (in lamports) via `getBalance`.
///
/// `pubkey_b58` is the base58 wallet address. Returns the lamport balance as a
/// `u64`. Fails closed on a missing/malformed result — never silently returns 0
/// (a faucet that reads 0 on RPC failure would re-drip an already-funded
/// wallet).
pub async fn get_sol_balance(
    client: &Client,
    rpc_url: &str,
    pubkey_b58: &str,
) -> Result<u64, Error> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getBalance",
        "params": [pubkey_b58, {"commitment": "confirmed"}],
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
        .and_then(|r| r.get("value"))
        .and_then(|v| v.as_u64())
        .ok_or_else(|| Error::Rpc("getBalance did not return a u64 lamport value".to_string()))
}

/// Fetch a wallet's USDC-SPL balance in atomic (6-decimal) units via the
/// owner's associated token account (ATA).
///
/// Derives `ATA(owner, mint)` and queries `getTokenAccountBalance`. A **missing
/// ATA** (the owner has never held this token) is treated as a balance of `0`,
/// matching how the on-chain world sees an unfunded token account — this is the
/// one sanctioned "absent → 0" case, distinct from an RPC/parse failure which
/// fails closed with an [`Error::Rpc`].
///
/// Returns the atomic `u64` amount (micro-USDC). Per the fintech rules, all
/// downstream USDC comparisons stay in integer atomic units — this function
/// never converts to a decimal float.
pub async fn get_usdc_balance(
    client: &Client,
    rpc_url: &str,
    owner_b58: &str,
    mint_b58: &str,
) -> Result<u64, Error> {
    use crate::solana_types::{derive_ata, Pubkey};
    use std::str::FromStr;

    let owner = Pubkey::from_str(owner_b58)
        .map_err(|e| Error::InvalidTransaction(format!("invalid owner pubkey: {e}")))?;
    let mint = Pubkey::from_str(mint_b58)
        .map_err(|e| Error::InvalidTransaction(format!("invalid mint pubkey: {e}")))?;

    let ata = derive_ata(&owner, &mint, &Pubkey::TOKEN_PROGRAM_ID)
        .ok_or_else(|| Error::InvalidTransaction("failed to derive owner USDC ATA".to_string()))?;
    let ata_b58 = ata.to_string();

    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getTokenAccountBalance",
        "params": [ata_b58, {"commitment": "confirmed"}],
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

    // A non-existent ATA returns an RPC error ("could not find account" /
    // "Invalid param: could not find account"). Treat that — and ONLY that —
    // as a zero balance. Any other RPC error fails closed.
    if let Some(error) = result.get("error") {
        let msg = error.to_string().to_lowercase();
        if msg.contains("could not find account") || msg.contains("invalid param") {
            return Ok(0);
        }
        return Err(Error::Rpc(error.to_string()));
    }

    // `amount` is a *string* of atomic units in the RPC response — parse it as
    // an integer; never go through an f64 (`uiAmount`) which would violate the
    // atomic-only money-math rule.
    let amount_str = result
        .get("result")
        .and_then(|r| r.get("value"))
        .and_then(|v| v.get("amount"))
        .and_then(|a| a.as_str())
        .ok_or_else(|| {
            Error::Rpc("getTokenAccountBalance did not return an amount string".to_string())
        })?;

    amount_str
        .parse::<u64>()
        .map_err(|e| Error::Rpc(format!("token amount is not a u64: {e}")))
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

    // -- refund RPC wire shapes (round-2 review item 2) ----------------------

    /// HALT-4 pin: `searchTransactionHistory` rides the params[1] config
    /// object with the exact camelCase key. The bare `[[sig]]` form (recent
    /// cache only) is the double-refund class that already bit
    /// `poll_for_confirmation` — this test breaks if anyone regresses to it.
    #[test]
    fn signature_status_body_pins_search_transaction_history_position() {
        for flag in [true, false] {
            let body = signature_status_request_body("5sig", flag);
            assert_eq!(body["method"], "getSignatureStatuses");
            assert_eq!(body["params"][0], serde_json::json!(["5sig"]));
            assert_eq!(
                body["params"][1]["searchTransactionHistory"],
                serde_json::json!(flag),
                "searchTransactionHistory must be the params[1] config key"
            );
        }
    }

    #[test]
    fn block_height_body_uses_confirmed_commitment() {
        let body = block_height_request_body();
        assert_eq!(body["method"], "getBlockHeight");
        assert_eq!(body["params"][0]["commitment"], "confirmed");
    }

    #[test]
    fn latest_blockhash_body_carries_commitment() {
        let body = latest_blockhash_request_body("confirmed");
        assert_eq!(body["method"], "getLatestBlockhash");
        assert_eq!(body["params"][0]["commitment"], "confirmed");
    }

    #[test]
    fn parse_signature_status_realistic_shapes() {
        // Not found: the RPC returns a literal null entry.
        let not_found = serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {"context": {"slot": 100}, "value": [null]},
        });
        assert_eq!(parse_signature_status_response(&not_found).unwrap(), None);

        // Landed and confirmed, no error.
        let confirmed = serde_json::json!({
            "result": {"value": [{
                "slot": 98, "confirmations": 10, "err": null,
                "confirmationStatus": "confirmed",
            }]},
        });
        assert_eq!(
            parse_signature_status_response(&confirmed).unwrap(),
            Some(SignatureStatus {
                err: None,
                confirmation_status: Some("confirmed".to_string()),
            })
        );

        // Landed WITH an on-chain execution error — err must surface (a
        // consumer that drops it would confirm a refund that moved no USDC).
        let failed = serde_json::json!({
            "result": {"value": [{
                "slot": 98, "confirmations": null,
                "err": {"InstructionError": [1, {"Custom": 1}]},
                "confirmationStatus": "finalized",
            }]},
        });
        let parsed = parse_signature_status_response(&failed).unwrap().unwrap();
        assert!(parsed.err.as_deref().unwrap().contains("InstructionError"));
        assert_eq!(parsed.confirmation_status.as_deref(), Some("finalized"));

        // A MALFORMED response (no value array) fails closed — it must never
        // read as "not found" (which would authorize a re-sign).
        let malformed = serde_json::json!({"result": {}});
        assert!(parse_signature_status_response(&malformed).is_err());
    }

    #[test]
    fn parse_latest_blockhash_fails_closed_without_height() {
        let blockhash_b58 = bs58::encode([7u8; 32]).into_string();
        let ok = serde_json::json!({
            "result": {"value": {
                "blockhash": blockhash_b58,
                "lastValidBlockHeight": 12345,
            }},
        });
        assert_eq!(
            parse_latest_blockhash_response(&ok).unwrap(),
            ([7u8; 32], 12_345)
        );

        // Missing lastValidBlockHeight → Err, never a silent 0 (which would
        // declare every transaction instantly dead → premature re-sign).
        let no_height = serde_json::json!({
            "result": {"value": {"blockhash": bs58::encode([7u8; 32]).into_string()}},
        });
        assert!(parse_latest_blockhash_response(&no_height).is_err());

        // Non-base58 / wrong-length blockhash → Err, never a zero hash.
        let bad_hash = serde_json::json!({
            "result": {"value": {"blockhash": "!!!", "lastValidBlockHeight": 1}},
        });
        assert!(parse_latest_blockhash_response(&bad_hash).is_err());
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
