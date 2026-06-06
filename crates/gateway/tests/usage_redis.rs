//! Redis-backed integration tests for `gateway::usage`.
//!
//! These exercise the hot-path counters in `log_spend`, the Redis
//! happy-path of `check_budget`, the cache-then-DB helpers
//! (`get_wallet_budget_config`, `get_team_for_wallet`,
//! `get_team_budget_config`), and team-level enforcement.
//!
//! Per-test isolation: each test generates a UUID-prefixed wallet address.
//! Spend, budget-config, and team-membership keys all embed the wallet,
//! so two parallel tests never collide. Postgres isolation comes from
//! `#[sqlx::test]` (fresh DB per test).

use chrono::Utc;
use sqlx::PgPool;
use uuid::Uuid;

use gateway::usage::{get_redis_spend, SpendLogEntry, UsageError, UsageTracker};

const REDIS_URL: &str = "redis://127.0.0.1:6379";

fn redis_client() -> redis::Client {
    redis::Client::open(REDIS_URL).expect("local Redis must be reachable")
}

/// Generate a per-test wallet so spend / budget_config / team_member
/// keys are isolated even when tests run in parallel.
fn unique_wallet() -> String {
    format!("test_w_{}", Uuid::new_v4().simple())
}

/// Wait for a Redis key to materialize (set by `tokio::spawn` inside `log_spend`).
/// Polls every 20ms for up to 1s.
async fn wait_for_key(client: &redis::Client, key: &str) -> Option<String> {
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let mut conn = client
            .get_multiplexed_async_connection()
            .await
            .expect("redis conn");
        if let Ok(Some(val)) = redis::cmd("GET")
            .arg(key)
            .query_async::<Option<String>>(&mut conn)
            .await
        {
            return Some(val);
        }
    }
    None
}

/// Best-effort cleanup of test keys.
async fn redis_del(client: &redis::Client, keys: &[&str]) {
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("redis conn");
    for key in keys {
        let _: Result<i64, _> = redis::cmd("DEL").arg(*key).query_async(&mut conn).await;
    }
}

#[sqlx::test(migrations = "../../migrations")]
async fn log_spend_writes_redis_hourly_daily_monthly_counters(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();
    let now = Utc::now();

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));
    tracker.log_spend(SpendLogEntry {
        wallet_address: wallet.clone(),
        model: "openai/gpt-4o".to_string(),
        provider: "openai".to_string(),
        input_tokens: 100,
        output_tokens: 200,
        cost_usdc: 0.0050,
        tx_signature: None,
        request_id: None,
        session_id: None,
        tenant: None,
        estimated_cost_usdc: None,
    });

    let hour_key = format!("spend:{}:{}", wallet, now.format("%Y-%m-%dT%H"));
    let day_key = format!("spend:{}:{}", wallet, now.format("%Y-%m-%d"));
    let month_key = format!("spend:{}:{}", wallet, now.format("%Y-%m"));

    let hour_val = wait_for_key(&client, &hour_key)
        .await
        .expect("hourly counter must appear");
    assert_eq!(hour_val.parse::<f64>().unwrap_or(0.0), 0.005);

    let day_val = wait_for_key(&client, &day_key)
        .await
        .expect("daily counter must appear");
    assert_eq!(day_val.parse::<f64>().unwrap_or(0.0), 0.005);

    let month_val = wait_for_key(&client, &month_key)
        .await
        .expect("monthly counter must appear");
    assert_eq!(month_val.parse::<f64>().unwrap_or(0.0), 0.005);

    let via_helper = get_redis_spend(&client, &hour_key)
        .await
        .expect("get_redis_spend");
    assert_eq!(via_helper, 0.005);

    redis_del(&client, &[&hour_key, &day_key, &month_key]).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn log_spend_accumulates_across_calls(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();
    let now = Utc::now();
    let day_key = format!("spend:{}:{}", wallet, now.format("%Y-%m-%d"));

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));
    let entry = SpendLogEntry {
        wallet_address: wallet.clone(),
        model: "openai/gpt-4o".to_string(),
        provider: "openai".to_string(),
        input_tokens: 1,
        output_tokens: 1,
        cost_usdc: 0.001,
        tx_signature: None,
        request_id: None,
        session_id: None,
        tenant: None,
        estimated_cost_usdc: None,
    };
    tracker.log_spend(entry.clone());
    tracker.log_spend(entry.clone());
    tracker.log_spend(entry);

    let mut total: f64 = 0.0;
    for _ in 0..100 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        if let Ok(val) = get_redis_spend(&client, &day_key).await {
            total = val;
            if (total - 0.003).abs() < 1e-9 {
                break;
            }
        }
    }
    assert!(
        (total - 0.003).abs() < 1e-9,
        "three log_spend calls of 0.001 must accumulate to 0.003, got {total}"
    );

    redis_del(&client, &[&day_key]).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn check_budget_passes_when_under_default_daily_limit(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));

    tracker
        .check_budget(&wallet, 0.50, None)
        .await
        .expect("default budget must allow $0.50");

    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("conn");
    let cache_key = format!("budget_config:{wallet}");
    let cached: Option<String> = redis::cmd("GET")
        .arg(&cache_key)
        .query_async(&mut conn)
        .await
        .expect("get cache");
    assert!(
        cached.is_some(),
        "wallet budget config must be cached after the first lookup"
    );

    redis_del(&client, &[&cache_key, &format!("team_member:{wallet}")]).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn check_budget_rejects_when_daily_limit_would_be_exceeded(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();
    let now = Utc::now();

    sqlx::query("INSERT INTO wallet_budgets (wallet_address, daily_limit_usdc) VALUES ($1, 1.00)")
        .bind(&wallet)
        .execute(&pool)
        .await
        .expect("seed wallet_budget");

    let day_key = format!("spend:{}:{}", wallet, now.format("%Y-%m-%d"));
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("conn");
    let _: () = redis::cmd("SET")
        .arg(&day_key)
        .arg("0.95")
        .arg("EX")
        .arg(3600)
        .query_async(&mut conn)
        .await
        .expect("seed spend");

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));

    tracker
        .check_budget(&wallet, 0.04, None)
        .await
        .expect("$0.04 must fit");

    let err = tracker
        .check_budget(&wallet, 0.10, None)
        .await
        .expect_err("must reject when over the daily limit");
    match err {
        UsageError::BudgetExceeded {
            wallet: w,
            limit,
            spent,
        } => {
            assert_eq!(w, wallet);
            assert_eq!(limit, 1.0);
            // H1 (atomic check-and-commit): the prior `check_budget(0.04)`
            // committed to the daily counter, so it's now 0.99. Adding
            // 0.10 would land at 1.09 (which is what `spent` reports as
            // the would-have-been amount). Pre-H1 this was 0.95 + 0.10
            // = 1.05 because the check was read-only.
            assert!(
                (spent - 1.09).abs() < 1e-9,
                "expected spent ≈ 1.09 (post-add value), got {spent}"
            );
        }
        other => panic!("expected BudgetExceeded, got {other:?}"),
    }

    redis_del(
        &client,
        &[
            &day_key,
            &format!("budget_config:{wallet}"),
            &format!("team_member:{wallet}"),
        ],
    )
    .await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn check_budget_rejects_when_hourly_limit_would_be_exceeded(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();
    let now = Utc::now();

    sqlx::query("INSERT INTO wallet_budgets (wallet_address, hourly_limit_usdc) VALUES ($1, 0.50)")
        .bind(&wallet)
        .execute(&pool)
        .await
        .expect("seed");

    let hour_key = format!("spend:{}:{}", wallet, now.format("%Y-%m-%dT%H"));
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("conn");
    let _: () = redis::cmd("SET")
        .arg(&hour_key)
        .arg("0.49")
        .arg("EX")
        .arg(3600)
        .query_async(&mut conn)
        .await
        .expect("seed");

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));

    let err = tracker
        .check_budget(&wallet, 0.10, None)
        .await
        .expect_err("over hourly cap");
    match err {
        UsageError::BudgetExceeded { limit, .. } => assert_eq!(limit, 0.5),
        other => panic!("expected BudgetExceeded, got {other:?}"),
    }

    redis_del(
        &client,
        &[
            &hour_key,
            &format!("budget_config:{wallet}"),
            &format!("team_member:{wallet}"),
        ],
    )
    .await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn check_budget_rejects_when_monthly_limit_would_be_exceeded(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();
    let now = Utc::now();

    sqlx::query(
        "INSERT INTO wallet_budgets (wallet_address, monthly_limit_usdc) VALUES ($1, 5.00)",
    )
    .bind(&wallet)
    .execute(&pool)
    .await
    .expect("seed");

    let month_key = format!("spend:{}:{}", wallet, now.format("%Y-%m"));
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("conn");
    let _: () = redis::cmd("SET")
        .arg(&month_key)
        .arg("4.99")
        .arg("EX")
        .arg(3600)
        .query_async(&mut conn)
        .await
        .expect("seed");

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));

    let err = tracker
        .check_budget(&wallet, 0.10, None)
        .await
        .expect_err("over monthly cap");
    match err {
        UsageError::BudgetExceeded { limit, .. } => assert_eq!(limit, 5.0),
        other => panic!("expected BudgetExceeded, got {other:?}"),
    }

    redis_del(
        &client,
        &[
            &month_key,
            &format!("budget_config:{wallet}"),
            &format!("team_member:{wallet}"),
        ],
    )
    .await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn check_budget_caches_team_membership_as_none_for_non_members(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));
    tracker
        .check_budget(&wallet, 0.10, None)
        .await
        .expect("default $100/day must allow $0.10");

    let cache_key = format!("team_member:{wallet}");
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("conn");
    let cached: Option<String> = redis::cmd("GET")
        .arg(&cache_key)
        .query_async(&mut conn)
        .await
        .expect("get cache");
    assert_eq!(
        cached.as_deref(),
        Some("none"),
        "non-member wallets should be cached as 'none' to avoid repeated DB misses"
    );

    redis_del(&client, &[&cache_key, &format!("budget_config:{wallet}")]).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn check_budget_uses_pre_populated_redis_cache(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();

    // Pre-populate cache with a tight $0.05/day cap. NO matching DB row,
    // so reaching the DB would yield the default $100. Exceeding the
    // cached $0.05 proves the cache (not the default) was consulted.
    let cache_key = format!("budget_config:{wallet}");
    let cached_config = r#"{"hourly":null,"daily":0.05,"monthly":null}"#;
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("conn");
    let _: () = redis::cmd("SET")
        .arg(&cache_key)
        .arg(cached_config)
        .arg("EX")
        .arg(60)
        .query_async(&mut conn)
        .await
        .expect("prime cache");

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));

    tracker
        .check_budget(&wallet, 0.04, None)
        .await
        .expect("$0.04 fits under cached cap");

    let err = tracker
        .check_budget(&wallet, 0.06, None)
        .await
        .expect_err("must reject under the cached cap");
    match err {
        UsageError::BudgetExceeded { limit, .. } => {
            assert!(
                (limit - 0.05).abs() < 1e-9,
                "limit must be cached 0.05, got {limit}"
            );
        }
        other => panic!("expected BudgetExceeded, got {other:?}"),
    }

    redis_del(&client, &[&cache_key, &format!("team_member:{wallet}")]).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn check_budget_falls_through_to_db_when_cache_is_corrupt(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();

    sqlx::query("INSERT INTO wallet_budgets (wallet_address, daily_limit_usdc) VALUES ($1, 1.00)")
        .bind(&wallet)
        .execute(&pool)
        .await
        .expect("seed");

    let cache_key = format!("budget_config:{wallet}");
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("conn");
    let _: () = redis::cmd("SET")
        .arg(&cache_key)
        .arg("not-valid-json")
        .arg("EX")
        .arg(60)
        .query_async(&mut conn)
        .await
        .expect("prime corrupt cache");

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));

    tracker
        .check_budget(&wallet, 0.50, None)
        .await
        .expect("must use DB after detecting corrupt cache");

    redis_del(&client, &[&cache_key, &format!("team_member:{wallet}")]).await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn log_spend_writes_team_counters_when_wallet_in_team(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();

    let owner_wallet = format!("test_owner_{}", Uuid::new_v4().simple());
    let org_id: Uuid = sqlx::query_scalar(
        "INSERT INTO organizations (name, slug, owner_wallet) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind("test-org")
    .bind(format!("slug-{}", Uuid::new_v4().simple()))
    .bind(&owner_wallet)
    .fetch_one(&pool)
    .await
    .expect("create org");

    let team_id: Uuid =
        sqlx::query_scalar("INSERT INTO teams (org_id, name) VALUES ($1, $2) RETURNING id")
            .bind(org_id)
            .bind(format!("team-{}", Uuid::new_v4().simple()))
            .fetch_one(&pool)
            .await
            .expect("create team");

    sqlx::query("INSERT INTO team_wallets (team_id, wallet_address) VALUES ($1, $2)")
        .bind(team_id)
        .bind(&wallet)
        .execute(&pool)
        .await
        .expect("assign wallet");

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));
    tracker.log_spend(SpendLogEntry {
        wallet_address: wallet.clone(),
        model: "openai/gpt-4o".to_string(),
        provider: "openai".to_string(),
        input_tokens: 1,
        output_tokens: 1,
        cost_usdc: 0.0025,
        tx_signature: None,
        request_id: None,
        session_id: None,
        tenant: None,
        estimated_cost_usdc: None,
    });

    let now = Utc::now();
    let team_day_key = format!("team_spend:{}:{}", team_id, now.format("%Y-%m-%d"));

    let val = wait_for_key(&client, &team_day_key)
        .await
        .expect("team_spend key must appear");
    assert_eq!(val.parse::<f64>().unwrap_or(0.0), 0.0025);

    redis_del(
        &client,
        &[
            &team_day_key,
            &format!("team_spend:{}:{}", team_id, now.format("%Y-%m-%dT%H")),
            &format!("team_spend:{}:{}", team_id, now.format("%Y-%m")),
            &format!("spend:{}:{}", wallet, now.format("%Y-%m-%d")),
            &format!("spend:{}:{}", wallet, now.format("%Y-%m-%dT%H")),
            &format!("spend:{}:{}", wallet, now.format("%Y-%m")),
            &format!("team_member:{wallet}"),
            &format!("team_budget:{team_id}"),
        ],
    )
    .await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn check_budget_enforces_team_daily_limit(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();

    let owner_wallet = format!("test_owner_{}", Uuid::new_v4().simple());
    let org_id: Uuid = sqlx::query_scalar(
        "INSERT INTO organizations (name, slug, owner_wallet) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind("test-org")
    .bind(format!("slug-{}", Uuid::new_v4().simple()))
    .bind(&owner_wallet)
    .fetch_one(&pool)
    .await
    .expect("org");

    let team_id: Uuid =
        sqlx::query_scalar("INSERT INTO teams (org_id, name) VALUES ($1, $2) RETURNING id")
            .bind(org_id)
            .bind(format!("team-{}", Uuid::new_v4().simple()))
            .fetch_one(&pool)
            .await
            .expect("team");

    sqlx::query("INSERT INTO team_wallets (team_id, wallet_address) VALUES ($1, $2)")
        .bind(team_id)
        .bind(&wallet)
        .execute(&pool)
        .await
        .expect("assign");

    sqlx::query("INSERT INTO team_budgets (team_id, daily_limit_usdc) VALUES ($1, 0.10)")
        .bind(team_id)
        .execute(&pool)
        .await
        .expect("team_budget");

    let now = Utc::now();
    let team_day_key = format!("team_spend:{}:{}", team_id, now.format("%Y-%m-%d"));
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("conn");
    let _: () = redis::cmd("SET")
        .arg(&team_day_key)
        .arg("0.09")
        .arg("EX")
        .arg(3600)
        .query_async(&mut conn)
        .await
        .expect("seed");

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));

    // Wallet's per-wallet cap is the default $100/day; team cap is $0.10.
    // $0.05 pushes team total to $0.14 — exceeds team cap.
    let err = tracker
        .check_budget(&wallet, 0.05, None)
        .await
        .expect_err("team daily cap must enforce");
    match err {
        UsageError::BudgetExceeded { limit, .. } => {
            assert!(
                (limit - 0.10).abs() < 1e-9,
                "limit must be team 0.10, got {limit}"
            );
        }
        other => panic!("expected BudgetExceeded, got {other:?}"),
    }

    redis_del(
        &client,
        &[
            &team_day_key,
            &format!("budget_config:{wallet}"),
            &format!("team_member:{wallet}"),
            &format!("team_budget:{team_id}"),
        ],
    )
    .await;
}

#[sqlx::test(migrations = "../../migrations")]
async fn check_budget_redis_get_failure_fails_closed(pool: PgPool) {
    // Garbage in the spend key forces the GET-as-f64 decode to fail.
    // The check_budget code path treats any GET error as a Redis error
    // and must deny the request (fail-closed).
    let client = redis_client();
    let wallet = unique_wallet();
    let now = Utc::now();

    let day_key = format!("spend:{}:{}", wallet, now.format("%Y-%m-%d"));
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("conn");
    let _: () = redis::cmd("SET")
        .arg(&day_key)
        .arg("not-a-number")
        .arg("EX")
        .arg(60)
        .query_async(&mut conn)
        .await
        .expect("seed garbage");

    sqlx::query("INSERT INTO wallet_budgets (wallet_address, daily_limit_usdc) VALUES ($1, 1.00)")
        .bind(&wallet)
        .execute(&pool)
        .await
        .expect("seed budget");

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));
    let err = tracker
        .check_budget(&wallet, 0.10, None)
        .await
        .expect_err("garbage in Redis must fail closed");
    match err {
        UsageError::Redis(_) => {}
        other => panic!("expected Redis error, got {other:?}"),
    }

    redis_del(
        &client,
        &[
            &day_key,
            &format!("budget_config:{wallet}"),
            &format!("team_member:{wallet}"),
        ],
    )
    .await;
}

/// Gap 2: `get_summary` must populate `daily_cost_usdc` / `monthly_cost_usdc`
/// from the same `spend:{wallet}:{period}` Redis counters the budget path
/// writes, parsed the same way (decimal USDC). Seed both windows, then assert
/// the summary reflects them.
#[sqlx::test(migrations = "../../migrations")]
async fn get_summary_populates_daily_and_monthly_from_redis(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();
    let now = Utc::now();

    let day_key = format!("spend:{}:{}", wallet, now.format("%Y-%m-%d"));
    let month_key = format!("spend:{}:{}", wallet, now.format("%Y-%m"));

    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("conn");
    let _: () = redis::cmd("SET")
        .arg(&day_key)
        .arg("0.25")
        .arg("EX")
        .arg(3600)
        .query_async(&mut conn)
        .await
        .expect("seed daily");
    let _: () = redis::cmd("SET")
        .arg(&month_key)
        .arg("1.75")
        .arg("EX")
        .arg(3600)
        .query_async(&mut conn)
        .await
        .expect("seed monthly");

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));
    let summary = tracker
        .get_summary(&wallet)
        .await
        .expect("summary with DB + Redis must succeed");

    assert!(
        (summary.daily_cost_usdc - 0.25).abs() < 1e-9,
        "daily must reflect the seeded counter, got {}",
        summary.daily_cost_usdc
    );
    assert!(
        (summary.monthly_cost_usdc - 1.75).abs() < 1e-9,
        "monthly must reflect the seeded counter, got {}",
        summary.monthly_cost_usdc
    );

    redis_del(&client, &[&day_key, &month_key]).await;
}

/// Gap 2: a missing Redis counter (no spend yet this window) must fall back to
/// 0.0 gracefully, never an error — matching the budget endpoint's
/// `.unwrap_or(0.0)`.
#[sqlx::test(migrations = "../../migrations")]
async fn get_summary_falls_back_to_zero_when_counters_missing(pool: PgPool) {
    let client = redis_client();
    // Fresh per-test wallet => no spend keys exist for it.
    let wallet = unique_wallet();

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));
    let summary = tracker
        .get_summary(&wallet)
        .await
        .expect("summary must succeed even with no Redis counters");

    assert_eq!(
        summary.daily_cost_usdc, 0.0,
        "missing daily counter must fall back to 0.0"
    );
    assert_eq!(
        summary.monthly_cost_usdc, 0.0,
        "missing monthly counter must fall back to 0.0"
    );
}

/// Gap 2: with a DB but NO Redis configured, daily/monthly must be 0.0 (not an
/// error) — Redis is optional (Architectural Rule #12).
#[sqlx::test(migrations = "../../migrations")]
async fn get_summary_zero_windows_when_redis_absent(pool: PgPool) {
    let wallet = unique_wallet();

    // DB present, Redis absent.
    let tracker = UsageTracker::new(Some(pool.clone()), None);
    let summary = tracker
        .get_summary(&wallet)
        .await
        .expect("summary must succeed with DB only");

    assert_eq!(summary.daily_cost_usdc, 0.0);
    assert_eq!(summary.monthly_cost_usdc, 0.0);
}

#[tokio::test]
async fn get_redis_spend_returns_zero_for_missing_key() {
    let client = redis_client();
    let key = format!("nonexistent:{}", Uuid::new_v4().simple());

    let val = get_redis_spend(&client, &key)
        .await
        .expect("missing key must return Ok(0.0)");
    assert_eq!(val, 0.0);
}

#[tokio::test]
async fn get_redis_spend_returns_value_for_present_key() {
    let client = redis_client();
    let key = format!("test:get_redis_spend:{}", Uuid::new_v4().simple());

    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("conn");
    let _: () = redis::cmd("SET")
        .arg(&key)
        .arg("0.42")
        .arg("EX")
        .arg(60)
        .query_async(&mut conn)
        .await
        .expect("set");

    let val = get_redis_spend(&client, &key)
        .await
        .expect("present key must return its value");
    assert!((val - 0.42).abs() < 1e-9);

    redis_del(&client, &[&key]).await;
}

/// Regression for the H1 TOCTOU finding: under concurrent `check_budget`
/// calls for the same wallet, the total committed spend must NEVER exceed
/// the limit. Pre-fix, two concurrent calls each read the same `spend`
/// value before either incremented, both passed the check, and total
/// overshoot was `N × estimated_cost`.
///
/// This test fires N concurrent `check_budget` calls each requesting
/// `estimated = limit / 2`. With proper serialization, exactly two can
/// succeed (covering the limit); the rest must fail. Without it, all N
/// would succeed.
#[sqlx::test(migrations = "../../migrations")]
async fn check_budget_serializes_concurrent_callers(pool: PgPool) {
    use std::sync::Arc;

    let client = redis_client();
    let wallet = unique_wallet();

    sqlx::query("INSERT INTO wallet_budgets (wallet_address, daily_limit_usdc) VALUES ($1, 1.00)")
        .bind(&wallet)
        .execute(&pool)
        .await
        .expect("seed wallet_budget");

    let tracker = Arc::new(UsageTracker::new(Some(pool.clone()), Some(client.clone())));

    // Fire 10 concurrent calls each asking for $0.50. With a $1.00 daily
    // limit, exactly 2 can fit; the other 8 must be rejected.
    const N: usize = 10;
    const PER_CALL: f64 = 0.50;
    let mut handles = Vec::with_capacity(N);
    for _ in 0..N {
        let tracker = Arc::clone(&tracker);
        let wallet = wallet.clone();
        handles.push(tokio::spawn(async move {
            tracker.check_budget(&wallet, PER_CALL, None).await
        }));
    }

    let mut ok = 0usize;
    let mut exceeded = 0usize;
    for h in handles {
        match h.await.expect("join") {
            // M3: `check_budget` now returns a `BudgetReservation` on success.
            Ok(_) => ok += 1,
            Err(UsageError::BudgetExceeded { .. }) => exceeded += 1,
            Err(other) => panic!("unexpected error: {other:?}"),
        }
    }

    assert_eq!(
        ok, 2,
        "exactly 2 of {N} concurrent ${PER_CALL} calls must fit a $1.00 limit; got {ok}"
    );
    assert_eq!(
        exceeded,
        N - 2,
        "the remaining concurrent calls must all be rejected; got {exceeded}"
    );

    // Cleanup
    let now = chrono::Utc::now();
    let day_key = format!("spend:{}:{}", wallet, now.format("%Y-%m-%d"));
    let hour_key = format!("spend:{}:{}", wallet, now.format("%Y-%m-%dT%H"));
    let month_key = format!("spend:{}:{}", wallet, now.format("%Y-%m"));
    redis_del(
        &client,
        &[
            &day_key,
            &hour_key,
            &month_key,
            &format!("budget_config:{wallet}"),
            &format!("team_member:{wallet}"),
        ],
    )
    .await;
}

// ---------------------------------------------------------------------------
// PR2: per-tenant budget enforcement
// ---------------------------------------------------------------------------
//
// SECURITY: the x-tenant tag is forgeable; these budgets are cooperative
// accounting under one trusted single-wallet proxy, NOT isolation (see the
// module-level note in usage.rs).

/// Cleanup helper for the per-tenant counter + cache keys of a (wallet, tenant).
async fn redis_del_tenant(client: &redis::Client, wallet: &str, tenant: &str) {
    let now = Utc::now();
    redis_del(
        client,
        &[
            &format!("spend:{}:{}:{}", wallet, tenant, now.format("%Y-%m-%dT%H")),
            &format!("spend:{}:{}:{}", wallet, tenant, now.format("%Y-%m-%d")),
            &format!("spend:{}:{}:{}", wallet, tenant, now.format("%Y-%m")),
            &format!("budget_config:{wallet}"),
            &format!("team_member:{wallet}"),
            &format!("tenant_require:{wallet}"),
            &format!("tenant_budget:{wallet}:{tenant}"),
            &format!("spend:{}:{}", wallet, now.format("%Y-%m-%d")),
            &format!("spend:{}:{}", wallet, now.format("%Y-%m-%dT%H")),
            &format!("spend:{}:{}", wallet, now.format("%Y-%m")),
        ],
    )
    .await;
}

/// A provisioned `(wallet, tenant)` daily budget is enforced when its tag is
/// present, even though require_tenant is FALSE (default). Under-limit passes;
/// the next request that would breach the tenant cap is rejected.
#[sqlx::test(migrations = "../../migrations")]
async fn check_budget_enforces_provisioned_tenant_daily_limit(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();
    let tenant = "acme";
    let now = Utc::now();

    // require_tenant defaults FALSE; only a tenant budget row is provisioned.
    sqlx::query(
        "INSERT INTO wallet_budgets (wallet_address, daily_limit_usdc) VALUES ($1, 100.00)",
    )
    .bind(&wallet)
    .execute(&pool)
    .await
    .expect("seed wallet budget");
    sqlx::query(
        "INSERT INTO tenant_budgets (wallet_address, tenant, daily_limit_usdc) VALUES ($1, $2, 1.00)",
    )
    .bind(&wallet)
    .bind(tenant)
    .execute(&pool)
    .await
    .expect("seed tenant budget");

    // Pre-seed the tenant daily counter near its $1.00 cap.
    let tenant_day_key = format!("spend:{}:{}:{}", wallet, tenant, now.format("%Y-%m-%d"));
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("conn");
    let _: () = redis::cmd("SET")
        .arg(&tenant_day_key)
        .arg("0.95")
        .arg("EX")
        .arg(3600)
        .query_async(&mut conn)
        .await
        .expect("seed tenant spend");

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));

    // $0.04 fits (0.95 + 0.04 = 0.99 ≤ 1.00).
    tracker
        .check_budget(&wallet, 0.04, Some(tenant))
        .await
        .expect("$0.04 must fit under the tenant daily cap");

    // $0.10 now breaches (0.99 + 0.10 = 1.09 > 1.00).
    let err = tracker
        .check_budget(&wallet, 0.10, Some(tenant))
        .await
        .expect_err("must reject over the tenant daily cap");
    match err {
        UsageError::BudgetExceeded { limit, .. } => {
            assert!(
                (limit - 1.0).abs() < 1e-9,
                "limit must be tenant 1.0, got {limit}"
            );
        }
        other => panic!("expected BudgetExceeded, got {other:?}"),
    }

    redis_del_tenant(&client, &wallet, tenant).await;
}

/// require_tenant=TRUE + untagged request → fail-closed (TenantRequired),
/// and BEFORE any settlement (check_budget runs pre-settlement). Also asserts
/// no wallet counter was leaked by the rejected request.
#[sqlx::test(migrations = "../../migrations")]
async fn check_budget_rejects_untagged_when_require_tenant(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();
    let now = Utc::now();

    sqlx::query(
        "INSERT INTO wallet_budgets (wallet_address, daily_limit_usdc, require_tenant) \
         VALUES ($1, 100.00, TRUE)",
    )
    .bind(&wallet)
    .execute(&pool)
    .await
    .expect("seed enforced wallet");

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));

    let err = tracker
        .check_budget(&wallet, 0.10, None)
        .await
        .expect_err("enforced wallet must reject untagged request");
    match err {
        UsageError::TenantRequired { wallet: w } => assert_eq!(w, wallet),
        other => panic!("expected TenantRequired, got {other:?}"),
    }

    // No budget leak: the wallet daily counter must not have been left committed.
    let day_key = format!("spend:{}:{}", wallet, now.format("%Y-%m-%d"));
    let leaked = get_redis_spend(&client, &day_key).await.expect("get");
    assert_eq!(leaked, 0.0, "rejected request must leak no wallet spend");

    redis_del(
        &client,
        &[
            &day_key,
            &format!("budget_config:{wallet}"),
            &format!("team_member:{wallet}"),
            &format!("tenant_require:{wallet}"),
        ],
    )
    .await;
}

/// require_tenant=TRUE + tag with no provisioned row → fail-closed
/// (TenantNotProvisioned), before settlement.
#[sqlx::test(migrations = "../../migrations")]
async fn check_budget_rejects_unknown_tenant_when_require_tenant(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();
    let now = Utc::now();

    sqlx::query(
        "INSERT INTO wallet_budgets (wallet_address, daily_limit_usdc, require_tenant) \
         VALUES ($1, 100.00, TRUE)",
    )
    .bind(&wallet)
    .execute(&pool)
    .await
    .expect("seed enforced wallet");

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));

    let err = tracker
        .check_budget(&wallet, 0.10, Some("ghost"))
        .await
        .expect_err("enforced wallet must reject unprovisioned tenant");
    match err {
        UsageError::TenantNotProvisioned { wallet: w, tenant } => {
            assert_eq!(w, wallet);
            assert_eq!(tenant, "ghost");
        }
        other => panic!("expected TenantNotProvisioned, got {other:?}"),
    }

    let day_key = format!("spend:{}:{}", wallet, now.format("%Y-%m-%d"));
    let leaked = get_redis_spend(&client, &day_key).await.expect("get");
    assert_eq!(leaked, 0.0, "rejected request must leak no wallet spend");

    redis_del_tenant(&client, &wallet, "ghost").await;
}

/// require_tenant=TRUE + tag WITH a provisioned row → allowed (and the tenant
/// bucket is enforced). Pins the happy path of the enforced wallet.
#[sqlx::test(migrations = "../../migrations")]
async fn check_budget_allows_provisioned_tenant_when_require_tenant(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();
    let tenant = "acme";

    sqlx::query(
        "INSERT INTO wallet_budgets (wallet_address, daily_limit_usdc, require_tenant) \
         VALUES ($1, 100.00, TRUE)",
    )
    .bind(&wallet)
    .execute(&pool)
    .await
    .expect("seed enforced wallet");
    sqlx::query(
        "INSERT INTO tenant_budgets (wallet_address, tenant, daily_limit_usdc) VALUES ($1, $2, 1.00)",
    )
    .bind(&wallet)
    .bind(tenant)
    .execute(&pool)
    .await
    .expect("seed tenant budget");

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));
    tracker
        .check_budget(&wallet, 0.10, Some(tenant))
        .await
        .expect("enforced wallet with provisioned tenant must be allowed");

    // The tenant daily counter must now reflect the reservation.
    let now = Utc::now();
    let tenant_day_key = format!("spend:{}:{}:{}", wallet, tenant, now.format("%Y-%m-%d"));
    let reserved = get_redis_spend(&client, &tenant_day_key)
        .await
        .expect("get");
    assert!(
        (reserved - 0.10).abs() < 1e-9,
        "tenant daily counter must hold the reservation, got {reserved}"
    );

    redis_del_tenant(&client, &wallet, tenant).await;
}

/// Backward-compat: an unenforced wallet (require_tenant=FALSE, no tenant_budgets
/// row) behaves identically with and without a tenant tag — the wallet daily cap
/// is the only thing enforced, and a tag adds no rejection. Proven against the
/// Redis counters.
#[sqlx::test(migrations = "../../migrations")]
async fn check_budget_backward_compat_unenforced_wallet_ignores_tag(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();
    let tenant = "acme";

    // Default wallet (no row → default $100/day, require_tenant=FALSE).
    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));

    // Untagged passes.
    tracker
        .check_budget(&wallet, 0.10, None)
        .await
        .expect("untagged must pass on unenforced wallet");
    // Tagged (no provisioned row) must ALSO pass — no tenant enforcement applies.
    tracker
        .check_budget(&wallet, 0.10, Some(tenant))
        .await
        .expect("tagged-but-unprovisioned must pass identically on unenforced wallet");

    // No tenant counter should have been committed by check_budget (Skip path):
    // tenant enforcement is skipped, so no spend:{wallet}:{tenant}:{period} key.
    let now = Utc::now();
    let tenant_day_key = format!("spend:{}:{}:{}", wallet, tenant, now.format("%Y-%m-%d"));
    let tenant_spend = get_redis_spend(&client, &tenant_day_key)
        .await
        .expect("get");
    assert_eq!(
        tenant_spend, 0.0,
        "Skip path must not commit a tenant counter at check_budget time"
    );

    redis_del_tenant(&client, &wallet, tenant).await;
}

/// Counter reconciliation: with a provisioned tenant budget, `check_budget`
/// reserves the ESTIMATE on the tenant daily counter and `log_spend` settles it
/// to the ACTUAL via the (cost - estimated) delta on the SAME
/// `spend:{wallet}:{tenant}:{period}` key. Net result equals the actual cost.
#[sqlx::test(migrations = "../../migrations")]
async fn tenant_counter_reconciles_estimate_to_actual(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();
    let tenant = "acme";

    sqlx::query(
        "INSERT INTO wallet_budgets (wallet_address, daily_limit_usdc) VALUES ($1, 100.00)",
    )
    .bind(&wallet)
    .execute(&pool)
    .await
    .expect("seed wallet");
    sqlx::query(
        "INSERT INTO tenant_budgets (wallet_address, tenant, daily_limit_usdc) VALUES ($1, $2, 10.00)",
    )
    .bind(&wallet)
    .bind(tenant)
    .execute(&pool)
    .await
    .expect("seed tenant budget");

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));

    // Reserve an estimate of $0.0050 on the tenant bucket.
    let estimated = 0.0050;
    tracker
        .check_budget(&wallet, estimated, Some(tenant))
        .await
        .expect("reserve estimate");

    let now = Utc::now();
    let tenant_day_key = format!("spend:{}:{}:{}", wallet, tenant, now.format("%Y-%m-%d"));
    let after_reserve = get_redis_spend(&client, &tenant_day_key)
        .await
        .expect("get");
    assert!(
        (after_reserve - estimated).abs() < 1e-9,
        "after reserve, tenant counter must equal estimate {estimated}, got {after_reserve}"
    );

    // Actual came in HIGHER than estimate; log_spend settles the delta.
    let actual = 0.0075;
    tracker.log_spend(SpendLogEntry {
        wallet_address: wallet.clone(),
        model: "openai/gpt-4o".to_string(),
        provider: "openai".to_string(),
        input_tokens: 10,
        output_tokens: 20,
        cost_usdc: actual,
        tx_signature: None,
        request_id: None,
        session_id: None,
        tenant: Some(tenant.to_string()),
        estimated_cost_usdc: Some(estimated),
    });

    // Poll until the tenant counter settles to the actual.
    let mut settled = after_reserve;
    for _ in 0..100 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        if let Ok(v) = get_redis_spend(&client, &tenant_day_key).await {
            settled = v;
            if (settled - actual).abs() < 1e-9 {
                break;
            }
        }
    }
    assert!(
        (settled - actual).abs() < 1e-9,
        "tenant counter must reconcile estimate→actual to {actual}, got {settled}"
    );

    redis_del_tenant(&client, &wallet, tenant).await;
}

/// log_spend writes the per-tenant hourly/daily/monthly counters using the
/// `spend:{wallet}:{tenant}:{period}` key format when the entry carries a tag
/// and no estimate was reserved (None → increment full cost).
#[sqlx::test(migrations = "../../migrations")]
async fn log_spend_writes_tenant_counters_with_correct_key(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();
    let tenant = "acme";
    let now = Utc::now();

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));
    tracker.log_spend(SpendLogEntry {
        wallet_address: wallet.clone(),
        model: "openai/gpt-4o".to_string(),
        provider: "openai".to_string(),
        input_tokens: 1,
        output_tokens: 1,
        cost_usdc: 0.0050,
        tx_signature: None,
        request_id: None,
        session_id: None,
        tenant: Some(tenant.to_string()),
        estimated_cost_usdc: None,
    });

    let tenant_day_key = format!("spend:{}:{}:{}", wallet, tenant, now.format("%Y-%m-%d"));
    let val = wait_for_key(&client, &tenant_day_key)
        .await
        .expect("tenant daily counter must appear at spend:{wallet}:{tenant}:{day}");
    assert_eq!(val.parse::<f64>().unwrap_or(0.0), 0.005);

    redis_del_tenant(&client, &wallet, tenant).await;
}
