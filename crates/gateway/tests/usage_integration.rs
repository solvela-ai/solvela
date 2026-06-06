//! Integration tests for `gateway::usage` against a real Postgres.
//!
//! Covers the DB-backed and fully-pure paths of `UsageTracker` and the
//! free-standing stats functions. Redis-backed budget enforcement paths
//! are left for a follow-up — they require a fresh Redis instance per test
//! and a different fixture story.

use sqlx::PgPool;
use uuid::Uuid;

use gateway::usage::{
    get_stats_by_day, get_stats_by_model, get_wallet_stats, BudgetConfig, SpendLogEntry,
    UsageError, UsageTracker,
};

const WALLET_A: &str = "DZNuWFpYwzEAcyaMQzqgbxA4d1f4Yq8EU2YbLPLYxNTw";
const WALLET_B: &str = "GbZ6oTmEa9PEd6FGQEmTm3GbXCQkAGqNGGJYXrW8oQte";

/// Seed N spend rows directly via SQL — bypassing `log_spend`'s `tokio::spawn`
/// so the read-side tests are deterministic.
async fn seed_spend(pool: &PgPool, wallet: &str, rows: &[(&str, &str, i32, i32, f64)]) {
    for (model, provider, input_tokens, output_tokens, cost) in rows {
        sqlx::query(
            r#"INSERT INTO spend_logs (id, wallet_address, model, provider,
                input_tokens, output_tokens, cost_usdc, created_at)
               VALUES ($1, $2, $3, $4, $5, $6, $7, NOW())"#,
        )
        .bind(Uuid::new_v4())
        .bind(wallet)
        .bind(model)
        .bind(provider)
        .bind(input_tokens)
        .bind(output_tokens)
        .bind(cost)
        .execute(pool)
        .await
        .expect("seed spend_logs insert");
    }
}

#[test]
fn budget_config_default_uses_100_dollar_daily_cap() {
    let cfg = BudgetConfig::default();
    assert!(cfg.hourly.is_none(), "default has no hourly cap");
    assert_eq!(
        cfg.daily,
        Some(100.0),
        "default daily cap matches DEFAULT_DAILY_LIMIT_USDC"
    );
    assert!(cfg.monthly.is_none(), "default has no monthly cap");
}

#[test]
fn usage_error_display_matches_thiserror_format() {
    let err = UsageError::Database("connection refused".into());
    assert_eq!(err.to_string(), "database error: connection refused");

    let err = UsageError::Redis("timeout".into());
    assert_eq!(err.to_string(), "redis error: timeout");

    let err = UsageError::NotConfigured;
    assert_eq!(err.to_string(), "not configured");

    let err = UsageError::BudgetExceeded {
        wallet: "WALLET".into(),
        limit: 1.0,
        spent: 1.5,
    };
    let s = err.to_string();
    assert!(s.contains("WALLET"), "wallet must appear: {s}");
    assert!(
        s.contains("$1.5000"),
        "spent must format with 4 decimals: {s}"
    );
    assert!(
        s.contains("$1.00"),
        "limit must format with 2 decimals: {s}"
    );
}

#[tokio::test]
async fn noop_tracker_has_no_backends() {
    let tracker = UsageTracker::noop();
    assert!(
        tracker.redis_client().is_none(),
        "noop tracker exposes no redis client"
    );
}

#[tokio::test]
async fn check_budget_without_redis_applies_one_dollar_per_request_cap() {
    let tracker = UsageTracker::noop();

    // Below the cap — allowed.
    tracker
        .check_budget(WALLET_A, 0.50)
        .await
        .expect("0.50 USDC must be within the no-Redis cap");

    // At the cap — allowed (boundary).
    tracker
        .check_budget(WALLET_A, 1.00)
        .await
        .expect("$1.00 USDC must be at-or-below the cap");

    // Above the cap — denied.
    let err = tracker
        .check_budget(WALLET_A, 1.01)
        .await
        .expect_err("anything above $1 must be rejected without Redis");
    match err {
        UsageError::BudgetExceeded {
            wallet,
            limit,
            spent,
        } => {
            assert_eq!(wallet, WALLET_A);
            assert_eq!(limit, 1.0);
            assert_eq!(spent, 1.01);
        }
        other => panic!("expected BudgetExceeded, got {other:?}"),
    }
}

#[tokio::test]
async fn get_summary_without_db_returns_not_configured() {
    let tracker = UsageTracker::noop();
    let result = tracker.get_summary(WALLET_A).await;
    assert!(matches!(result, Err(UsageError::NotConfigured)));
}

#[sqlx::test(migrations = "../../migrations")]
async fn get_summary_aggregates_spend_logs(pool: PgPool) {
    seed_spend(
        &pool,
        WALLET_A,
        &[
            ("openai/gpt-4o", "openai", 100, 200, 0.0050),
            ("openai/gpt-4o", "openai", 50, 150, 0.0025),
            ("anthropic/claude-3.5", "anthropic", 200, 400, 0.0080),
        ],
    )
    .await;
    // Other wallet must not pollute results.
    seed_spend(
        &pool,
        WALLET_B,
        &[("openai/gpt-4o", "openai", 999, 999, 9.99)],
    )
    .await;

    let tracker = UsageTracker::new(Some(pool.clone()), None);
    let summary = tracker.get_summary(WALLET_A).await.expect("summary");

    assert_eq!(summary.wallet_address, WALLET_A);
    assert_eq!(summary.total_requests, 3);
    assert_eq!(summary.total_input_tokens, 350);
    assert_eq!(summary.total_output_tokens, 750);
    assert!(
        (summary.total_cost_usdc - 0.0155).abs() < 1e-9,
        "summed cost must be 0.0155, got {}",
        summary.total_cost_usdc
    );
    // daily/monthly Redis aggregations are stubbed at 0 in the source.
    assert_eq!(summary.daily_cost_usdc, 0.0);
    assert_eq!(summary.monthly_cost_usdc, 0.0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn log_spend_persists_row_when_db_configured(pool: PgPool) {
    let tracker = UsageTracker::new(Some(pool.clone()), None);

    tracker.log_spend(SpendLogEntry {
        wallet_address: WALLET_A.to_string(),
        model: "openai/gpt-4o".to_string(),
        provider: "openai".to_string(),
        input_tokens: 12,
        output_tokens: 34,
        cost_usdc: 0.0009,
        tx_signature: Some("tx-test-sig".to_string()),
        request_id: Some("req-abc".to_string()),
        session_id: None,
        tenant: None,
        estimated_cost_usdc: None,
    });

    // Fire-and-forget: poll briefly for the row to land.
    let mut found = false;
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*)::BIGINT FROM spend_logs WHERE wallet_address = $1")
                .bind(WALLET_A)
                .fetch_one(&pool)
                .await
                .expect("count");
        if count == 1 {
            found = true;
            break;
        }
    }
    assert!(
        found,
        "log_spend should have persisted exactly one row within 1s"
    );

    let row: (
        String,
        String,
        i32,
        i32,
        f64,
        Option<String>,
        Option<String>,
    ) = sqlx::query_as(
        r#"SELECT model, provider, input_tokens, output_tokens,
                  cost_usdc::DOUBLE PRECISION, tx_signature, request_id
           FROM spend_logs WHERE wallet_address = $1"#,
    )
    .bind(WALLET_A)
    .fetch_one(&pool)
    .await
    .expect("read back");

    assert_eq!(row.0, "openai/gpt-4o");
    assert_eq!(row.1, "openai");
    assert_eq!(row.2, 12);
    assert_eq!(row.3, 34);
    assert!((row.4 - 0.0009).abs() < 1e-9);
    assert_eq!(row.5.as_deref(), Some("tx-test-sig"));
    assert_eq!(row.6.as_deref(), Some("req-abc"));
}

#[tokio::test]
async fn log_spend_with_no_backends_does_not_panic() {
    // Pure smoke: emits a tracing event without touching DB or Redis.
    let tracker = UsageTracker::noop();
    tracker.log_spend(SpendLogEntry {
        wallet_address: WALLET_A.to_string(),
        model: "openai/gpt-4o".to_string(),
        provider: "openai".to_string(),
        input_tokens: 1,
        output_tokens: 2,
        cost_usdc: 0.0001,
        tx_signature: None,
        request_id: None,
        session_id: None,
        tenant: None,
        estimated_cost_usdc: None,
    });
}

#[sqlx::test(migrations = "../../migrations")]
async fn get_wallet_stats_aggregates_within_window(pool: PgPool) {
    seed_spend(
        &pool,
        WALLET_A,
        &[
            ("openai/gpt-4o", "openai", 100, 200, 0.005),
            ("openai/gpt-4o", "openai", 50, 100, 0.002),
        ],
    )
    .await;

    let stats = get_wallet_stats(&pool, WALLET_A, 7).await.expect("stats");
    assert_eq!(stats.total_requests, 2);
    assert!((stats.total_cost - 0.007).abs() < 1e-9);
    assert_eq!(stats.total_input, 150);
    assert_eq!(stats.total_output, 300);
}

#[sqlx::test(migrations = "../../migrations")]
async fn get_wallet_stats_returns_zeros_for_unknown_wallet(pool: PgPool) {
    let stats = get_wallet_stats(&pool, "no-such-wallet", 7)
        .await
        .expect("stats");
    assert_eq!(stats.total_requests, 0);
    assert_eq!(stats.total_cost, 0.0);
    assert_eq!(stats.total_input, 0);
    assert_eq!(stats.total_output, 0);
}

#[sqlx::test(migrations = "../../migrations")]
async fn get_stats_by_model_groups_and_orders_by_cost_desc(pool: PgPool) {
    seed_spend(
        &pool,
        WALLET_A,
        &[
            ("openai/gpt-4o", "openai", 100, 200, 0.001),
            ("openai/gpt-4o", "openai", 100, 200, 0.002),
            ("anthropic/claude-3.5", "anthropic", 50, 50, 0.010),
        ],
    )
    .await;

    let rows = get_stats_by_model(&pool, WALLET_A, 7)
        .await
        .expect("by_model");
    assert_eq!(rows.len(), 2);
    // Ordered by total cost DESC — anthropic ($0.010) outranks openai ($0.003).
    assert_eq!(rows[0].model, "anthropic/claude-3.5");
    assert!((rows[0].cost - 0.010).abs() < 1e-9);
    assert_eq!(rows[0].requests, 1);

    assert_eq!(rows[1].model, "openai/gpt-4o");
    assert!((rows[1].cost - 0.003).abs() < 1e-9);
    assert_eq!(rows[1].requests, 2);
    assert_eq!(rows[1].input_tokens, 200);
    assert_eq!(rows[1].output_tokens, 400);
}

#[sqlx::test(migrations = "../../migrations")]
async fn get_stats_by_day_groups_today(pool: PgPool) {
    seed_spend(
        &pool,
        WALLET_A,
        &[
            ("openai/gpt-4o", "openai", 100, 200, 0.005),
            ("openai/gpt-4o", "openai", 50, 100, 0.002),
        ],
    )
    .await;

    let rows = get_stats_by_day(&pool, WALLET_A, 7).await.expect("by_day");
    // Both rows seeded NOW() so they collapse into a single day bucket.
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].requests, 2);
    assert!((rows[0].cost - 0.007).abs() < 1e-9);
}

#[sqlx::test(migrations = "../../migrations")]
async fn get_stats_filters_outside_window(pool: PgPool) {
    // Seed a row dated 30 days ago — outside the 7-day window.
    sqlx::query(
        r#"INSERT INTO spend_logs (id, wallet_address, model, provider,
            input_tokens, output_tokens, cost_usdc, created_at)
           VALUES ($1, $2, 'old-model', 'old', 1, 1, 99.99, NOW() - INTERVAL '30 days')"#,
    )
    .bind(Uuid::new_v4())
    .bind(WALLET_A)
    .execute(&pool)
    .await
    .expect("seed old row");

    // And one row inside the window.
    seed_spend(&pool, WALLET_A, &[("openai/gpt-4o", "openai", 1, 1, 0.001)]).await;

    let stats = get_wallet_stats(&pool, WALLET_A, 7).await.expect("stats");
    assert_eq!(
        stats.total_requests, 1,
        "30-day-old row must be filtered out by the 7-day window"
    );
    assert!((stats.total_cost - 0.001).abs() < 1e-9);
}
