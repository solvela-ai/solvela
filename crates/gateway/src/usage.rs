//! Per-wallet usage tracking and budget management.
//!
//! PostgreSQL for persistent spend logs, Redis for hot-path spend tracking.
//! All DB writes are async (tokio::spawn) — never on the request critical path.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use uuid::Uuid;

/// Tolerance for f64 USDC comparisons.
///
/// USDC has 6 decimal places, so 1 atomic unit = 0.000001 USDC.
/// We use half an atomic unit as epsilon to avoid rounding errors
/// affecting budget comparisons while still being strict enough
/// for financial correctness.
const USDC_EPSILON: f64 = 0.000_000_5;

/// A single spend log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpendLog {
    pub id: Uuid,
    pub wallet_address: String,
    pub model: String,
    pub provider: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cost_usdc: f64,
    pub tx_signature: Option<String>,
    pub created_at: DateTime<Utc>,
}

/// Budget limits for a wallet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletBudget {
    pub wallet_address: String,
    pub daily_limit_usdc: Option<f64>,
    pub monthly_limit_usdc: Option<f64>,
    pub total_spent_usdc: f64,
    pub created_at: DateTime<Utc>,
}

/// Summary of wallet spending.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpendSummary {
    pub wallet_address: String,
    pub total_requests: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cost_usdc: f64,
    pub daily_cost_usdc: f64,
    pub monthly_cost_usdc: f64,
}

/// Input struct for `log_spend()` — groups all spend log fields.
///
/// Replaces positional arguments to keep the API clean as fields grow.
#[derive(Debug, Clone)]
pub struct SpendLogEntry {
    pub wallet_address: String,
    pub model: String,
    pub provider: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cost_usdc: f64,
    pub tx_signature: Option<String>,
    pub request_id: Option<String>,
    pub session_id: Option<String>,
}

/// Error types for usage tracking.
#[derive(Debug, thiserror::Error)]
pub enum UsageError {
    #[error("database error: {0}")]
    Database(String),

    #[error(
        "budget exceeded for wallet {wallet}: estimated ${spent:.4} exceeds limit ${limit:.2}"
    )]
    BudgetExceeded {
        wallet: String,
        limit: f64,
        spent: f64,
    },

    #[error("redis error: {0}")]
    Redis(String),

    #[error("not configured")]
    NotConfigured,
}

/// Usage tracker with optional PostgreSQL and Redis backends.
///
/// Designed for graceful degradation:
/// - Without PostgreSQL: spend logs are logged but not persisted
/// - Without Redis: hot-path tracking falls back to in-memory
pub struct UsageTracker {
    /// Optional PostgreSQL connection pool.
    db_pool: Option<sqlx::PgPool>,
    /// Optional Redis client for hot-path data.
    redis_client: Option<redis::Client>,
}

impl UsageTracker {
    /// Create a new usage tracker.
    ///
    /// Both database and Redis are optional — pass None for development/testing.
    pub fn new(db_pool: Option<sqlx::PgPool>, redis_client: Option<redis::Client>) -> Self {
        Self {
            db_pool,
            redis_client,
        }
    }

    /// Create a tracker with no backends (for testing).
    pub fn noop() -> Self {
        Self {
            db_pool: None,
            redis_client: None,
        }
    }

    /// Log a spend event asynchronously (non-blocking).
    ///
    /// This should be called after every successful LLM request.
    /// The write is spawned onto a background task.
    pub fn log_spend(&self, entry: SpendLogEntry) {
        let id = Uuid::new_v4();
        let created_at = Utc::now();

        info!(
            wallet = %entry.wallet_address,
            model = %entry.model,
            provider = %entry.provider,
            input_tokens = entry.input_tokens,
            output_tokens = entry.output_tokens,
            cost_usdc = entry.cost_usdc,
            tx_signature = entry.tx_signature.as_deref().unwrap_or("none"),
            request_id = entry.request_id.as_deref().unwrap_or("none"),
            session_id = entry.session_id.as_deref().unwrap_or("none"),
            "spend logged"
        );

        // Write to PostgreSQL asynchronously
        if let Some(pool) = &self.db_pool {
            let pool = pool.clone();
            let db_entry = entry.clone();
            tokio::spawn(async move {
                let result = sqlx::query(
                    r#"INSERT INTO spend_logs (id, wallet_address, model, provider, input_tokens, output_tokens, cost_usdc, tx_signature, request_id, session_id, created_at)
                       VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)"#,
                )
                .bind(id)
                .bind(&db_entry.wallet_address)
                .bind(&db_entry.model)
                .bind(&db_entry.provider)
                .bind(db_entry.input_tokens as i32)
                .bind(db_entry.output_tokens as i32)
                .bind(db_entry.cost_usdc)
                .bind(&db_entry.tx_signature)
                .bind(&db_entry.request_id)
                .bind(&db_entry.session_id)
                .bind(created_at)
                .execute(&pool)
                .await;

                if let Err(e) = result {
                    warn!(error = %e, "failed to write spend log to database");
                }
            });
        }

        // Update Redis hot-path counters
        if let Some(client) = &self.redis_client {
            let client = client.clone();
            let wallet = entry.wallet_address;
            let cost = entry.cost_usdc;
            tokio::spawn(async move {
                let mut conn = match client.get_multiplexed_async_connection().await {
                    Ok(c) => c,
                    Err(e) => {
                        // SECURITY: Redis spend tracking is unavailable. Budget enforcement
                        // is degraded — requests proceed without accumulation tracking.
                        // This is fail-open by design (availability over strict enforcement),
                        // but operators MUST investigate promptly.
                        warn!(
                            error = %e,
                            wallet = %wallet,
                            cost_usdc = cost,
                            "Redis unavailable for spend tracking — budget enforcement degraded"
                        );
                        return;
                    }
                };

                // Daily spend counter
                let day_key = format!("spend:{}:{}", wallet, Utc::now().format("%Y-%m-%d"));
                if let Err(e) = redis::cmd("INCRBYFLOAT")
                    .arg(&day_key)
                    .arg(cost)
                    .query_async::<()>(&mut conn)
                    .await
                {
                    warn!(error = %e, key = %day_key, "failed to increment daily spend counter in Redis");
                }
                if let Err(e) = redis::cmd("EXPIRE")
                    .arg(&day_key)
                    .arg(86400_u64)
                    .query_async::<()>(&mut conn)
                    .await
                {
                    warn!(error = %e, key = %day_key, "failed to set TTL on daily spend counter");
                }

                // Monthly spend counter
                let month_key = format!("spend:{}:{}", wallet, Utc::now().format("%Y-%m"));
                if let Err(e) = redis::cmd("INCRBYFLOAT")
                    .arg(&month_key)
                    .arg(cost)
                    .query_async::<()>(&mut conn)
                    .await
                {
                    warn!(error = %e, key = %month_key, "failed to increment monthly spend counter in Redis");
                }
                if let Err(e) = redis::cmd("EXPIRE")
                    .arg(&month_key)
                    .arg(86400_u64 * 31)
                    .query_async::<()>(&mut conn)
                    .await
                {
                    warn!(error = %e, key = %month_key, "failed to set TTL on monthly spend counter");
                }
            });
        }
    }

    /// Check if a wallet's budget allows a request with the estimated cost.
    ///
    /// Returns `Ok(())` if within budget, `Err(UsageError::BudgetExceeded)` if not.
    ///
    /// **No-Redis fallback**: when Redis is unavailable and no client is configured,
    /// a conservative per-request cap of $1.00 USDC is applied to prevent runaway
    /// spend on high-cost models.  Requests with an estimated cost at or below $1.00
    /// are allowed through; above that they are rejected.
    ///
    /// **Fail-open design decision**: When a Redis client IS configured but the
    /// connection fails at request time (e.g., Redis is temporarily down), the
    /// budget check is skipped and the request is allowed through. This is an
    /// intentional fail-open design: we prefer serving requests over blocking
    /// paying users due to an infrastructure issue. Operators should monitor the
    /// `budget_check_skipped` warning logs and set up alerts for sustained Redis
    /// outages that could allow budget overruns.
    pub async fn check_budget(
        &self,
        wallet_address: &str,
        estimated_cost_usdc: f64,
    ) -> Result<(), UsageError> {
        // No Redis configured — apply a conservative per-request hard cap.
        if self.redis_client.is_none() {
            const NO_REDIS_REQUEST_CAP_USDC: f64 = 1.0;
            if estimated_cost_usdc > NO_REDIS_REQUEST_CAP_USDC {
                return Err(UsageError::BudgetExceeded {
                    wallet: wallet_address.to_string(),
                    limit: NO_REDIS_REQUEST_CAP_USDC,
                    spent: estimated_cost_usdc,
                });
            }
            return Ok(());
        }

        // Try Redis hot-path budget check.
        if let Some(client) = &self.redis_client {
            match client.get_multiplexed_async_connection().await {
                Ok(mut conn) => {
                    let day_key =
                        format!("spend:{}:{}", wallet_address, Utc::now().format("%Y-%m-%d"));
                    let daily_spend: f64 = match redis::cmd("GET")
                        .arg(&day_key)
                        .query_async::<Option<f64>>(&mut conn)
                        .await
                    {
                        Ok(Some(val)) => val,
                        Ok(None) => 0.0, // Key doesn't exist yet — no spend today
                        Err(e) => {
                            warn!(
                                key = %day_key,
                                error = %e,
                                "Redis GET failed for daily spend — assuming 0.0 (fail-open)"
                            );
                            0.0
                        }
                    };

                    // Default daily limit: $100 USDC.
                    // Use epsilon-aware comparison to avoid f64 rounding errors.
                    // For example, $99.999999 + $0.000002 should not falsely exceed $100.00.
                    const DAILY_LIMIT_USDC: f64 = 100.0;
                    if daily_spend + estimated_cost_usdc > DAILY_LIMIT_USDC + USDC_EPSILON {
                        return Err(UsageError::BudgetExceeded {
                            wallet: wallet_address.to_string(),
                            limit: DAILY_LIMIT_USDC,
                            spent: daily_spend + estimated_cost_usdc,
                        });
                    }
                }
                Err(e) => {
                    // Fail-open: allow the request but log a warning so operators
                    // can monitor for sustained Redis outages. See doc comment above
                    // for the design rationale.
                    warn!(
                        wallet = %wallet_address,
                        estimated_cost_usdc = estimated_cost_usdc,
                        error = %e,
                        "budget_check_skipped: Redis connection failed, allowing request through (fail-open)"
                    );
                }
            }
        }

        Ok(())
    }

    /// Get spending summary for a wallet.
    pub async fn get_summary(&self, wallet_address: &str) -> Result<SpendSummary, UsageError> {
        if let Some(pool) = &self.db_pool {
            let row: (i64, i64, i64, f64) = sqlx::query_as(
                r#"SELECT
                    COUNT(*) as total_requests,
                    COALESCE(SUM(input_tokens), 0) as total_input,
                    COALESCE(SUM(output_tokens), 0) as total_output,
                    COALESCE(SUM(cost_usdc), 0.0)::DOUBLE PRECISION as total_cost
                FROM spend_logs
                WHERE wallet_address = $1"#,
            )
            .bind(wallet_address)
            .fetch_one(pool)
            .await
            .map_err(|e| UsageError::Database(e.to_string()))?;

            return Ok(SpendSummary {
                wallet_address: wallet_address.to_string(),
                total_requests: row.0 as u64,
                total_input_tokens: row.1 as u64,
                total_output_tokens: row.2 as u64,
                total_cost_usdc: row.3,
                // TODO: Query Redis daily/monthly spend counters to populate these fields.
                // Currently returns 0.0 — see backend audit Q5.
                daily_cost_usdc: 0.0,
                monthly_cost_usdc: 0.0,
            });
        }

        Err(UsageError::NotConfigured)
    }
}

// ---------------------------------------------------------------------------
// Stats query functions (used by routes/stats.rs)
// ---------------------------------------------------------------------------

/// Summary row returned by [`get_wallet_stats`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletStatsSummary {
    pub total_requests: i64,
    pub total_cost: f64,
    pub total_input: i64,
    pub total_output: i64,
}

/// Per-model row returned by [`get_stats_by_model`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatsModelRow {
    pub model: String,
    pub requests: i64,
    pub cost: f64,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// Per-day row returned by [`get_stats_by_day`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatsDayRow {
    pub date: chrono::NaiveDate,
    pub requests: i64,
    pub cost: f64,
}

/// Fetch aggregate spend summary for a wallet over the given number of days.
pub async fn get_wallet_stats(
    pool: &sqlx::PgPool,
    wallet: &str,
    days: i32,
) -> Result<WalletStatsSummary, sqlx::Error> {
    let row: (i64, f64, i64, i64) = sqlx::query_as(
        r#"SELECT COUNT(*) as total_requests,
                  COALESCE(SUM(cost_usdc), 0)::DOUBLE PRECISION as total_cost,
                  COALESCE(SUM(input_tokens), 0) as total_input,
                  COALESCE(SUM(output_tokens), 0) as total_output
           FROM spend_logs
           WHERE wallet_address = $1
             AND created_at >= NOW() - make_interval(days => $2)"#,
    )
    .bind(wallet)
    .bind(days)
    .fetch_one(pool)
    .await?;

    Ok(WalletStatsSummary {
        total_requests: row.0,
        total_cost: row.1,
        total_input: row.2,
        total_output: row.3,
    })
}

/// Fetch per-model spend breakdown for a wallet over the given number of days.
pub async fn get_stats_by_model(
    pool: &sqlx::PgPool,
    wallet: &str,
    days: i32,
) -> Result<Vec<StatsModelRow>, sqlx::Error> {
    let rows: Vec<(String, i64, f64, i64, i64)> = sqlx::query_as(
        r#"SELECT model, COUNT(*) as requests,
                  COALESCE(SUM(cost_usdc), 0)::DOUBLE PRECISION as cost,
                  COALESCE(SUM(input_tokens), 0) as input_tokens,
                  COALESCE(SUM(output_tokens), 0) as output_tokens
           FROM spend_logs
           WHERE wallet_address = $1
             AND created_at >= NOW() - make_interval(days => $2)
           GROUP BY model ORDER BY cost DESC"#,
    )
    .bind(wallet)
    .bind(days)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(model, requests, cost, input_tokens, output_tokens)| StatsModelRow {
                model,
                requests,
                cost,
                input_tokens,
                output_tokens,
            },
        )
        .collect())
}

/// Fetch per-day spend breakdown for a wallet over the given number of days.
pub async fn get_stats_by_day(
    pool: &sqlx::PgPool,
    wallet: &str,
    days: i32,
) -> Result<Vec<StatsDayRow>, sqlx::Error> {
    let rows: Vec<(chrono::NaiveDate, i64, f64)> = sqlx::query_as(
        r#"SELECT DATE(created_at) as date,
                  COUNT(*) as requests,
                  COALESCE(SUM(cost_usdc), 0)::DOUBLE PRECISION as cost
           FROM spend_logs
           WHERE wallet_address = $1
             AND created_at >= NOW() - make_interval(days => $2)
           GROUP BY DATE(created_at) ORDER BY date"#,
    )
    .bind(wallet)
    .bind(days)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(date, requests, cost)| StatsDayRow {
            date,
            requests,
            cost,
        })
        .collect())
}

/// SQL migration for usage tracking tables.
/// Run this against PostgreSQL to create the required tables.
pub const MIGRATION_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS spend_logs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    wallet_address TEXT NOT NULL,
    model TEXT NOT NULL,
    provider TEXT NOT NULL,
    input_tokens INTEGER NOT NULL CHECK (input_tokens >= 0),
    output_tokens INTEGER NOT NULL CHECK (output_tokens >= 0),
    cost_usdc DECIMAL(18, 6) NOT NULL CHECK (cost_usdc >= 0),
    tx_signature TEXT,
    request_id TEXT,
    session_id TEXT,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

-- Reserved for future per-wallet budget customization (see backend audit Q4).
-- Currently created but not queried; budget enforcement uses a hardcoded
-- $100/day limit checked against Redis spend counters in check_budget().
CREATE TABLE IF NOT EXISTS wallet_budgets (
    wallet_address TEXT PRIMARY KEY,
    daily_limit_usdc DECIMAL(18, 6),
    monthly_limit_usdc DECIMAL(18, 6),
    total_spent_usdc DECIMAL(18, 6) DEFAULT 0 CHECK (total_spent_usdc >= 0),
    created_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_spend_wallet ON spend_logs(wallet_address);
CREATE INDEX IF NOT EXISTS idx_spend_created ON spend_logs(created_at);
CREATE INDEX IF NOT EXISTS idx_spend_session ON spend_logs(session_id) WHERE session_id IS NOT NULL;
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spend_log_struct() {
        let log = SpendLog {
            id: Uuid::new_v4(),
            wallet_address: "So11111111111111111111111111111111111111112".to_string(),
            model: "openai/gpt-4o".to_string(),
            provider: "openai".to_string(),
            input_tokens: 150,
            output_tokens: 300,
            cost_usdc: 0.004375,
            tx_signature: Some("5VERv8NMH...".to_string()),
            created_at: Utc::now(),
        };

        // Serialize and deserialize round-trip
        let json = serde_json::to_string(&log).expect("should serialize");
        let deserialized: SpendLog = serde_json::from_str(&json).expect("should deserialize");

        assert_eq!(deserialized.wallet_address, log.wallet_address);
        assert_eq!(deserialized.model, log.model);
        assert_eq!(deserialized.provider, log.provider);
        assert_eq!(deserialized.input_tokens, 150);
        assert_eq!(deserialized.output_tokens, 300);
        assert!((deserialized.cost_usdc - 0.004375).abs() < f64::EPSILON);
        assert_eq!(deserialized.tx_signature, Some("5VERv8NMH...".to_string()));
    }

    #[test]
    fn test_wallet_budget_struct() {
        let budget = WalletBudget {
            wallet_address: "So11111111111111111111111111111111111111112".to_string(),
            daily_limit_usdc: Some(50.0),
            monthly_limit_usdc: Some(500.0),
            total_spent_usdc: 12.50,
            created_at: Utc::now(),
        };

        let json = serde_json::to_string(&budget).expect("should serialize");
        let deserialized: WalletBudget = serde_json::from_str(&json).expect("should deserialize");

        assert_eq!(deserialized.wallet_address, budget.wallet_address);
        assert_eq!(deserialized.daily_limit_usdc, Some(50.0));
        assert_eq!(deserialized.monthly_limit_usdc, Some(500.0));
        assert!((deserialized.total_spent_usdc - 12.50).abs() < f64::EPSILON);
    }

    #[test]
    fn test_noop_tracker_logs_without_error() {
        let tracker = UsageTracker::noop();

        // Should not panic — just logs and returns
        tracker.log_spend(SpendLogEntry {
            wallet_address: "wallet123".to_string(),
            model: "openai/gpt-4o".to_string(),
            provider: "openai".to_string(),
            input_tokens: 100,
            output_tokens: 200,
            cost_usdc: 0.003,
            tx_signature: None,
            request_id: None,
            session_id: None,
        });
    }

    #[test]
    fn test_migration_sql_is_valid() {
        assert!(!MIGRATION_SQL.is_empty());
        assert!(MIGRATION_SQL.contains("spend_logs"));
        assert!(MIGRATION_SQL.contains("wallet_budgets"));
        assert!(MIGRATION_SQL.contains("idx_spend_wallet"));
        assert!(MIGRATION_SQL.contains("idx_spend_created"));
        assert!(MIGRATION_SQL.contains("CREATE TABLE IF NOT EXISTS"));
        assert!(MIGRATION_SQL.contains("CREATE INDEX IF NOT EXISTS"));
        // Phase G: request_id and session_id columns
        assert!(MIGRATION_SQL.contains("request_id TEXT"));
        assert!(MIGRATION_SQL.contains("session_id TEXT"));
        assert!(MIGRATION_SQL.contains("idx_spend_session"));
        // CHECK constraints (must match 001_initial_schema.sql)
        assert!(MIGRATION_SQL.contains("CHECK (input_tokens >= 0)"));
        assert!(MIGRATION_SQL.contains("CHECK (output_tokens >= 0)"));
        assert!(MIGRATION_SQL.contains("CHECK (cost_usdc >= 0)"));
        assert!(MIGRATION_SQL.contains("CHECK (total_spent_usdc >= 0)"));
    }

    #[test]
    fn test_spend_summary_defaults() {
        let summary = SpendSummary {
            wallet_address: "wallet123".to_string(),
            total_requests: 0,
            total_input_tokens: 0,
            total_output_tokens: 0,
            total_cost_usdc: 0.0,
            daily_cost_usdc: 0.0,
            monthly_cost_usdc: 0.0,
        };

        assert_eq!(summary.total_requests, 0_u64);
        assert_eq!(summary.total_input_tokens, 0_u64);
        assert_eq!(summary.total_output_tokens, 0_u64);
        assert!((summary.total_cost_usdc - 0.0).abs() < f64::EPSILON);
        assert!((summary.daily_cost_usdc - 0.0).abs() < f64::EPSILON);
        assert!((summary.monthly_cost_usdc - 0.0).abs() < f64::EPSILON);

        // Verify it serializes correctly
        let json = serde_json::to_string(&summary).expect("should serialize");
        let deserialized: SpendSummary = serde_json::from_str(&json).expect("should deserialize");
        assert_eq!(deserialized.wallet_address, "wallet123");
    }

    #[tokio::test]
    async fn test_noop_tracker_check_budget_passes_at_cap() {
        // Without Redis, the conservative cap is $1.00.  A cost of exactly $1.00
        // (not strictly greater) must be allowed through.
        let tracker = UsageTracker::noop();
        let result = tracker.check_budget("wallet123", 1.0).await;
        assert!(result.is_ok(), "cost equal to cap should be allowed");
    }

    #[tokio::test]
    async fn test_noop_tracker_check_budget_rejects_above_cap() {
        // Without Redis, requests exceeding $1.00 must be rejected to prevent
        // runaway spend on high-cost models.
        let tracker = UsageTracker::noop();
        let result = tracker.check_budget("wallet123", 1.01).await;
        assert!(
            matches!(result, Err(UsageError::BudgetExceeded { .. })),
            "cost above cap should be rejected when Redis is unavailable"
        );
    }

    #[tokio::test]
    async fn test_noop_tracker_check_budget_passes_below_cap() {
        let tracker = UsageTracker::noop();
        let result = tracker.check_budget("wallet123", 0.50).await;
        assert!(result.is_ok(), "cost below cap should be allowed");
    }

    #[tokio::test]
    async fn test_noop_tracker_get_summary_returns_not_configured() {
        let tracker = UsageTracker::noop();
        let result = tracker.get_summary("wallet123").await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, UsageError::NotConfigured));
        assert_eq!(err.to_string(), "not configured");
    }

    #[test]
    fn test_usdc_epsilon_prevents_false_budget_exceed() {
        // USDC_EPSILON must be sub-atomic-unit (< 0.000001) and positive.
        const _: () = {
            assert!(USDC_EPSILON < 0.000_001);
            assert!(USDC_EPSILON > 0.0);
        };

        // Simulate a budget check where f64 rounding causes a tiny overshoot.
        // Without epsilon, this would falsely exceed the $100 limit.
        let daily_limit: f64 = 100.0;
        let daily_spend: f64 = 99.999_999_999_999;
        let estimated_cost: f64 = 0.000_000_000_002;
        let total = daily_spend + estimated_cost;

        // total might be 100.000000000001 due to f64, but should NOT exceed
        // the limit when epsilon is applied.
        assert!(
            total <= daily_limit + USDC_EPSILON,
            "epsilon-aware comparison should not trigger false budget exceeded"
        );
    }

    #[test]
    fn test_usdc_epsilon_still_catches_real_overages() {
        // A genuine overage of $0.01 must still be caught.
        let daily_limit: f64 = 100.0;
        let total: f64 = 100.01;

        assert!(
            total > daily_limit + USDC_EPSILON,
            "real overages must still be caught"
        );
    }

    /// Phase G migration SQL (loaded from migrations/003_phase_g_request_session_ids.sql).
    const PHASE_G_MIGRATION: &str =
        include_str!("../../../migrations/003_phase_g_request_session_ids.sql");

    #[test]
    fn test_phase_g_migration_adds_request_id_and_session_id_columns() {
        // Verify the migration adds both new columns to spend_logs
        assert!(
            PHASE_G_MIGRATION.contains("ADD COLUMN IF NOT EXISTS request_id TEXT"),
            "migration must add request_id column"
        );
        assert!(
            PHASE_G_MIGRATION.contains("ADD COLUMN IF NOT EXISTS session_id TEXT"),
            "migration must add session_id column"
        );
        // Both columns should default to NULL (nullable, no NOT NULL constraint)
        assert!(
            PHASE_G_MIGRATION.contains("request_id TEXT DEFAULT NULL"),
            "request_id must default to NULL"
        );
        assert!(
            PHASE_G_MIGRATION.contains("session_id TEXT DEFAULT NULL"),
            "session_id must default to NULL"
        );
        // All statements must be idempotent (IF NOT EXISTS / IF EXISTS)
        for line in PHASE_G_MIGRATION.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with("--") {
                continue;
            }
            assert!(
                trimmed.contains("IF NOT EXISTS") || trimmed.contains("IF EXISTS"),
                "non-comment SQL statement must be idempotent: {trimmed}"
            );
        }
    }

    #[test]
    fn test_phase_g_migration_creates_partial_index_on_session_id() {
        // Verify the partial index is created on session_id (WHERE NOT NULL)
        assert!(
            PHASE_G_MIGRATION.contains("CREATE INDEX IF NOT EXISTS idx_spend_session"),
            "migration must create idx_spend_session index"
        );
        assert!(
            PHASE_G_MIGRATION.contains("WHERE session_id IS NOT NULL"),
            "session_id index must be partial (WHERE NOT NULL) to avoid bloat on null rows"
        );
    }

    #[test]
    fn test_usage_error_display() {
        let err = UsageError::Database("connection refused".to_string());
        assert_eq!(err.to_string(), "database error: connection refused");

        let err = UsageError::BudgetExceeded {
            wallet: "wallet123".to_string(),
            limit: 100.0,
            spent: 150.0,
        };
        assert!(err.to_string().contains("budget exceeded"));
        assert!(err.to_string().contains("wallet123"));

        let err = UsageError::Redis("timeout".to_string());
        assert_eq!(err.to_string(), "redis error: timeout");

        let err = UsageError::NotConfigured;
        assert_eq!(err.to_string(), "not configured");
    }
}
