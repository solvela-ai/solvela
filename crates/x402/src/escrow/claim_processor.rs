//! Background task that processes pending escrow claims from the queue.
//!
//! Polls the `escrow_claim_queue` table at a configurable interval, picks up
//! pending claims, submits them on-chain via [`do_claim_with_params`], and
//! marks them as completed or failed in the database.
//!
//! Includes exponential backoff for retries and a circuit breaker that pauses
//! processing when the failure rate exceeds 50% in a 5-minute window.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use metrics::{counter, gauge};
use tracing::{error, info, warn};

use super::claim_queue::{self, MAX_CLAIM_ATTEMPTS};
use super::claimer::{do_claim_with_params, EscrowClaimer};
use super::pda::decode_bs58_pubkey;

// ---------------------------------------------------------------------------
// Escrow Metrics
// ---------------------------------------------------------------------------

/// In-memory atomic counters for escrow claim processing metrics.
///
/// These counters are cheap (zero-cost atomics), reset on gateway restart,
/// and exposed via the `/v1/escrow/health` endpoint. Persistent claim data
/// lives in the PostgreSQL `escrow_claim_queue` table.
#[derive(Debug)]
pub struct EscrowMetrics {
    pub claims_submitted: AtomicU64,
    pub claims_succeeded: AtomicU64,
    pub claims_failed: AtomicU64,
    pub claims_retried: AtomicU64,
}

impl EscrowMetrics {
    /// Create a new metrics instance with all counters at zero.
    pub fn new() -> Self {
        Self {
            claims_submitted: AtomicU64::new(0),
            claims_succeeded: AtomicU64::new(0),
            claims_failed: AtomicU64::new(0),
            claims_retried: AtomicU64::new(0),
        }
    }

    /// Read the current counter values as a snapshot.
    pub fn snapshot(&self) -> EscrowMetricsSnapshot {
        EscrowMetricsSnapshot {
            claims_submitted: self.claims_submitted.load(Ordering::Relaxed),
            claims_succeeded: self.claims_succeeded.load(Ordering::Relaxed),
            claims_failed: self.claims_failed.load(Ordering::Relaxed),
            claims_retried: self.claims_retried.load(Ordering::Relaxed),
        }
    }
}

impl Default for EscrowMetrics {
    fn default() -> Self {
        Self::new()
    }
}

/// Point-in-time snapshot of escrow claim metrics.
#[derive(Debug, Clone)]
pub struct EscrowMetricsSnapshot {
    pub claims_submitted: u64,
    pub claims_succeeded: u64,
    pub claims_failed: u64,
    pub claims_retried: u64,
}

// ---------------------------------------------------------------------------
// Circuit Breaker
// ---------------------------------------------------------------------------

/// Mutable state protected by a single lock to avoid ABBA deadlocks.
struct CircuitBreakerState {
    last_reset: Instant,
    tripped_until: Option<Instant>,
}

/// Rolling-window circuit breaker for claim processing.
///
/// Tracks success/failure counts in a 5-minute window. If the failure rate
/// exceeds 50% (with a minimum of 4 samples), processing is paused for 60
/// seconds.
pub struct ClaimCircuitBreaker {
    success_count: AtomicU64,
    failure_count: AtomicU64,
    state: Mutex<CircuitBreakerState>,
    /// Duration of the rolling window (default 5 minutes).
    window: Duration,
    /// How long to pause when tripped (default 60 seconds).
    pause_duration: Duration,
    /// Minimum total samples before the breaker can trip.
    min_sample_size: u64,
}

impl ClaimCircuitBreaker {
    /// Create a new circuit breaker with default parameters.
    pub fn new() -> Self {
        Self {
            success_count: AtomicU64::new(0),
            failure_count: AtomicU64::new(0),
            state: Mutex::new(CircuitBreakerState {
                last_reset: Instant::now(),
                tripped_until: None,
            }),
            window: Duration::from_secs(300),
            pause_duration: Duration::from_secs(60),
            min_sample_size: 4,
        }
    }

    /// Create a circuit breaker with custom parameters (for testing).
    #[cfg(test)]
    pub fn with_params(window: Duration, pause_duration: Duration, min_sample_size: u64) -> Self {
        Self {
            success_count: AtomicU64::new(0),
            failure_count: AtomicU64::new(0),
            state: Mutex::new(CircuitBreakerState {
                last_reset: Instant::now(),
                tripped_until: None,
            }),
            window,
            pause_duration,
            min_sample_size,
        }
    }

    /// Reset counters if the rolling window has expired.
    ///
    /// Caller must hold the `state` lock and pass the guard.
    fn maybe_reset_window(&self, state: &mut CircuitBreakerState) {
        if state.last_reset.elapsed() >= self.window {
            self.success_count.store(0, Ordering::Relaxed);
            self.failure_count.store(0, Ordering::Relaxed);
            state.last_reset = Instant::now();
        }
    }

    /// Record a successful claim.
    pub fn record_success(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.maybe_reset_window(&mut state);
        self.success_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a failed claim. May trip the circuit breaker.
    pub fn record_failure(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        self.maybe_reset_window(&mut state);
        let failures = self.failure_count.fetch_add(1, Ordering::Relaxed) + 1;
        let successes = self.success_count.load(Ordering::Relaxed);
        let total = successes + failures;

        if total >= self.min_sample_size && failures * 2 > total && state.tripped_until.is_none() {
            let until = Instant::now() + self.pause_duration;
            state.tripped_until = Some(until);
            error!(
                failures = failures,
                successes = successes,
                total = total,
                pause_secs = self.pause_duration.as_secs(),
                "claim circuit breaker OPENED — pausing claim processing"
            );
        }
    }

    /// Check if the circuit breaker is open (processing should be paused).
    pub fn is_open(&self) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        match state.tripped_until {
            Some(until) => {
                if Instant::now() >= until {
                    // Circuit breaker closes — reset counters
                    state.tripped_until = None;
                    state.last_reset = Instant::now();
                    self.success_count.store(0, Ordering::Relaxed);
                    self.failure_count.store(0, Ordering::Relaxed);
                    info!("claim circuit breaker CLOSED — resuming claim processing");
                    false
                } else {
                    true
                }
            }
            None => false,
        }
    }
}

impl Default for ClaimCircuitBreaker {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Processor
// ---------------------------------------------------------------------------

/// Start the background claim processor. Polls every `poll_interval`.
///
/// The optional `metrics` parameter enables in-memory counters for
/// `claims_submitted`, `claims_succeeded`, `claims_failed`, and `claims_retried`.
/// Pass `None` to disable metrics tracking.
///
/// The `shutdown_rx` watch channel enables graceful shutdown. When the sender
/// sends `true`, the processor finishes its current cycle and exits.
///
/// Returns the [`tokio::task::JoinHandle`] so the caller can await clean
/// shutdown.
pub fn start_claim_processor(
    pool: sqlx::PgPool,
    claimer: Arc<EscrowClaimer>,
    poll_interval: Duration,
    metrics: Option<Arc<EscrowMetrics>>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    let circuit_breaker = Arc::new(ClaimCircuitBreaker::new());

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(poll_interval);
        info!(
            poll_interval_secs = poll_interval.as_secs(),
            "claim processor started"
        );
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(e) =
                        process_pending_claims(&pool, &claimer, &circuit_breaker, metrics.as_deref()).await
                    {
                        warn!(error = %e, "claim processor cycle failed");
                    }
                }
                _ = shutdown_rx.changed() => {
                    info!("claim processor shutting down gracefully");
                    break;
                }
            }
        }
    })
}

/// Process all pending claims in a single cycle.
///
/// Fetches up to 10 pending claims (respecting `next_retry_at` backoff),
/// marks each as in-progress, attempts the on-chain claim, and records the
/// outcome. Claims that have exceeded [`MAX_CLAIM_ATTEMPTS`] are marked as
/// permanently failed. The circuit breaker is checked before processing and
/// updated after each claim.
async fn process_pending_claims(
    pool: &sqlx::PgPool,
    claimer: &EscrowClaimer,
    circuit_breaker: &ClaimCircuitBreaker,
    metrics: Option<&EscrowMetrics>,
) -> Result<(), String> {
    // Check circuit breaker before processing
    if circuit_breaker.is_open() {
        warn!("claim circuit breaker is open — skipping processing cycle");
        return Ok(());
    }

    let pending = claim_queue::fetch_pending_claims(pool, 10)
        .await
        .map_err(|e| format!("failed to fetch pending claims: {e}"))?;

    gauge!("solvela_escrow_queue_depth").set(pending.len() as f64);

    if pending.is_empty() {
        return Ok(());
    }

    info!(count = pending.len(), "processing pending escrow claims");

    // Track each claim that enters processing as "submitted"
    if let Some(m) = metrics {
        m.claims_submitted
            .fetch_add(pending.len() as u64, Ordering::Relaxed);
    }

    for entry in &pending {
        // Skip claims that have exceeded max attempts (mark as failed)
        if entry.attempts >= MAX_CLAIM_ATTEMPTS {
            let error_msg = format!("exceeded maximum retry attempts ({MAX_CLAIM_ATTEMPTS})");
            warn!(
                claim_id = %entry.id,
                attempts = entry.attempts,
                "marking claim as permanently failed — max retries exceeded"
            );
            if let Err(db_err) =
                claim_queue::mark_attempt_failed(pool, &entry.id, &error_msg, entry.attempts).await
            {
                warn!(
                    claim_id = %entry.id,
                    error = %db_err,
                    "failed to mark max-retry claim as permanently failed — \
                     claim may be retried indefinitely"
                );
            }
            circuit_breaker.record_failure();
            counter!("solvela_escrow_claims_total", "result" => "failure").increment(1);
            if let Some(m) = metrics {
                m.claims_failed.fetch_add(1, Ordering::Relaxed);
            }
            continue;
        }

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
                let _ = claim_queue::mark_attempt_failed(
                    pool,
                    &entry.id,
                    &error_msg,
                    entry.attempts + 1,
                )
                .await;
                circuit_breaker.record_failure();
                counter!("solvela_escrow_claims_total", "result" => "failure").increment(1);
                if let Some(m) = metrics {
                    m.claims_failed.fetch_add(1, Ordering::Relaxed);
                }
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
                circuit_breaker.record_success();
                counter!("solvela_escrow_claims_total", "result" => "success").increment(1);
                if let Some(m) = metrics {
                    m.claims_succeeded.fetch_add(1, Ordering::Relaxed);
                }
            }
            Err(e) => {
                let error_msg = e.to_string();
                warn!(
                    claim_id = %entry.id,
                    error = %error_msg,
                    attempt = entry.attempts + 1,
                    max_attempts = MAX_CLAIM_ATTEMPTS,
                    "escrow claim attempt failed"
                );
                if let Err(db_err) = claim_queue::mark_attempt_failed(
                    pool,
                    &entry.id,
                    &error_msg,
                    entry.attempts + 1,
                )
                .await
                {
                    warn!(
                        claim_id = %entry.id,
                        error = %db_err,
                        "failed to record claim failure"
                    );
                }
                circuit_breaker.record_failure();
                counter!("solvela_escrow_claims_total", "result" => "failure").increment(1);
                if let Some(m) = metrics {
                    // If this claim has exceeded max attempts, count as permanently failed.
                    // Otherwise, it's a retry.
                    if entry.attempts + 1 >= MAX_CLAIM_ATTEMPTS {
                        m.claims_failed.fetch_add(1, Ordering::Relaxed);
                    } else {
                        m.claims_retried.fetch_add(1, Ordering::Relaxed);
                    }
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
    #[allow(clippy::type_complexity)]
    fn test_start_claim_processor_compiles() {
        // Verify the public API compiles correctly.
        // Actual processing requires a live PgPool + EscrowClaimer,
        // which are covered by integration tests.
        let _: fn(
            sqlx::PgPool,
            Arc<EscrowClaimer>,
            Duration,
            Option<Arc<EscrowMetrics>>,
            tokio::sync::watch::Receiver<bool>,
        ) -> tokio::task::JoinHandle<()> = start_claim_processor;
    }

    #[test]
    fn test_circuit_breaker_starts_closed() {
        let cb = ClaimCircuitBreaker::new();
        assert!(!cb.is_open());
    }

    #[test]
    fn test_circuit_breaker_does_not_trip_below_min_sample() {
        let cb = ClaimCircuitBreaker::new();
        // 3 failures, but total < 4 (min_sample_size)
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        assert!(
            !cb.is_open(),
            "should not trip with only 3 samples (min is 4)"
        );
    }

    #[test]
    fn test_circuit_breaker_trips_at_50_percent_failure() {
        let cb = ClaimCircuitBreaker::new();
        // 1 success + 3 failures = 4 total, 75% failure rate > 50%
        // The success must come first because tripping is checked in record_failure.
        cb.record_success();
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        assert!(
            cb.is_open(),
            "should trip at 75% failure rate with 4 samples"
        );
    }

    #[test]
    fn test_circuit_breaker_does_not_trip_at_50_percent_exactly() {
        let cb = ClaimCircuitBreaker::new();
        // 2 failures + 2 successes = exactly 50%, should NOT trip (>50% required)
        cb.record_success();
        cb.record_success();
        cb.record_failure();
        cb.record_failure();
        assert!(!cb.is_open(), "should not trip at exactly 50%");
    }

    #[test]
    fn test_circuit_breaker_stays_open_until_pause_expires() {
        let cb = ClaimCircuitBreaker::with_params(
            Duration::from_secs(300),
            Duration::from_millis(50), // short pause for testing
            4,
        );
        // Trip the breaker: 4 failures, 0 successes = 100%
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        assert!(cb.is_open(), "should be open after tripping");
        assert!(cb.is_open(), "should still be open immediately after");
    }

    #[test]
    fn test_circuit_breaker_closes_after_pause() {
        let cb = ClaimCircuitBreaker::with_params(
            Duration::from_secs(300),
            Duration::from_millis(1), // 1ms pause
            4,
        );
        // Trip the breaker
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        assert!(cb.is_open());

        // Wait for pause to expire
        std::thread::sleep(Duration::from_millis(5));
        assert!(!cb.is_open(), "should close after pause expires");
    }

    #[test]
    fn test_circuit_breaker_resets_counters_on_close() {
        let cb =
            ClaimCircuitBreaker::with_params(Duration::from_secs(300), Duration::from_millis(1), 4);
        // Trip the breaker
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();
        assert!(cb.is_open());

        // Wait for close
        std::thread::sleep(Duration::from_millis(5));
        assert!(!cb.is_open());

        // After closing, counters should be reset — a single failure should not trip
        cb.record_failure();
        assert!(
            !cb.is_open(),
            "should not trip after reset with only 1 failure"
        );
    }

    #[test]
    fn test_circuit_breaker_window_resets_counts() {
        let cb = ClaimCircuitBreaker::with_params(
            Duration::from_millis(1), // 1ms window
            Duration::from_secs(60),
            4,
        );
        // Record some failures
        cb.record_failure();
        cb.record_failure();
        cb.record_failure();

        // Wait for window to expire
        std::thread::sleep(Duration::from_millis(5));

        // After window reset, recording one more failure should not trip (total = 1 < 4)
        cb.record_failure();
        assert!(!cb.is_open(), "window reset should clear old counts");
    }

    #[test]
    fn test_circuit_breaker_all_successes_stays_closed() {
        let cb = ClaimCircuitBreaker::new();
        for _ in 0..20 {
            cb.record_success();
        }
        assert!(!cb.is_open());
    }

    // -----------------------------------------------------------------------
    // EscrowMetrics tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_escrow_metrics_new_starts_at_zero() {
        let m = EscrowMetrics::new();
        let snap = m.snapshot();
        assert_eq!(snap.claims_submitted, 0);
        assert_eq!(snap.claims_succeeded, 0);
        assert_eq!(snap.claims_failed, 0);
        assert_eq!(snap.claims_retried, 0);
    }

    #[test]
    fn test_escrow_metrics_increments_correctly() {
        let m = EscrowMetrics::new();
        m.claims_submitted.fetch_add(10, Ordering::Relaxed);
        m.claims_succeeded.fetch_add(7, Ordering::Relaxed);
        m.claims_failed.fetch_add(2, Ordering::Relaxed);
        m.claims_retried.fetch_add(1, Ordering::Relaxed);

        let snap = m.snapshot();
        assert_eq!(snap.claims_submitted, 10);
        assert_eq!(snap.claims_succeeded, 7);
        assert_eq!(snap.claims_failed, 2);
        assert_eq!(snap.claims_retried, 1);
    }

    #[test]
    fn test_escrow_metrics_default_is_zero() {
        let m = EscrowMetrics::default();
        let snap = m.snapshot();
        assert_eq!(snap.claims_submitted, 0);
        assert_eq!(snap.claims_succeeded, 0);
        assert_eq!(snap.claims_failed, 0);
        assert_eq!(snap.claims_retried, 0);
    }

    // ---------------------------------------------------------------------
    // process_pending_claims integration tests
    //
    // Cover the DB-bound branches without touching RPC. EscrowClaimer is
    // built with a drained fee-payer pool so do_claim_with_params returns
    // Err synchronously before any HTTP call (matches the "no healthy fee
    // payer" path tested in claimer.rs::do_claim_with_params_errors_*).
    // ---------------------------------------------------------------------

    use crate::fee_payer::FeePayerPool;
    use sqlx::PgPool;

    /// Generate a deterministic valid base58-encoded ed25519 keypair.
    fn test_keypair_b58(seed: u8) -> String {
        use ed25519_dalek::SigningKey;
        let mut secret = [0u8; 32];
        secret[0] = seed;
        secret[31] = seed.wrapping_add(1);
        let signing_key = SigningKey::from_bytes(&secret);
        let mut keypair_bytes = [0u8; 64];
        keypair_bytes[..32].copy_from_slice(&signing_key.to_bytes());
        keypair_bytes[32..].copy_from_slice(signing_key.verifying_key().as_bytes());
        bs58::encode(&keypair_bytes).into_string()
    }

    /// Construct an EscrowClaimer whose only fee payer is marked unhealthy,
    /// so any do_claim_with_params call fails immediately with
    /// "no healthy fee payer" without touching the network.
    fn drained_test_claimer() -> Arc<EscrowClaimer> {
        let pool = Arc::new(FeePayerPool::from_keys(&[test_keypair_b58(99)]).expect("pool"));
        pool.mark_failed(0);
        Arc::new(
            EscrowClaimer::new(
                "https://api.devnet.solana.com".to_string(),
                pool,
                "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU",
                "11111111111111111111111111111111",
                "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                None,
            )
            .expect("construct"),
        )
    }

    const VALID_AGENT_B58: &str = "9noXzpXnkyEcKF3AeXqUHTdR59V5uvrRBUo9bwsHaByz";

    #[sqlx::test(migrations = "../../migrations")]
    async fn process_pending_claims_returns_ok_when_circuit_breaker_open(pool: PgPool) {
        // Pre-seed a pending claim that the loop would otherwise pick up.
        claim_queue::enqueue_claim(&pool, &[1u8; 32], VALID_AGENT_B58, 1_000, None)
            .await
            .expect("enqueue");

        // Trip the circuit breaker before processing.
        let cb =
            ClaimCircuitBreaker::with_params(Duration::from_secs(60), Duration::from_secs(60), 4);
        for _ in 0..4 {
            cb.record_failure();
        }
        assert!(cb.is_open(), "fixture must trip the breaker");

        let claimer = drained_test_claimer();
        process_pending_claims(&pool, &claimer, &cb, None)
            .await
            .expect("must short-circuit on open breaker");

        // Claim must remain pending — the loop returned without writing.
        let status: String = sqlx::query_scalar("SELECT status FROM escrow_claim_queue LIMIT 1")
            .fetch_one(&pool)
            .await
            .expect("read");
        assert_eq!(status, "pending", "must not touch DB when breaker is open");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn process_pending_claims_returns_ok_with_empty_queue(pool: PgPool) {
        let cb = ClaimCircuitBreaker::new();
        let claimer = drained_test_claimer();
        process_pending_claims(&pool, &claimer, &cb, None)
            .await
            .expect("empty queue must return Ok");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn process_pending_claims_marks_max_retries_claim_as_failed(pool: PgPool) {
        let id = claim_queue::enqueue_claim(&pool, &[1u8; 32], VALID_AGENT_B58, 1_000, None)
            .await
            .expect("enqueue");
        // Force attempts = MAX while keeping status='pending' so fetch_pending_claims
        // picks the row up. (mark_in_progress would flip status to 'in_progress' and
        // the recent updated_at would exclude it from the next poll.)
        sqlx::query("UPDATE escrow_claim_queue SET attempts = $1 WHERE id = $2::uuid")
            .bind(MAX_CLAIM_ATTEMPTS)
            .bind(&id)
            .execute(&pool)
            .await
            .expect("force attempts");

        let cb = ClaimCircuitBreaker::new();
        let metrics = Arc::new(EscrowMetrics::default());
        let claimer = drained_test_claimer();
        process_pending_claims(&pool, &claimer, &cb, Some(&metrics))
            .await
            .expect("loop should succeed");

        let status: String =
            sqlx::query_scalar("SELECT status FROM escrow_claim_queue WHERE id = $1::uuid")
                .bind(&id)
                .fetch_one(&pool)
                .await
                .expect("read");
        assert_eq!(status, "failed", "max-retries must mark permanently failed");
        assert_eq!(metrics.snapshot().claims_failed, 1, "metric must increment");
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn process_pending_claims_marks_invalid_agent_pubkey_as_failed(pool: PgPool) {
        // bs58-invalid agent pubkey must fail before the RPC call.
        let id = claim_queue::enqueue_claim(&pool, &[1u8; 32], "not-valid-bs58!!!", 1_000, None)
            .await
            .expect("enqueue");

        let cb = ClaimCircuitBreaker::new();
        let metrics = Arc::new(EscrowMetrics::default());
        let claimer = drained_test_claimer();
        process_pending_claims(&pool, &claimer, &cb, Some(&metrics))
            .await
            .expect("loop should succeed");

        let (status, attempts, error_message): (String, i32, Option<String>) = sqlx::query_as(
            "SELECT status, attempts, error_message
             FROM escrow_claim_queue WHERE id = $1::uuid",
        )
        .bind(&id)
        .fetch_one(&pool)
        .await
        .expect("read");

        // After mark_in_progress(+1) + mark_attempt_failed(invalid bs58),
        // status returns to pending (under MAX) and error_message is set.
        assert_eq!(status, "pending");
        assert_eq!(attempts, 1, "mark_in_progress must have incremented");
        let msg = error_message.expect("error_message must be set");
        assert!(msg.contains("invalid agent pubkey"), "msg: {msg}");

        // Metric must increment claims_failed (this branch counts as failure).
        assert_eq!(metrics.snapshot().claims_failed, 1);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn process_pending_claims_marks_attempt_failed_when_claimer_errors(pool: PgPool) {
        // Valid claim with a valid bs58 agent — but the claimer's fee-payer
        // pool is drained, so do_claim_with_params returns Err synchronously
        // before any HTTP call.
        let id = claim_queue::enqueue_claim(&pool, &[1u8; 32], VALID_AGENT_B58, 1_000, None)
            .await
            .expect("enqueue");

        let cb = ClaimCircuitBreaker::new();
        let metrics = Arc::new(EscrowMetrics::default());
        let claimer = drained_test_claimer();
        process_pending_claims(&pool, &claimer, &cb, Some(&metrics))
            .await
            .expect("loop should succeed");

        let (status, attempts, error_message): (String, i32, Option<String>) = sqlx::query_as(
            "SELECT status, attempts, error_message
             FROM escrow_claim_queue WHERE id = $1::uuid",
        )
        .bind(&id)
        .fetch_one(&pool)
        .await
        .expect("read");

        // mark_in_progress (+1) → submission Err → mark_attempt_failed → pending again.
        assert_eq!(status, "pending");
        assert_eq!(attempts, 1);
        let msg = error_message.expect("error_message must be set");
        assert!(
            msg.contains("no healthy fee payer"),
            "claimer error must propagate to error_message: {msg}"
        );

        // Below MAX → counts as a retry, not a failure.
        let snap = metrics.snapshot();
        assert_eq!(snap.claims_submitted, 1);
        assert_eq!(snap.claims_retried, 1);
        assert_eq!(snap.claims_failed, 0);
    }

    #[sqlx::test(migrations = "../../migrations")]
    async fn process_pending_claims_counts_failure_when_attempts_at_max_after_increment(
        pool: PgPool,
    ) {
        // After mark_in_progress, attempts becomes MAX_CLAIM_ATTEMPTS exactly.
        // The submission errors, mark_attempt_failed marks the row failed,
        // and metrics must increment claims_failed (not claims_retried).
        let id = claim_queue::enqueue_claim(&pool, &[1u8; 32], VALID_AGENT_B58, 1_000, None)
            .await
            .expect("enqueue");
        // Force attempts = MAX-1 while keeping status='pending' so the row passes
        // the entry guard (entry.attempts < MAX_CLAIM_ATTEMPTS) and the
        // post-increment value (entry.attempts + 1) hits MAX exactly.
        sqlx::query("UPDATE escrow_claim_queue SET attempts = $1 WHERE id = $2::uuid")
            .bind(MAX_CLAIM_ATTEMPTS - 1)
            .bind(&id)
            .execute(&pool)
            .await
            .expect("force attempts");

        let cb = ClaimCircuitBreaker::new();
        let metrics = Arc::new(EscrowMetrics::default());
        let claimer = drained_test_claimer();
        process_pending_claims(&pool, &claimer, &cb, Some(&metrics))
            .await
            .expect("loop should succeed");

        let snap = metrics.snapshot();
        assert_eq!(snap.claims_submitted, 1);
        assert_eq!(
            snap.claims_failed, 1,
            "attempts+1 == MAX must count as permanent failure"
        );
        assert_eq!(snap.claims_retried, 0);
    }
}
