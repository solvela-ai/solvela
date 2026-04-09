//! Shared Solana JSON-RPC helpers used by payment verifiers.
//!
//! This module centralizes `sendTransaction`, `getSignatureStatuses`, and
//! confirmation polling so both `SolanaVerifier` (direct) and `EscrowVerifier`
//! use the exact same behavior.

use std::time::Duration;

use reqwest::Client;
use tracing::{debug, info, warn};

use crate::traits::Error;

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
/// Covers known variants: "already processed" (lowercase/titlecase/spaced),
/// `AlreadyProcessed` enum form, JSON-RPC code -32002.
pub fn is_already_processed_error(err: &Error) -> bool {
    let s = err.to_string();
    let lower = s.to_lowercase();
    lower.contains("already processed")
        || lower.contains("already been processed")
        || lower.contains("alreadyprocessed")
        || s.contains("-32002")
}

/// Poll `getSignatureStatuses` until the transaction reaches `processed`,
/// `confirmed`, or `finalized` status, or until the budget expires.
///
/// Uses exponential backoff (500ms → 4s cap) matching `SolanaVerifier`'s
/// existing pattern. Treats transient RPC errors as retryable (does NOT abort
/// the polling loop on network blips).
///
/// Returns:
/// - `Ok(())` if confirmed/processed/finalized
/// - `Err(Error::SettlementFailed)` if the tx landed with an error
/// - `Err(Error::SettlementFailed("timeout"))` if not confirmed within budget
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
                "processed" | "confirmed" | "finalized" => {
                    info!(
                        signature = signature_b58,
                        status = confirmation,
                        attempt,
                        "transaction confirmed"
                    );
                    return Ok(());
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
    fn test_is_already_processed_error_jsonrpc_code() {
        let e = Error::Rpc(
            r#"{"code":-32002,"message":"Transaction simulation failed"}"#.to_string(),
        );
        assert!(is_already_processed_error(&e));
    }

    #[test]
    fn test_is_already_processed_error_unrelated() {
        let e = Error::Rpc("blockhash not found".to_string());
        assert!(!is_already_processed_error(&e));
    }
}
