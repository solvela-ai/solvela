//! Integration tests for `solvela_x402::escrow::claim_queue` against real Postgres.
//!
//! Lives in the gateway crate because the queue module is gated behind the
//! `postgres` feature, which gateway already enables. Each `#[sqlx::test]`
//! provisions a fresh per-test database.

use sqlx::PgPool;

use solvela_x402::escrow::claim_queue::{
    enqueue_claim, fetch_pending_claims, mark_attempt_failed, mark_completed, mark_in_progress,
    ClaimStatus, MAX_CLAIM_ATTEMPTS,
};

const SERVICE_ID_A: [u8; 32] = [1u8; 32];
const SERVICE_ID_B: [u8; 32] = [2u8; 32];
const AGENT_A: &str = "9noXzpXnkyEcKF3AeXqUHTdR59V5uvrRBUo9bwsHaByz";
const AGENT_B: &str = "DZNuWFpYwzEAcyaMQzqgbxA4d1f4Yq8EU2YbLPLYxNTw";

// ---------------------------------------------------------------------------
// enqueue_claim
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../../migrations")]
async fn enqueue_claim_inserts_pending_row_with_returned_id(pool: PgPool) {
    let id = enqueue_claim(&pool, &SERVICE_ID_A, AGENT_A, 1_000_000, Some(2_000_000))
        .await
        .expect("enqueue must succeed");
    assert!(!id.is_empty(), "returned id must be non-empty");
    assert!(uuid::Uuid::parse_str(&id).is_ok(), "id must be a UUID");

    let row: (String, Vec<u8>, String, i64, Option<i64>, i32) = sqlx::query_as(
        "SELECT status, service_id, agent_pubkey, claim_amount, deposited_amount, attempts
         FROM escrow_claim_queue WHERE id = $1::uuid",
    )
    .bind(&id)
    .fetch_one(&pool)
    .await
    .expect("read back");

    assert_eq!(row.0, "pending");
    assert_eq!(&row.1[..], &SERVICE_ID_A[..]);
    assert_eq!(row.2, AGENT_A);
    assert_eq!(row.3, 1_000_000);
    assert_eq!(row.4, Some(2_000_000));
    assert_eq!(row.5, 0, "fresh row must start with attempts = 0");
}

#[sqlx::test(migrations = "../../migrations")]
async fn enqueue_claim_persists_null_deposited_amount(pool: PgPool) {
    let id = enqueue_claim(&pool, &SERVICE_ID_A, AGENT_A, 500_000, None)
        .await
        .expect("enqueue");

    let deposited: Option<i64> =
        sqlx::query_scalar("SELECT deposited_amount FROM escrow_claim_queue WHERE id = $1::uuid")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .expect("read");
    assert_eq!(deposited, None, "None must persist as SQL NULL");
}

// ---------------------------------------------------------------------------
// mark_in_progress
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../../migrations")]
async fn mark_in_progress_increments_attempt_and_sets_timestamp(pool: PgPool) {
    let id = enqueue_claim(&pool, &SERVICE_ID_A, AGENT_A, 1, None)
        .await
        .expect("enqueue");

    mark_in_progress(&pool, &id)
        .await
        .expect("mark_in_progress");

    let (status, attempts, last_attempt_at): (String, i32, Option<chrono::DateTime<chrono::Utc>>) =
        sqlx::query_as(
            "SELECT status, attempts, last_attempt_at
             FROM escrow_claim_queue WHERE id = $1::uuid",
        )
        .bind(&id)
        .fetch_one(&pool)
        .await
        .expect("read");

    assert_eq!(status, "in_progress");
    assert_eq!(attempts, 1);
    assert!(last_attempt_at.is_some(), "last_attempt_at must be set");
}

#[sqlx::test(migrations = "../../migrations")]
async fn mark_in_progress_compounds_attempts_across_calls(pool: PgPool) {
    let id = enqueue_claim(&pool, &SERVICE_ID_A, AGENT_A, 1, None)
        .await
        .expect("enqueue");

    for expected in 1..=3 {
        mark_in_progress(&pool, &id).await.expect("mark");
        let attempts: i32 =
            sqlx::query_scalar("SELECT attempts FROM escrow_claim_queue WHERE id = $1::uuid")
                .bind(&id)
                .fetch_one(&pool)
                .await
                .expect("read");
        assert_eq!(attempts, expected, "attempts must increment monotonically");
    }
}

// ---------------------------------------------------------------------------
// mark_completed
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../../migrations")]
async fn mark_completed_sets_signature_and_completed_at(pool: PgPool) {
    let id = enqueue_claim(&pool, &SERVICE_ID_A, AGENT_A, 1, None)
        .await
        .expect("enqueue");

    mark_completed(&pool, &id, "tx-sig-success")
        .await
        .expect("mark_completed");

    let (status, sig, completed_at): (
        String,
        Option<String>,
        Option<chrono::DateTime<chrono::Utc>>,
    ) = sqlx::query_as(
        "SELECT status, tx_signature, completed_at
             FROM escrow_claim_queue WHERE id = $1::uuid",
    )
    .bind(&id)
    .fetch_one(&pool)
    .await
    .expect("read");

    assert_eq!(status, "completed");
    assert_eq!(sig.as_deref(), Some("tx-sig-success"));
    assert!(completed_at.is_some(), "completed_at must be set");
}

// ---------------------------------------------------------------------------
// mark_attempt_failed
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../../migrations")]
async fn mark_attempt_failed_keeps_pending_until_max_attempts(pool: PgPool) {
    let id = enqueue_claim(&pool, &SERVICE_ID_A, AGENT_A, 1, None)
        .await
        .expect("enqueue");

    // After mark_in_progress + mark_attempt_failed at attempts=1, we're under the cap.
    mark_in_progress(&pool, &id).await.expect("ip");
    mark_attempt_failed(&pool, &id, "rpc timeout", 1)
        .await
        .expect("fail");

    let (status, error_message, next_retry_at): (
        String,
        Option<String>,
        Option<chrono::DateTime<chrono::Utc>>,
    ) = sqlx::query_as(
        "SELECT status, error_message, next_retry_at
         FROM escrow_claim_queue WHERE id = $1::uuid",
    )
    .bind(&id)
    .fetch_one(&pool)
    .await
    .expect("read");

    assert_eq!(
        status, "pending",
        "must return to pending below max attempts"
    );
    assert_eq!(error_message.as_deref(), Some("rpc timeout"));
    assert!(
        next_retry_at.is_some(),
        "backoff must populate next_retry_at"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn mark_attempt_failed_marks_failed_at_max_attempts(pool: PgPool) {
    let id = enqueue_claim(&pool, &SERVICE_ID_A, AGENT_A, 1, None)
        .await
        .expect("enqueue");

    // Drive the row's `attempts` column up to MAX_CLAIM_ATTEMPTS via repeated
    // mark_in_progress calls (each increments by 1).
    for _ in 0..MAX_CLAIM_ATTEMPTS {
        mark_in_progress(&pool, &id).await.expect("ip");
    }

    mark_attempt_failed(&pool, &id, "permanent error", MAX_CLAIM_ATTEMPTS)
        .await
        .expect("fail");

    let status: String =
        sqlx::query_scalar("SELECT status FROM escrow_claim_queue WHERE id = $1::uuid")
            .bind(&id)
            .fetch_one(&pool)
            .await
            .expect("read");
    assert_eq!(status, "failed", "must permanently fail at max attempts");
}

// ---------------------------------------------------------------------------
// fetch_pending_claims
// ---------------------------------------------------------------------------

#[sqlx::test(migrations = "../../migrations")]
async fn fetch_pending_claims_returns_pending_rows_in_creation_order(pool: PgPool) {
    let id_a = enqueue_claim(&pool, &SERVICE_ID_A, AGENT_A, 100, None)
        .await
        .expect("a");
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let id_b = enqueue_claim(&pool, &SERVICE_ID_B, AGENT_B, 200, Some(300))
        .await
        .expect("b");

    let claims = fetch_pending_claims(&pool, 10)
        .await
        .expect("fetch_pending_claims");

    assert_eq!(claims.len(), 2);
    assert_eq!(claims[0].id, id_a, "oldest first");
    assert_eq!(claims[1].id, id_b);

    let claim_a = &claims[0];
    assert_eq!(claim_a.agent_pubkey, AGENT_A);
    assert_eq!(claim_a.claim_amount, 100);
    assert_eq!(claim_a.deposited_amount, None);
    assert_eq!(claim_a.status, ClaimStatus::Pending);
    assert_eq!(claim_a.attempts, 0);
    assert_eq!(&claim_a.service_id[..], &SERVICE_ID_A[..]);

    let claim_b = &claims[1];
    assert_eq!(claim_b.deposited_amount, Some(300));
    assert_eq!(&claim_b.service_id[..], &SERVICE_ID_B[..]);
}

#[sqlx::test(migrations = "../../migrations")]
async fn fetch_pending_claims_respects_limit(pool: PgPool) {
    for n in 0..3u8 {
        let mut sid = [0u8; 32];
        sid[0] = n + 1;
        enqueue_claim(&pool, &sid, AGENT_A, 1, None)
            .await
            .expect("enqueue");
    }

    let claims = fetch_pending_claims(&pool, 2)
        .await
        .expect("fetch_pending_claims");
    assert_eq!(claims.len(), 2, "limit must be honored");
}

#[sqlx::test(migrations = "../../migrations")]
async fn fetch_pending_claims_skips_completed_and_failed(pool: PgPool) {
    let id_done = enqueue_claim(&pool, &SERVICE_ID_A, AGENT_A, 1, None)
        .await
        .expect("a");
    let id_pending = enqueue_claim(&pool, &SERVICE_ID_B, AGENT_B, 2, None)
        .await
        .expect("b");

    mark_completed(&pool, &id_done, "sig").await.expect("done");

    let claims = fetch_pending_claims(&pool, 10)
        .await
        .expect("fetch_pending_claims");
    assert_eq!(claims.len(), 1, "completed claim must be excluded");
    assert_eq!(claims[0].id, id_pending);
}

#[sqlx::test(migrations = "../../migrations")]
async fn fetch_pending_claims_skips_pending_rows_with_future_next_retry_at(pool: PgPool) {
    let id_now = enqueue_claim(&pool, &SERVICE_ID_A, AGENT_A, 1, None)
        .await
        .expect("now");
    let id_later = enqueue_claim(&pool, &SERVICE_ID_B, AGENT_B, 2, None)
        .await
        .expect("later");

    sqlx::query("UPDATE escrow_claim_queue SET next_retry_at = NOW() + INTERVAL '1 hour' WHERE id = $1::uuid")
        .bind(&id_later)
        .execute(&pool)
        .await
        .expect("set future retry");

    let claims = fetch_pending_claims(&pool, 10)
        .await
        .expect("fetch_pending_claims");
    assert_eq!(claims.len(), 1, "future-scheduled retry must be skipped");
    assert_eq!(claims[0].id, id_now);
}
