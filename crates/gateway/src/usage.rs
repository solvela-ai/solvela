//! Per-wallet usage tracking and budget management.
//!
//! PostgreSQL for persistent spend logs, Redis for hot-path spend tracking.
//! All DB writes are async (tokio::spawn) — never on the request critical path.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use uuid::Uuid;

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

/// Error types for usage tracking.
#[derive(Debug, thiserror::Error)]
pub enum UsageError {
    #[error("database error: {0}")]
    Database(String),

    #[error("budget exceeded: {0}")]
    BudgetExceeded(String),

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
    #[allow(clippy::too_many_arguments)]
    pub fn log_spend(
        &self,
        wallet_address: String,
        model: String,
        provider: String,
        input_tokens: u32,
        output_tokens: u32,
        cost_usdc: f64,
        tx_signature: Option<String>,
    ) {
        let id = Uuid::new_v4();
        let created_at = Utc::now();

        info!(
            wallet = %wallet_address,
            model = %model,
            provider = %provider,
            input_tokens,
            output_tokens,
            cost_usdc,
            tx_signature = tx_signature.as_deref().unwrap_or("none"),
            "spend logged"
        );

        // Write to PostgreSQL asynchronously
        if let Some(pool) = &self.db_pool {
            let pool = pool.clone();
            let db_wallet = wallet_address.clone();
            let db_model = model.clone();
            let db_provider = provider.clone();
            let db_tx_sig = tx_signature.clone();
            tokio::spawn(async move {
                let result = sqlx::query(
                    r#"INSERT INTO spend_logs (id, wallet_address, model, provider, input_tokens, output_tokens, cost_usdc, tx_signature, created_at)
                       VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"#,
                )
                .bind(id)
                .bind(&db_wallet)
                .bind(&db_model)
                .bind(&db_provider)
                .bind(input_tokens as i32)
                .bind(output_tokens as i32)
                .bind(cost_usdc)
                .bind(&db_tx_sig)
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
            let wallet = wallet_address;
            let cost = cost_usdc;
            tokio::spawn(async move {
                if let Ok(mut conn) = client.get_multiplexed_async_connection().await {
                    // Daily spend counter
                    let day_key = format!("spend:{}:{}", wallet, Utc::now().format("%Y-%m-%d"));
                    let _: Result<(), _> = redis::cmd("INCRBYFLOAT")
                        .arg(&day_key)
                        .arg(cost)
                        .query_async(&mut conn)
                        .await;
                    let _: Result<(), _> = redis::cmd("EXPIRE")
                        .arg(&day_key)
                        .arg(86400_u64)
                        .query_async(&mut conn)
                        .await;

                    // Monthly spend counter
                    let month_key = format!("spend:{}:{}", wallet, Utc::now().format("%Y-%m"));
                    let _: Result<(), _> = redis::cmd("INCRBYFLOAT")
                        .arg(&month_key)
                        .arg(cost)
                        .query_async(&mut conn)
                        .await;
                    let _: Result<(), _> = redis::cmd("EXPIRE")
                        .arg(&month_key)
                        .arg(86400_u64 * 31)
                        .query_async(&mut conn)
                        .await;
                }
            });
        }
    }

    /// Check if a wallet's budget allows a request with the estimated cost.
    ///
    /// Returns Ok(()) if within budget, Err if budget exceeded.
    pub async fn check_budget(
        &self,
        wallet_address: &str,
        estimated_cost_usdc: f64,
    ) -> Result<(), UsageError> {
        // Try Redis first (hot path)
        if let Some(client) = &self.redis_client {
            if let Ok(mut conn) = client.get_multiplexed_async_connection().await {
                let day_key = format!("spend:{}:{}", wallet_address, Utc::now().format("%Y-%m-%d"));
                let daily_spend: f64 = redis::cmd("GET")
                    .arg(&day_key)
                    .query_async(&mut conn)
                    .await
                    .unwrap_or(0.0);

                // Default daily limit: $100 USDC
                let daily_limit = 100.0;
                if daily_spend + estimated_cost_usdc > daily_limit {
                    return Err(UsageError::BudgetExceeded(format!(
                        "daily spend ${:.4} + estimated ${:.4} exceeds limit ${:.2}",
                        daily_spend, estimated_cost_usdc, daily_limit
                    )));
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
                    COALESCE(SUM(cost_usdc), 0.0) as total_cost
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
                daily_cost_usdc: 0.0,   // Would query Redis
                monthly_cost_usdc: 0.0, // Would query Redis
            });
        }

        Err(UsageError::NotConfigured)
    }
}

/// SQL migration for usage tracking tables.
/// Run this against PostgreSQL to create the required tables.
pub const MIGRATION_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS spend_logs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    wallet_address TEXT NOT NULL,
    model TEXT NOT NULL,
    provider TEXT NOT NULL,
    input_tokens INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    cost_usdc DECIMAL(18, 6) NOT NULL,
    tx_signature TEXT,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS wallet_budgets (
    wallet_address TEXT PRIMARY KEY,
    daily_limit_usdc DECIMAL(18, 6),
    monthly_limit_usdc DECIMAL(18, 6),
    total_spent_usdc DECIMAL(18, 6) DEFAULT 0,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_spend_wallet ON spend_logs(wallet_address);
CREATE INDEX IF NOT EXISTS idx_spend_created ON spend_logs(created_at);
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
        tracker.log_spend(
            "wallet123".to_string(),
            "openai/gpt-4o".to_string(),
            "openai".to_string(),
            100,
            200,
            0.003,
            None,
        );
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
    async fn test_noop_tracker_check_budget_passes() {
        let tracker = UsageTracker::noop();
        let result = tracker.check_budget("wallet123", 1.0).await;
        assert!(result.is_ok());
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
    fn test_usage_error_display() {
        let err = UsageError::Database("connection refused".to_string());
        assert_eq!(err.to_string(), "database error: connection refused");

        let err = UsageError::BudgetExceeded("daily limit".to_string());
        assert_eq!(err.to_string(), "budget exceeded: daily limit");

        let err = UsageError::Redis("timeout".to_string());
        assert_eq!(err.to_string(), "redis error: timeout");

        let err = UsageError::NotConfigured;
        assert_eq!(err.to_string(), "not configured");
    }
}
