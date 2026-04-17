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

    assert_eq!(
        migrations_run, 7,
        "expected 7 applied migrations, got {migrations_run}"
    );
}
