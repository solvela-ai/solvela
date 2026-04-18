//! Integration test: all migration files in `migrations/` apply cleanly to a
//! fresh Postgres database and produce the expected schema.
//!
//! Requires a running Postgres. Opt-in via the `TEST_DATABASE_URL` env var:
//!
//! ```sh
//! docker compose up -d
//! docker compose exec -T postgres psql -U solvela -d solvela \
//!     -c "DROP DATABASE IF EXISTS solvela_migrate_test; CREATE DATABASE solvela_migrate_test;"
//! TEST_DATABASE_URL=postgres://solvela:solvela_dev_password@localhost:5432/solvela_migrate_test \
//!     cargo test -p gateway --test migrations -- --ignored
//! ```

use sqlx::postgres::PgPoolOptions;
use sqlx::Row;

/// Tables that all seven migration files, applied in order, must produce.
const EXPECTED_TABLES: &[&str] = &[
    "spend_logs",
    "wallet_budgets",
    "escrow_claim_queue",
    "organizations",
    "teams",
    "org_members",
    "team_wallets",
    "api_keys",
    "audit_logs",
    "team_budgets",
];

/// Columns whose presence proves the right ALTER TABLE migrations ran.
const EXPECTED_COLUMNS: &[(&str, &str)] = &[
    ("spend_logs", "request_id"),            // migration 003
    ("spend_logs", "session_id"),            // migration 003
    ("escrow_claim_queue", "next_retry_at"), // migration 004
    ("wallet_budgets", "updated_at"),        // migration 001
    ("wallet_budgets", "hourly_limit_usdc"), // migration 007
];

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing at a throwaway Postgres"]
async fn all_migrations_apply_cleanly() {
    let url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must be set to a fresh Postgres database");

    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .expect("failed to connect to TEST_DATABASE_URL");

    sqlx::migrate!("../../migrations")
        .run(&pool)
        .await
        .expect("migrations should apply cleanly to a fresh database");

    for table in EXPECTED_TABLES {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM information_schema.tables
                WHERE table_schema = 'public' AND table_name = $1
            )",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|e| panic!("failed to query for table {table}: {e}"));

        assert!(
            exists,
            "expected table `{table}` not found in public schema"
        );
    }

    for (table, column) in EXPECTED_COLUMNS {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM information_schema.columns
                WHERE table_schema = 'public'
                  AND table_name = $1
                  AND column_name = $2
            )",
        )
        .bind(table)
        .bind(column)
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|e| panic!("failed to query for column {table}.{column}: {e}"));

        assert!(
            exists,
            "expected column `{table}.{column}` not found — migration likely missing"
        );
    }

    let migrations_run: i64 =
        sqlx::query("SELECT COUNT(*)::BIGINT AS n FROM _sqlx_migrations WHERE success = TRUE")
            .fetch_one(&pool)
            .await
            .expect("failed to query _sqlx_migrations")
            .get("n");

    // Count .sql files in the migrations/ directory at test time so this
    // assertion auto-tracks new migrations instead of failing with a stale
    // hardcoded count. Cross-checks that every file got applied.
    let expected_migrations = std::fs::read_dir("../../migrations")
        .expect("migrations directory should exist at test time")
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry
                .path()
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext == "sql")
        })
        .count() as i64;

    assert_eq!(
        migrations_run, expected_migrations,
        "_sqlx_migrations should have one row per .sql file in migrations/ \
         (got {migrations_run}, expected {expected_migrations})"
    );

    // Regression guard: the wallet_budgets `updated_at` trigger must fire on
    // UPDATE. Migration 005 repoints the trigger to a new generic function;
    // if that rewiring silently breaks, every UPDATE on wallet_budgets would
    // fail at trigger-fire time. This is a behavioral check beyond schema shape.
    sqlx::query("DELETE FROM wallet_budgets WHERE wallet_address = 'TEST_TRIGGER_WALLET'")
        .execute(&pool)
        .await
        .expect("pre-test cleanup");

    sqlx::query("INSERT INTO wallet_budgets (wallet_address) VALUES ('TEST_TRIGGER_WALLET')")
        .execute(&pool)
        .await
        .expect("insert wallet_budgets row");

    let created_at: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "SELECT created_at FROM wallet_budgets WHERE wallet_address = 'TEST_TRIGGER_WALLET'",
    )
    .fetch_one(&pool)
    .await
    .expect("read created_at");

    // Brief sleep so the timestamp delta is measurable.
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    sqlx::query(
        "UPDATE wallet_budgets SET daily_limit_usdc = 100 \
         WHERE wallet_address = 'TEST_TRIGGER_WALLET'",
    )
    .execute(&pool)
    .await
    .expect("update wallet_budgets row");

    let updated_at: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "SELECT updated_at FROM wallet_budgets WHERE wallet_address = 'TEST_TRIGGER_WALLET'",
    )
    .fetch_one(&pool)
    .await
    .expect("read updated_at");

    assert!(
        updated_at > created_at,
        "trg_wallet_budgets_updated_at must fire on UPDATE \
         (updated_at {updated_at} should be > created_at {created_at})"
    );

    // Regression guard: spend_logs CHECK constraints must actually reject invalid
    // data. The deleted test_migration_sql_is_valid asserted these as substrings;
    // this is the runtime-enforcement equivalent.
    let neg_tokens_err = sqlx::query(
        "INSERT INTO spend_logs (wallet_address, model, provider, \
         input_tokens, output_tokens, cost_usdc) \
         VALUES ('CHECK_WALLET', 'test-model', 'test-provider', -1, 0, 0)",
    )
    .execute(&pool)
    .await;
    assert!(
        neg_tokens_err.is_err(),
        "CHECK (input_tokens >= 0) must reject negative values"
    );

    let neg_cost_err = sqlx::query(
        "INSERT INTO spend_logs (wallet_address, model, provider, \
         input_tokens, output_tokens, cost_usdc) \
         VALUES ('CHECK_WALLET', 'test-model', 'test-provider', 0, 0, -0.01)",
    )
    .execute(&pool)
    .await;
    assert!(
        neg_cost_err.is_err(),
        "CHECK (cost_usdc >= 0) must reject negative values"
    );
}
