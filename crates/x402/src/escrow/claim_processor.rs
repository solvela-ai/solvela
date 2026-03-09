//! Background task that processes pending escrow claims from the queue.
//!
//! Polls the `escrow_claim_queue` table at a configurable interval, picks up
//! pending claims, submits them on-chain via [`do_claim_with_params`], and
//! marks them as completed or failed in the database.

use std::sync::Arc;
use std::time::Duration;

use tracing::{info, warn};

use super::claim_queue;
use super::claimer::{do_claim_with_params, EscrowClaimer};
use super::pda::decode_bs58_pubkey;

/// Start the background claim processor. Polls every `poll_interval`.
///
/// Returns the [`tokio::task::JoinHandle`] so the caller can optionally
/// await shutdown. In practice the handle is dropped (fire-and-forget).
pub fn start_claim_processor(
    pool: sqlx::PgPool,
    claimer: Arc<EscrowClaimer>,
    poll_interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(poll_interval);
        info!(
            poll_interval_secs = poll_interval.as_secs(),
            "claim processor started"
        );
        loop {
            interval.tick().await;
            if let Err(e) = process_pending_claims(&pool, &claimer).await {
                warn!(error = %e, "claim processor cycle failed");
            }
        }
    })
}

/// Process all pending claims in a single cycle.
///
/// Fetches up to 10 pending claims, marks each as in-progress, attempts the
/// on-chain claim, and records the outcome (completed with tx signature, or
/// failed with error message).
async fn process_pending_claims(
    pool: &sqlx::PgPool,
    claimer: &EscrowClaimer,
) -> Result<(), String> {
    let pending = claim_queue::fetch_pending_claims(pool, 10)
        .await
        .map_err(|e| format!("failed to fetch pending claims: {e}"))?;

    if pending.is_empty() {
        return Ok(());
    }

    info!(count = pending.len(), "processing pending escrow claims");

    for entry in &pending {
        // Mark in-progress before attempting
        if let Err(e) = claim_queue::mark_in_progress(pool, &entry.id).await {
            warn!(
                claim_id = %entry.id,
                error = %e,
                "failed to mark claim in_progress, skipping"
            );
            continue;
        }

        // Decode agent pubkey from base58
        let agent_bytes = match decode_bs58_pubkey(&entry.agent_pubkey) {
            Ok(bytes) => bytes,
            Err(e) => {
                let error_msg = format!("invalid agent pubkey: {e}");
                warn!(claim_id = %entry.id, error = %error_msg, "skipping claim");
                let _ = claim_queue::mark_attempt_failed(pool, &entry.id, &error_msg).await;
                continue;
            }
        };

        // Submit on-chain claim
        match do_claim_with_params(claimer, entry.service_id, agent_bytes, entry.claim_amount).await
        {
            Ok(tx_sig) => {
                info!(
                    claim_id = %entry.id,
                    tx_signature = %tx_sig,
                    amount = entry.claim_amount,
                    "escrow claim completed"
                );
                if let Err(e) = claim_queue::mark_completed(pool, &entry.id, &tx_sig).await {
                    warn!(
                        claim_id = %entry.id,
                        error = %e,
                        "failed to mark claim completed"
                    );
                }
            }
            Err(e) => {
                let error_msg = e.to_string();
                warn!(
                    claim_id = %entry.id,
                    error = %error_msg,
                    attempt = entry.attempts + 1,
                    "escrow claim attempt failed"
                );
                if let Err(db_err) =
                    claim_queue::mark_attempt_failed(pool, &entry.id, &error_msg).await
                {
                    warn!(
                        claim_id = %entry.id,
                        error = %db_err,
                        "failed to record claim failure"
                    );
                }
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_start_claim_processor_compiles() {
        // Verify the public API compiles correctly.
        // Actual processing requires a live PgPool + EscrowClaimer,
        // which are covered by integration tests.
        let _: fn(sqlx::PgPool, Arc<EscrowClaimer>, Duration) -> tokio::task::JoinHandle<()> =
            start_claim_processor;
    }
}
