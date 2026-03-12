//! Durable escrow claim queue backed by PostgreSQL.
//!
//! Persists escrow claims to the `escrow_claim_queue` table for reliable
//! delivery with automatic retry. Each claim starts as [`ClaimStatus::Pending`],
//! transitions to [`ClaimStatus::InProgress`] when picked up by the background
//! worker, and settles as either [`ClaimStatus::Completed`] or
//! [`ClaimStatus::Failed`] after [`MAX_CLAIM_ATTEMPTS`] exhausted retries.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Lifecycle status of an escrow claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

impl ClaimStatus {
    /// Convert from the text representation stored in PostgreSQL.
    fn from_db(s: &str) -> Self {
        match s {
            "in_progress" => Self::InProgress,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            _ => Self::Pending,
        }
    }
}

/// A single row in the `escrow_claim_queue` table.
#[derive(Debug, Clone)]
pub struct ClaimEntry {
    pub id: String,
    pub service_id: [u8; 32],
    pub agent_pubkey: String,
    pub claim_amount: u64,
    pub deposited_amount: Option<u64>,
    pub status: ClaimStatus,
    pub attempts: i32,
    pub tx_signature: Option<String>,
    pub error_message: Option<String>,
}

/// Maximum number of submission attempts before a claim is marked as permanently
/// [`ClaimStatus::Failed`].
pub const MAX_CLAIM_ATTEMPTS: i32 = 10;

/// Maximum backoff duration (5 minutes) in seconds.
const MAX_BACKOFF_SECS: u64 = 300;

/// Compute exponential backoff duration for the given attempt number.
///
/// Schedule: 1s, 2s, 4s, 8s, 16s, 32s, 64s, 128s, 256s, 300s (capped).
pub fn backoff_duration(attempt: i32) -> std::time::Duration {
    let secs = 1u64
        .checked_shl(attempt.max(0) as u32)
        .unwrap_or(MAX_BACKOFF_SECS)
        .min(MAX_BACKOFF_SECS);
    std::time::Duration::from_secs(secs)
}

// ---------------------------------------------------------------------------
// Queue operations
// ---------------------------------------------------------------------------

/// Enqueue a new claim (persists to DB in pending status).
///
/// Returns the UUID of the newly inserted row.
pub async fn enqueue_claim(
    pool: &sqlx::PgPool,
    service_id: &[u8; 32],
    agent_pubkey: &str,
    claim_amount: u64,
    deposited_amount: Option<u64>,
) -> Result<String, sqlx::Error> {
    let row = sqlx::query_scalar::<_, String>(
        "INSERT INTO escrow_claim_queue (service_id, agent_pubkey, claim_amount, deposited_amount)
         VALUES ($1, $2, $3, $4) RETURNING id::text",
    )
    .bind(service_id.as_slice())
    .bind(agent_pubkey)
    .bind(claim_amount as i64)
    .bind(deposited_amount.map(|v| v as i64))
    .fetch_one(pool)
    .await?;
    Ok(row)
}

/// Mark a claim as in-progress (being submitted). Increments the attempt
/// counter and records the timestamp.
pub async fn mark_in_progress(pool: &sqlx::PgPool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE escrow_claim_queue
         SET status = 'in_progress', attempts = attempts + 1, last_attempt_at = NOW()
         WHERE id = $1::uuid",
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark a claim as completed with the on-chain transaction signature.
pub async fn mark_completed(
    pool: &sqlx::PgPool,
    id: &str,
    tx_signature: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE escrow_claim_queue
         SET status = 'completed', tx_signature = $1, completed_at = NOW()
         WHERE id = $2::uuid",
    )
    .bind(tx_signature)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Record a failed attempt. If attempts >= [`MAX_CLAIM_ATTEMPTS`] the claim is
/// marked as permanently `failed`; otherwise it returns to `pending` for retry
/// with an exponential backoff delay stored in `next_retry_at`.
pub async fn mark_attempt_failed(
    pool: &sqlx::PgPool,
    id: &str,
    error: &str,
    current_attempts: i32,
) -> Result<(), sqlx::Error> {
    let backoff = backoff_duration(current_attempts);
    let backoff_secs = backoff.as_secs() as i32;

    sqlx::query(
        "UPDATE escrow_claim_queue
         SET status = CASE
             WHEN attempts >= $1 THEN 'failed'
             ELSE 'pending'
         END,
         error_message = $2,
         last_attempt_at = NOW(),
         next_retry_at = CASE
             WHEN attempts >= $1 THEN next_retry_at
             ELSE NOW() + ($4 || ' seconds')::interval
         END
         WHERE id = $3::uuid",
    )
    .bind(MAX_CLAIM_ATTEMPTS)
    .bind(error)
    .bind(id)
    .bind(backoff_secs.to_string())
    .execute(pool)
    .await?;
    Ok(())
}

/// Fetch pending claims ordered by creation time (oldest first).
///
/// Also recovers stale `in_progress` claims that have been stuck for more
/// than 5 minutes (likely abandoned due to a crash or SIGTERM).
pub async fn fetch_pending_claims(
    pool: &sqlx::PgPool,
    limit: i64,
) -> Result<Vec<ClaimEntry>, sqlx::Error> {
    let rows = sqlx::query_as::<
        _,
        (
            String,
            Vec<u8>,
            String,
            i64,
            Option<i64>,
            String,
            i32,
            Option<String>,
            Option<String>,
        ),
    >(
        "SELECT id::text, service_id, agent_pubkey, claim_amount, deposited_amount,
                status, attempts, tx_signature, error_message
         FROM escrow_claim_queue
         WHERE (status = 'pending' AND (next_retry_at IS NULL OR next_retry_at <= NOW()))
            OR (status = 'in_progress' AND updated_at < NOW() - INTERVAL '5 minutes')
         ORDER BY created_at ASC
         LIMIT $1",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let entries = rows
        .into_iter()
        .map(|r| {
            let mut service_id = [0u8; 32];
            let len = r.1.len().min(32);
            service_id[..len].copy_from_slice(&r.1[..len]);

            ClaimEntry {
                id: r.0,
                service_id,
                agent_pubkey: r.2,
                claim_amount: r.3 as u64,
                deposited_amount: r.4.map(|v| v as u64),
                status: ClaimStatus::from_db(&r.5),
                attempts: r.6,
                tx_signature: r.7,
                error_message: r.8,
            }
        })
        .collect();

    Ok(entries)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_claim_status_serde() {
        // Serialize
        assert_eq!(
            serde_json::to_string(&ClaimStatus::Pending).unwrap(),
            "\"pending\""
        );
        assert_eq!(
            serde_json::to_string(&ClaimStatus::InProgress).unwrap(),
            "\"in_progress\""
        );
        assert_eq!(
            serde_json::to_string(&ClaimStatus::Completed).unwrap(),
            "\"completed\""
        );
        assert_eq!(
            serde_json::to_string(&ClaimStatus::Failed).unwrap(),
            "\"failed\""
        );

        // Deserialize
        let pending: ClaimStatus = serde_json::from_str("\"pending\"").unwrap();
        assert_eq!(pending, ClaimStatus::Pending);

        let in_progress: ClaimStatus = serde_json::from_str("\"in_progress\"").unwrap();
        assert_eq!(in_progress, ClaimStatus::InProgress);

        let completed: ClaimStatus = serde_json::from_str("\"completed\"").unwrap();
        assert_eq!(completed, ClaimStatus::Completed);

        let failed: ClaimStatus = serde_json::from_str("\"failed\"").unwrap();
        assert_eq!(failed, ClaimStatus::Failed);
    }

    #[test]
    fn test_max_attempts_constant() {
        assert_eq!(MAX_CLAIM_ATTEMPTS, 10);
    }

    #[test]
    fn test_backoff_schedule() {
        // 1s, 2s, 4s, 8s, 16s, 32s, 64s, 128s, 256s, 300s (capped)
        let expected_secs = [1, 2, 4, 8, 16, 32, 64, 128, 256, 300];
        for (attempt, &expected) in expected_secs.iter().enumerate() {
            let duration = backoff_duration(attempt as i32);
            assert_eq!(
                duration.as_secs(),
                expected,
                "attempt {attempt}: expected {expected}s, got {}s",
                duration.as_secs()
            );
        }
    }

    #[test]
    fn test_backoff_capped_at_five_minutes() {
        // Even very high attempt numbers should cap at 300s
        assert_eq!(backoff_duration(20).as_secs(), 300);
        assert_eq!(backoff_duration(100).as_secs(), 300);
    }

    #[test]
    fn test_backoff_negative_attempt_treated_as_zero() {
        // Negative attempts should be treated as attempt 0 (1 second)
        assert_eq!(backoff_duration(-1).as_secs(), 1);
    }

    #[test]
    fn test_claim_status_from_db() {
        assert_eq!(ClaimStatus::from_db("pending"), ClaimStatus::Pending);
        assert_eq!(ClaimStatus::from_db("in_progress"), ClaimStatus::InProgress);
        assert_eq!(ClaimStatus::from_db("completed"), ClaimStatus::Completed);
        assert_eq!(ClaimStatus::from_db("failed"), ClaimStatus::Failed);
        // Unknown falls back to Pending
        assert_eq!(ClaimStatus::from_db("unknown"), ClaimStatus::Pending);
    }

    #[test]
    fn test_claim_entry_debug() {
        let entry = ClaimEntry {
            id: "test-id".to_string(),
            service_id: [0u8; 32],
            agent_pubkey: "AgentPubkey123".to_string(),
            claim_amount: 1_000_000,
            deposited_amount: Some(2_000_000),
            status: ClaimStatus::Pending,
            attempts: 0,
            tx_signature: None,
            error_message: None,
        };
        let debug = format!("{entry:?}");
        assert!(debug.contains("ClaimEntry"));
        assert!(debug.contains("AgentPubkey123"));
    }
}
