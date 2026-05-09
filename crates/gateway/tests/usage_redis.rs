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
        .check_budget(&wallet, 0.50)
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
        .check_budget(&wallet, 0.04)
        .await
        .expect("$0.04 must fit");

    let err = tracker
        .check_budget(&wallet, 0.10)
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
            assert!((spent - 1.05).abs() < 1e-9);
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
        .check_budget(&wallet, 0.10)
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
        .check_budget(&wallet, 0.10)
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
        .check_budget(&wallet, 0.10)
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
        .check_budget(&wallet, 0.04)
        .await
        .expect("$0.04 fits under cached cap");

    let err = tracker
        .check_budget(&wallet, 0.06)
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
        .check_budget(&wallet, 0.50)
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
        .check_budget(&wallet, 0.05)
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
        .check_budget(&wallet, 0.10)
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
