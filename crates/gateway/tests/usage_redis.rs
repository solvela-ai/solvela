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
        tenant_enforced: false,
        estimated_cost_usdc: None,
        reserved: Default::default(),
        vendor: None,
        routing_tier: None,
        routing_score: None,
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
        tenant_enforced: false,
        estimated_cost_usdc: None,
        reserved: Default::default(),
        vendor: None,
        routing_tier: None,
        routing_score: None,
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

/// FINDING 3: a PURE zero-cost row (`cost_usdc == 0.0` AND `estimated_cost_usdc`
/// is None — the free-tier $0 entry) must write the DB row for observability but
/// issue NO Redis spend-counter increment (the increment would be `INCRBYFLOAT 0.0`,
/// a no-op round-trip). Proven by asserting the `spend:{wallet}:{day}` key NEVER
/// materializes after a settling window, while a nonzero entry for the SAME wallet
/// DOES create it.
#[sqlx::test(migrations = "../../migrations")]
async fn log_spend_zero_cost_no_reservation_skips_redis_but_writes_db(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();
    let now = Utc::now();
    let day_key = format!("spend:{}:{}", wallet, now.format("%Y-%m-%d"));

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));

    // (a) Pure zero-cost free-tier entry: $0, no reservation.
    tracker.log_spend(SpendLogEntry {
        wallet_address: wallet.clone(),
        model: "google/gemini-3.1-flash-lite".to_string(),
        provider: "google".to_string(),
        input_tokens: 5,
        output_tokens: 7,
        cost_usdc: 0.0,
        tx_signature: None,
        request_id: None,
        session_id: None,
        tenant: None,
        tenant_enforced: false,
        estimated_cost_usdc: None,
        reserved: Default::default(),
        vendor: None,
        routing_tier: None,
        routing_score: None,
    });

    // The DB row must be written (observability). Poll for it.
    let mut db_rows: i64 = 0;
    for _ in 0..50 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        db_rows = sqlx::query_scalar("SELECT COUNT(*) FROM spend_logs WHERE wallet_address = $1")
            .bind(&wallet)
            .fetch_one(&pool)
            .await
            .expect("count spend_logs");
        if db_rows >= 1 {
            break;
        }
    }
    assert_eq!(
        db_rows, 1,
        "a zero-cost free-tier entry must still write its DB row for observability"
    );

    // The Redis spend key must NOT exist — no INCRBYFLOAT was issued. Give the
    // fire-and-forget spawn ample time to (not) write.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("conn");
    let zero_cost_val: Option<String> = redis::cmd("GET")
        .arg(&day_key)
        .query_async(&mut conn)
        .await
        .expect("get day key");
    assert!(
        zero_cost_val.is_none(),
        "a pure $0/no-reservation entry must NOT create a Redis spend counter, got {zero_cost_val:?}"
    );

    // (b) A NONZERO entry for the same wallet still increments Redis.
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
        tenant_enforced: false,
        estimated_cost_usdc: None,
        reserved: Default::default(),
        vendor: None,
        routing_tier: None,
        routing_score: None,
    });
    let day_val = wait_for_key(&client, &day_key)
        .await
        .expect("a nonzero entry must still create the Redis spend counter");
    assert!(
        (day_val.parse::<f64>().unwrap_or(0.0) - 0.005).abs() < 1e-9,
        "nonzero spend counter must equal 0.005, got {day_val}"
    );

    redis_del(
        &client,
        &[
            &day_key,
            &format!("spend:{}:{}", wallet, now.format("%Y-%m-%dT%H")),
            &format!("spend:{}:{}", wallet, now.format("%Y-%m")),
        ],
    )
    .await;
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
        tenant_enforced: false,
        estimated_cost_usdc: None,
        reserved: Default::default(),
        vendor: None,
        routing_tier: None,
        routing_score: None,
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

    // Seed BOTH hourly and daily limits so both wallet windows are reserved
    // before the tenant gate rejects — proves rollback releases ALL windows.
    sqlx::query(
        "INSERT INTO wallet_budgets (wallet_address, hourly_limit_usdc, daily_limit_usdc, require_tenant) \
         VALUES ($1, 50.00, 100.00, TRUE)",
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

    // No budget leak: NEITHER the wallet daily NOR the wallet hourly counter
    // must have been left committed (rollback completeness — all reserved
    // windows are released on a fail-closed rejection, not just the daily one).
    let day_key = format!("spend:{}:{}", wallet, now.format("%Y-%m-%d"));
    let hour_key = format!("spend:{}:{}", wallet, now.format("%Y-%m-%dT%H"));
    let leaked_day = get_redis_spend(&client, &day_key).await.expect("get day");
    let leaked_hour = get_redis_spend(&client, &hour_key).await.expect("get hour");
    assert_eq!(
        leaked_day, 0.0,
        "rejected request must leak no wallet daily spend"
    );
    assert_eq!(
        leaked_hour, 0.0,
        "rejected request must leak no wallet hourly spend"
    );

    redis_del(
        &client,
        &[
            &day_key,
            &hour_key,
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
    let reservation = tracker
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
        // A provisioned tenant bucket was enforced by check_budget above, so the
        // handler would thread tenant_enforced=true → reconcile per-tenant
        // counters.
        tenant_enforced: reservation.tenant_enforced(),
        estimated_cost_usdc: Some(estimated),
        // Per-window reserved flags from the SAME reservation the handler would
        // thread, so each reserved window nets to actual (delta) and each
        // unreserved window nets to actual (full cost) — no double count.
        reserved: reservation.reserved_windows(),
        vendor: None,
        routing_tier: None,
        routing_score: None,
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
        // Tenant enforcement was active for this request → reconcile counters.
        tenant_enforced: true,
        estimated_cost_usdc: None,
        // No estimate reserved (None branch) → reserved flags are inert here.
        reserved: Default::default(),
        vendor: None,
        routing_tier: None,
        routing_score: None,
    });

    let tenant_day_key = format!("spend:{}:{}:{}", wallet, tenant, now.format("%Y-%m-%d"));
    let val = wait_for_key(&client, &tenant_day_key)
        .await
        .expect("tenant daily counter must appear at spend:{wallet}:{tenant}:{day}");
    assert_eq!(val.parse::<f64>().unwrap_or(0.0), 0.005);

    redis_del_tenant(&client, &wallet, tenant).await;
}

/// A provisioned tenant HOURLY budget is enforced on the
/// `spend:{wallet}:{tenant}:{%Y-%m-%dT%H}` key. A wrong `%H` key derivation
/// would let this slip through undetected (only daily was covered before).
#[sqlx::test(migrations = "../../migrations")]
async fn check_budget_enforces_provisioned_tenant_hourly_limit(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();
    let tenant = "acme";
    let now = Utc::now();

    sqlx::query(
        "INSERT INTO wallet_budgets (wallet_address, daily_limit_usdc) VALUES ($1, 100.00)",
    )
    .bind(&wallet)
    .execute(&pool)
    .await
    .expect("seed wallet budget");
    sqlx::query(
        "INSERT INTO tenant_budgets (wallet_address, tenant, hourly_limit_usdc) VALUES ($1, $2, 1.00)",
    )
    .bind(&wallet)
    .bind(tenant)
    .execute(&pool)
    .await
    .expect("seed tenant hourly budget");

    // Pre-seed the tenant HOURLY counter near its $1.00 cap.
    let tenant_hour_key = format!("spend:{}:{}:{}", wallet, tenant, now.format("%Y-%m-%dT%H"));
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("conn");
    let _: () = redis::cmd("SET")
        .arg(&tenant_hour_key)
        .arg("0.95")
        .arg("EX")
        .arg(3600)
        .query_async(&mut conn)
        .await
        .expect("seed tenant hourly spend");

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));

    // $0.04 fits (0.95 + 0.04 = 0.99 ≤ 1.00).
    tracker
        .check_budget(&wallet, 0.04, Some(tenant))
        .await
        .expect("$0.04 must fit under the tenant hourly cap");

    // $0.10 now breaches the HOURLY cap (0.99 + 0.10 = 1.09 > 1.00).
    let err = tracker
        .check_budget(&wallet, 0.10, Some(tenant))
        .await
        .expect_err("must reject over the tenant hourly cap");
    match err {
        UsageError::BudgetExceeded { limit, .. } => {
            assert!(
                (limit - 1.0).abs() < 1e-9,
                "limit must be tenant hourly 1.0, got {limit}"
            );
        }
        other => panic!("expected BudgetExceeded, got {other:?}"),
    }

    redis_del_tenant(&client, &wallet, tenant).await;
}

/// A provisioned tenant MONTHLY budget is enforced on the
/// `spend:{wallet}:{tenant}:{%Y-%m}` key. Guards a wrong `%Y-%m` derivation.
#[sqlx::test(migrations = "../../migrations")]
async fn check_budget_enforces_provisioned_tenant_monthly_limit(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();
    let tenant = "acme";
    let now = Utc::now();

    sqlx::query(
        "INSERT INTO wallet_budgets (wallet_address, daily_limit_usdc) VALUES ($1, 100.00)",
    )
    .bind(&wallet)
    .execute(&pool)
    .await
    .expect("seed wallet budget");
    sqlx::query(
        "INSERT INTO tenant_budgets (wallet_address, tenant, monthly_limit_usdc) VALUES ($1, $2, 1.00)",
    )
    .bind(&wallet)
    .bind(tenant)
    .execute(&pool)
    .await
    .expect("seed tenant monthly budget");

    // Pre-seed the tenant MONTHLY counter near its $1.00 cap.
    let tenant_month_key = format!("spend:{}:{}:{}", wallet, tenant, now.format("%Y-%m"));
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("conn");
    let _: () = redis::cmd("SET")
        .arg(&tenant_month_key)
        .arg("0.95")
        .arg("EX")
        .arg(3600)
        .query_async(&mut conn)
        .await
        .expect("seed tenant monthly spend");

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));

    tracker
        .check_budget(&wallet, 0.04, Some(tenant))
        .await
        .expect("$0.04 must fit under the tenant monthly cap");

    let err = tracker
        .check_budget(&wallet, 0.10, Some(tenant))
        .await
        .expect_err("must reject over the tenant monthly cap");
    match err {
        UsageError::BudgetExceeded { limit, .. } => {
            assert!(
                (limit - 1.0).abs() < 1e-9,
                "limit must be tenant monthly 1.0, got {limit}"
            );
        }
        other => panic!("expected BudgetExceeded, got {other:?}"),
    }

    redis_del_tenant(&client, &wallet, tenant).await;
}

/// Reconciliation across ALL THREE tenant windows (hour + day + month), not
/// just day. `check_budget` reserves the estimate on each provisioned window and
/// `log_spend` settles each to the actual via the (cost − estimate) delta on the
/// SAME `spend:{wallet}:{tenant}:{period}` key.
#[sqlx::test(migrations = "../../migrations")]
async fn tenant_counters_reconcile_all_three_windows(pool: PgPool) {
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
    // All three tenant windows provisioned and generous so nothing rejects.
    sqlx::query(
        "INSERT INTO tenant_budgets (wallet_address, tenant, hourly_limit_usdc, daily_limit_usdc, monthly_limit_usdc) \
         VALUES ($1, $2, 10.00, 10.00, 10.00)",
    )
    .bind(&wallet)
    .bind(tenant)
    .execute(&pool)
    .await
    .expect("seed tenant budget");

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));

    let estimated = 0.0050;
    let reservation = tracker
        .check_budget(&wallet, estimated, Some(tenant))
        .await
        .expect("reserve estimate");

    let now = Utc::now();
    let hour_key = format!("spend:{}:{}:{}", wallet, tenant, now.format("%Y-%m-%dT%H"));
    let day_key = format!("spend:{}:{}:{}", wallet, tenant, now.format("%Y-%m-%d"));
    let month_key = format!("spend:{}:{}:{}", wallet, tenant, now.format("%Y-%m"));

    for k in [&hour_key, &day_key, &month_key] {
        let v = get_redis_spend(&client, k).await.expect("get");
        assert!(
            (v - estimated).abs() < 1e-9,
            "after reserve, {k} must equal estimate {estimated}, got {v}"
        );
    }

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
        tenant_enforced: reservation.tenant_enforced(),
        estimated_cost_usdc: Some(estimated),
        // Per-window reserved flags from the SAME reservation the handler would
        // thread, so each reserved window nets to actual (delta) and each
        // unreserved window nets to actual (full cost) — no double count.
        reserved: reservation.reserved_windows(),
        vendor: None,
        routing_tier: None,
        routing_score: None,
    });

    // Each of the three windows must settle to the actual.
    for k in [&hour_key, &day_key, &month_key] {
        let mut settled = estimated;
        for _ in 0..100 {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            if let Ok(v) = get_redis_spend(&client, k).await {
                settled = v;
                if (settled - actual).abs() < 1e-9 {
                    break;
                }
            }
        }
        assert!(
            (settled - actual).abs() < 1e-9,
            "{k} must reconcile estimate→actual to {actual}, got {settled}"
        );
    }

    redis_del_tenant(&client, &wallet, tenant).await;
}

/// Core bug-class test: cap-escape blocked END-TO-END. After `log_spend`
/// reconciles the tenant counter to the actual, a SECOND `check_budget` whose
/// amount would breach the tenant cap cumulatively is REJECTED. This proves the
/// counter the handler actually wrote (`log_spend`) is the very one enforcement
/// reads (`check_budget`) — a wrong-key reconcile would let the second request
/// through and fail this test.
#[sqlx::test(migrations = "../../migrations")]
async fn tenant_cap_escape_blocked_end_to_end(pool: PgPool) {
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
        "INSERT INTO tenant_budgets (wallet_address, tenant, daily_limit_usdc) VALUES ($1, $2, 1.00)",
    )
    .bind(&wallet)
    .bind(tenant)
    .execute(&pool)
    .await
    .expect("seed tenant budget");

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));

    // First request: reserve a small estimate, then settle a LARGE actual via
    // log_spend so the tenant counter ends near the cap (0.90 of 1.00).
    let estimated = 0.10;
    let reservation = tracker
        .check_budget(&wallet, estimated, Some(tenant))
        .await
        .expect("first request fits");

    let actual = 0.90;
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
        tenant_enforced: reservation.tenant_enforced(),
        estimated_cost_usdc: Some(estimated),
        // Per-window reserved flags from the SAME reservation the handler would
        // thread, so each reserved window nets to actual (delta) and each
        // unreserved window nets to actual (full cost) — no double count.
        reserved: reservation.reserved_windows(),
        vendor: None,
        routing_tier: None,
        routing_score: None,
    });

    // Wait for the counter to settle to the actual (0.90).
    let now = Utc::now();
    let tenant_day_key = format!("spend:{}:{}:{}", wallet, tenant, now.format("%Y-%m-%d"));
    let mut settled = estimated;
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
        "tenant counter must settle to {actual} before the escape attempt, got {settled}"
    );

    // Second request: 0.90 + 0.20 = 1.10 > 1.00 → MUST be rejected. If the
    // reconcile had written a wrong key, the counter read here would still be
    // ~0.10 (the reservation only) and this would wrongly succeed.
    let err = tracker
        .check_budget(&wallet, 0.20, Some(tenant))
        .await
        .expect_err("cumulative spend must breach the tenant cap and be rejected");
    match err {
        UsageError::BudgetExceeded { limit, .. } => {
            assert!((limit - 1.0).abs() < 1e-9, "limit must be 1.0, got {limit}");
        }
        other => panic!("expected BudgetExceeded, got {other:?}"),
    }

    redis_del_tenant(&client, &wallet, tenant).await;
}

/// Skip-path counter suppression: a tagged request on an UNENFORCED wallet (no
/// provisioned row, require_tenant=FALSE) must NOT accumulate per-tenant Redis
/// counters via `log_spend` (tenant_enforced=false). Pre-PR2-fix this would
/// poison a later-provisioned budget. Also asserts the wallet daily counter
/// still tracks the spend (attribution/enforcement unaffected for the wallet).
#[sqlx::test(migrations = "../../migrations")]
async fn log_spend_skips_tenant_counter_when_not_enforced(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();
    let tenant = "acme";
    let now = Utc::now();

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));
    // tenant_enforced=false mirrors what the handler threads on the Skip path.
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
        tenant_enforced: false,
        estimated_cost_usdc: None,
        reserved: Default::default(),
        vendor: None,
        routing_tier: None,
        routing_score: None,
    });

    // Wallet daily counter must materialize (wallet accounting unaffected).
    let wallet_day_key = format!("spend:{}:{}", wallet, now.format("%Y-%m-%d"));
    wait_for_key(&client, &wallet_day_key)
        .await
        .expect("wallet daily counter must still be written");

    // The per-tenant counter must NOT have been written.
    let tenant_day_key = format!("spend:{}:{}:{}", wallet, tenant, now.format("%Y-%m-%d"));
    let tenant_spend = get_redis_spend(&client, &tenant_day_key)
        .await
        .expect("get");
    assert_eq!(
        tenant_spend, 0.0,
        "unenforced (Skip-path) request must not write a per-tenant counter"
    );

    redis_del(&client, &[&wallet_day_key]).await;
    redis_del_tenant(&client, &wallet, tenant).await;
}

/// Backward-compat: the wallet DAILY counter sum is identical for an unenforced
/// wallet whether or not a tenant tag is supplied. The tag must not alter wallet
/// accounting in any way (it only ever drives the optional tenant bucket).
#[sqlx::test(migrations = "../../migrations")]
async fn wallet_daily_counter_unchanged_by_tag_on_unenforced_wallet(pool: PgPool) {
    let client = redis_client();
    let wallet_untagged = unique_wallet();
    let wallet_tagged = unique_wallet();
    let now = Utc::now();

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));

    let mk = |wallet: &str, tenant: Option<String>| SpendLogEntry {
        wallet_address: wallet.to_string(),
        model: "openai/gpt-4o".to_string(),
        provider: "openai".to_string(),
        input_tokens: 1,
        output_tokens: 1,
        cost_usdc: 0.0050,
        tx_signature: None,
        request_id: None,
        session_id: None,
        tenant,
        // Unenforced wallet → Skip path → tenant_enforced is false either way.
        tenant_enforced: false,
        estimated_cost_usdc: None,
        reserved: Default::default(),
        vendor: None,
        routing_tier: None,
        routing_score: None,
    };
    tracker.log_spend(mk(&wallet_untagged, None));
    tracker.log_spend(mk(&wallet_tagged, Some("acme".to_string())));

    let untagged_key = format!("spend:{}:{}", wallet_untagged, now.format("%Y-%m-%d"));
    let tagged_key = format!("spend:{}:{}", wallet_tagged, now.format("%Y-%m-%d"));
    let u = wait_for_key(&client, &untagged_key)
        .await
        .expect("untagged daily counter")
        .parse::<f64>()
        .unwrap_or(-1.0);
    let t = wait_for_key(&client, &tagged_key)
        .await
        .expect("tagged daily counter")
        .parse::<f64>()
        .unwrap_or(-1.0);
    assert!(
        (u - t).abs() < 1e-9 && (u - 0.005).abs() < 1e-9,
        "wallet daily counter must be identical with/without a tag, got untagged={u} tagged={t}"
    );

    redis_del(&client, &[&untagged_key, &tagged_key]).await;
    redis_del_tenant(&client, &wallet_tagged, "acme").await;
}

/// Fail-OPEN on wallet-config DB error: when the `wallet_budgets` read errors
/// (forced by closing the Postgres pool), an UNTAGGED request on a wallet that
/// WAS configured `require_tenant=TRUE` is NOT rejected by the tenant gate —
/// enforcement degrades to the (restrictive) wallet cap (the authoritative
/// backstop), with `require_tenant` reading `false` from the restrictive
/// fallback. Also asserts the error-derived config is NOT cached (N2
/// cache-poisoning fix): `budget_config:{wallet}` must be absent after the failed
/// read, so the next request re-attempts the DB.
///
/// N2 note: `require_tenant` now rides on `budget_config:{wallet}` (the separate
/// `tenant_require:{wallet}` sentinel was removed). To isolate the wallet-config
/// DB error from the H4 team-membership fail-closed path, the team membership is
/// pre-seeded as a cached "none" so `get_team_for_wallet` is served from Redis
/// (and does not error on the closed pool) — otherwise the team gate would deny
/// first and we could never reach the tenant gate.
#[sqlx::test(migrations = "../../migrations")]
async fn require_tenant_db_error_fails_open_and_does_not_cache(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();

    // Configure the wallet as ENFORCED, then close the pool so the wallet-config
    // read errors at check_budget time.
    sqlx::query(
        "INSERT INTO wallet_budgets (wallet_address, daily_limit_usdc, require_tenant) \
         VALUES ($1, 100.00, TRUE)",
    )
    .bind(&wallet)
    .execute(&pool)
    .await
    .expect("seed enforced wallet");

    // Clear any cached wallet config from prior reads in this wallet's space, and
    // pre-seed team membership as "none" so the team lookup is served from Redis
    // (not the closed pool).
    let config_key = format!("budget_config:{wallet}");
    let team_key = format!("team_member:{wallet}");
    redis_del(&client, &[&config_key]).await;
    {
        let mut conn = client
            .get_multiplexed_async_connection()
            .await
            .expect("conn");
        let _: () = redis::cmd("SET")
            .arg(&team_key)
            .arg("none")
            .arg("EX")
            .arg(60)
            .query_async(&mut conn)
            .await
            .expect("seed team none");
    }

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));
    // Force every subsequent DB query on this pool to error.
    pool.close().await;

    // Untagged request: with a healthy DB this would be RejectRequired. With the
    // wallet-config DB read erroring, require_tenant reads `false` (restrictive
    // fallback) → tenant gate Skips, and the request passes under the restrictive
    // wallet cap ($0.50/hr, $1/day, $10/mo). $0.10 fits.
    tracker
        .check_budget(&wallet, 0.10, None)
        .await
        .expect("DB error on wallet config must fail OPEN for the tenant gate (untagged passes)");

    // N2 cache-poisoning guard: the error-derived restrictive fallback must NOT
    // have been cached — `budget_config:{wallet}` must be absent so the next
    // request re-attempts the DB read rather than serving require_tenant=false
    // for a full TTL.
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("conn");
    let exists: bool = redis::cmd("EXISTS")
        .arg(&config_key)
        .query_async(&mut conn)
        .await
        .expect("EXISTS");
    assert!(
        !exists,
        "a wallet-config DB error must NOT cache an error-derived budget_config (cache-poisoning), but {config_key} exists"
    );

    let now = Utc::now();
    redis_del(
        &client,
        &[
            &config_key,
            &team_key,
            &format!("tenant_require:{wallet}"),
            &format!("spend:{}:{}", wallet, now.format("%Y-%m-%d")),
            &format!("spend:{}:{}", wallet, now.format("%Y-%m-%dT%H")),
        ],
    )
    .await;
}

/// N3: a DB ERROR reading `tenant_budgets` for an ENFORCED wallet (require_tenant
/// = TRUE) is still fail-closed (deny), but surfaced as the transient
/// `UsageError::Database` variant — NOT `TenantNotProvisioned` (which would
/// mislead an operator into chasing a phantom provisioning issue during a DB
/// blip). Confirmed-absent vs. DB-error are distinguished by `TenantLookup`.
///
/// Setup mirrors the fail-open test: pre-seed `budget_config:{wallet}` as cached
/// (require_tenant=true) and `team_member:{wallet}` as "none" so neither the
/// wallet-config nor team reads touch the DB, then close the pool so ONLY the
/// tagged `tenant_budgets` lookup errors.
#[sqlx::test(migrations = "../../migrations")]
async fn tenant_lookup_db_error_for_enforced_wallet_surfaces_database_not_not_provisioned(
    pool: PgPool,
) {
    let client = redis_client();
    let wallet = unique_wallet();
    let tenant = "acme";

    let config_key = format!("budget_config:{wallet}");
    let team_key = format!("team_member:{wallet}");
    let tenant_budget_key = format!("tenant_budget:{wallet}:{tenant}");

    {
        let mut conn = client
            .get_multiplexed_async_connection()
            .await
            .expect("conn");
        // Cache an ENFORCED wallet config (require_tenant=true) so the
        // wallet-config read is served from Redis on the closed pool.
        let cfg = r#"{"hourly":null,"daily":100.0,"monthly":null,"require_tenant":true}"#;
        let _: () = redis::cmd("SET")
            .arg(&config_key)
            .arg(cfg)
            .arg("EX")
            .arg(60)
            .query_async(&mut conn)
            .await
            .expect("seed config");
        // Team membership "none" so the team lookup is served from Redis.
        let _: () = redis::cmd("SET")
            .arg(&team_key)
            .arg("none")
            .arg("EX")
            .arg(60)
            .query_async(&mut conn)
            .await
            .expect("seed team none");
        // Ensure no cached tenant_budget so the lookup must hit the (closed) DB.
        let _: i64 = redis::cmd("DEL")
            .arg(&tenant_budget_key)
            .query_async(&mut conn)
            .await
            .expect("del tenant_budget cache");
    }

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));
    // Close the pool so the tenant_budgets read errors (the only uncached read).
    pool.close().await;

    let err = tracker
        .check_budget(&wallet, 0.10, Some(tenant))
        .await
        .expect_err("enforced wallet + tenant_budgets DB error must deny");
    match err {
        UsageError::Database(_) => {}
        UsageError::TenantNotProvisioned { .. } => panic!(
            "a transient tenant_budgets DB error must surface as Database, not TenantNotProvisioned"
        ),
        other => panic!("expected Database, got {other:?}"),
    }

    // The transient error must NOT have cached a "none" sentinel for the tenant.
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("conn");
    let exists: bool = redis::cmd("EXISTS")
        .arg(&tenant_budget_key)
        .query_async(&mut conn)
        .await
        .expect("EXISTS");
    assert!(
        !exists,
        "a tenant_budgets DB error must NOT cache a 'none' sentinel (cache-poisoning)"
    );

    let now = Utc::now();
    redis_del(
        &client,
        &[
            &config_key,
            &team_key,
            &tenant_budget_key,
            &format!("spend:{}:{}", wallet, now.format("%Y-%m-%d")),
            &format!("spend:{}:{}", wallet, now.format("%Y-%m-%dT%H")),
        ],
    )
    .await;
}

/// N4 / 4a: the Skip path's `tenant_enforced` flag must flow from PRODUCTION code
/// into `log_spend`, never a hard-coded literal. `check_budget` on an unprovisioned
/// wallet returns a `BudgetReservation` whose `tenant_enforced()` is false; we
/// thread THAT value into `SpendLogEntry.tenant_enforced` (exactly as the chat
/// handler does) and assert `log_spend` writes NO per-tenant counter. A future
/// regression that set the flag true on Skip would write a tenant counter and
/// fail this test.
#[sqlx::test(migrations = "../../migrations")]
async fn skip_path_reservation_flag_suppresses_tenant_counter_through_real_path(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();
    let tenant = "acme";
    let now = Utc::now();

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));

    // Default wallet (no wallet_budgets row, no tenant_budgets row,
    // require_tenant=FALSE) → tenant decision is Skip even with a tag present.
    let reservation = tracker
        .check_budget(&wallet, 0.0050, Some(tenant))
        .await
        .expect("Skip-path request must be allowed under the default wallet cap");

    // The value MUST come from production code, not a literal.
    assert!(
        !reservation.tenant_enforced(),
        "Skip path must yield tenant_enforced=false from check_budget"
    );

    tracker.log_spend(SpendLogEntry {
        wallet_address: wallet.clone(),
        model: "openai/gpt-4o".to_string(),
        provider: "openai".to_string(),
        input_tokens: 10,
        output_tokens: 20,
        cost_usdc: 0.0050,
        tx_signature: None,
        request_id: None,
        session_id: None,
        tenant: Some(tenant.to_string()),
        // Sourced from the real reservation, NOT hard-coded.
        tenant_enforced: reservation.tenant_enforced(),
        estimated_cost_usdc: Some(0.0050),
        // Sourced from the real reservation too (Skip path → only the wallet
        // daily window reserved; no tenant windows).
        reserved: reservation.reserved_windows(),
        vendor: None,
        routing_tier: None,
        routing_score: None,
    });

    // The wallet daily counter must materialize (wallet accounting unaffected)...
    let wallet_day_key = format!("spend:{}:{}", wallet, now.format("%Y-%m-%d"));
    wait_for_key(&client, &wallet_day_key)
        .await
        .expect("wallet daily counter must be written");

    // ...but NO per-tenant counter may have been written on the Skip path.
    let tenant_day_key = format!("spend:{}:{}:{}", wallet, tenant, now.format("%Y-%m-%d"));
    let tenant_spend = get_redis_spend(&client, &tenant_day_key)
        .await
        .expect("get tenant spend");
    assert_eq!(
        tenant_spend, 0.0,
        "Skip-path reservation (tenant_enforced=false) must not write a per-tenant counter"
    );

    redis_del(&client, &[&wallet_day_key]).await;
    redis_del_tenant(&client, &wallet, tenant).await;
}

/// N4: a provisioned tenant row with NO hourly/daily/monthly limits commits zero
/// tenant counters, so `check_budget` must report `tenant_enforced=false` (only
/// true when at least one window counter was actually committed). Otherwise
/// `log_spend` would write per-tenant counters that `check_budget` never
/// reserved, breaking reserve/settle symmetry.
#[sqlx::test(migrations = "../../migrations")]
async fn limitless_provisioned_tenant_row_reports_not_enforced(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();
    let tenant = "acme";
    let now = Utc::now();

    sqlx::query(
        "INSERT INTO wallet_budgets (wallet_address, daily_limit_usdc) VALUES ($1, 100.00)",
    )
    .bind(&wallet)
    .execute(&pool)
    .await
    .expect("seed wallet");
    // Provisioned row with all-NULL limits (a real, present row, no caps).
    sqlx::query("INSERT INTO tenant_budgets (wallet_address, tenant) VALUES ($1, $2)")
        .bind(&wallet)
        .bind(tenant)
        .execute(&pool)
        .await
        .expect("seed limitless tenant budget");

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));
    let reservation = tracker
        .check_budget(&wallet, 0.0050, Some(tenant))
        .await
        .expect("limitless tenant row must pass under the wallet cap");

    assert!(
        !reservation.tenant_enforced(),
        "a limitless provisioned tenant row commits no counters → tenant_enforced must be false"
    );

    // And no tenant counter should have been committed.
    let tenant_day_key = format!("spend:{}:{}:{}", wallet, tenant, now.format("%Y-%m-%d"));
    let tenant_spend = get_redis_spend(&client, &tenant_day_key)
        .await
        .expect("get tenant spend");
    assert_eq!(
        tenant_spend, 0.0,
        "limitless tenant row must commit no per-tenant counter"
    );

    redis_del_tenant(&client, &wallet, tenant).await;
}

/// Partial tenant-window rollback (4b): a provisioned tenant row with hourly +
/// daily + monthly limits where MONTHLY is near-full. A request whose estimate
/// fits hourly+daily but trips monthly must roll back the hourly AND daily tenant
/// counters that were committed before the monthly window rejected — leaving both
/// back at 0.0 (mirrors the wallet-window rollback assertion).
#[sqlx::test(migrations = "../../migrations")]
async fn tenant_partial_window_rollback_on_monthly_trip(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();
    let tenant = "acme";
    let now = Utc::now();

    sqlx::query(
        "INSERT INTO wallet_budgets (wallet_address, daily_limit_usdc) VALUES ($1, 100.00)",
    )
    .bind(&wallet)
    .execute(&pool)
    .await
    .expect("seed wallet");
    // Tenant: generous hourly + daily, tight monthly (so monthly trips last).
    sqlx::query(
        "INSERT INTO tenant_budgets (wallet_address, tenant, hourly_limit_usdc, daily_limit_usdc, monthly_limit_usdc) \
         VALUES ($1, $2, 100.00, 100.00, 1.00)",
    )
    .bind(&wallet)
    .bind(tenant)
    .execute(&pool)
    .await
    .expect("seed tenant budget");

    // Pre-seed the MONTHLY tenant counter near its $1.00 cap so the request trips
    // monthly only (hourly+daily are generous and start empty).
    let tenant_hour_key = format!("spend:{}:{}:{}", wallet, tenant, now.format("%Y-%m-%dT%H"));
    let tenant_day_key = format!("spend:{}:{}:{}", wallet, tenant, now.format("%Y-%m-%d"));
    let tenant_month_key = format!("spend:{}:{}:{}", wallet, tenant, now.format("%Y-%m"));
    {
        let mut conn = client
            .get_multiplexed_async_connection()
            .await
            .expect("conn");
        let _: () = redis::cmd("SET")
            .arg(&tenant_month_key)
            .arg("0.95")
            .arg("EX")
            .arg(3600)
            .query_async(&mut conn)
            .await
            .expect("seed monthly near cap");
    }

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));

    // $0.10: hourly (0→0.10) and daily (0→0.10) commit, then monthly
    // (0.95→1.05 > 1.00) trips → all three must roll back.
    let err = tracker
        .check_budget(&wallet, 0.10, Some(tenant))
        .await
        .expect_err("monthly tenant cap must reject");
    match err {
        UsageError::BudgetExceeded { limit, .. } => {
            assert!(
                (limit - 1.0).abs() < 1e-9,
                "limit must be tenant monthly 1.0, got {limit}"
            );
        }
        other => panic!("expected BudgetExceeded, got {other:?}"),
    }

    // Hourly AND daily tenant counters must be back at 0.0 (rolled back).
    let leaked_hour = get_redis_spend(&client, &tenant_hour_key)
        .await
        .expect("get tenant hour");
    let leaked_day = get_redis_spend(&client, &tenant_day_key)
        .await
        .expect("get tenant day");
    assert_eq!(
        leaked_hour, 0.0,
        "tenant hourly counter must be rolled back to 0.0 after the monthly trip"
    );
    assert_eq!(
        leaked_day, 0.0,
        "tenant daily counter must be rolled back to 0.0 after the monthly trip"
    );
    // Monthly counter must be unchanged at its pre-seeded value (the trip rolls
    // back its own add inside the Lua script).
    let month_val = get_redis_spend(&client, &tenant_month_key)
        .await
        .expect("get tenant month");
    assert!(
        (month_val - 0.95).abs() < 1e-9,
        "tenant monthly counter must remain at the pre-seeded 0.95, got {month_val}"
    );

    redis_del_tenant(&client, &wallet, tenant).await;
}

/// Seed an org + team and map `wallet` into the team. Returns the `team_id`.
async fn seed_team_for_wallet(pool: &PgPool, wallet: &str) -> Uuid {
    let owner_wallet = format!("test_owner_{}", Uuid::new_v4().simple());
    let org_id: Uuid = sqlx::query_scalar(
        "INSERT INTO organizations (name, slug, owner_wallet) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind("test-org")
    .bind(format!("slug-{}", Uuid::new_v4().simple()))
    .bind(&owner_wallet)
    .fetch_one(pool)
    .await
    .expect("create org");

    let team_id: Uuid =
        sqlx::query_scalar("INSERT INTO teams (org_id, name) VALUES ($1, $2) RETURNING id")
            .bind(org_id)
            .bind(format!("team-{}", Uuid::new_v4().simple()))
            .fetch_one(pool)
            .await
            .expect("create team");

    sqlx::query("INSERT INTO team_wallets (team_id, wallet_address) VALUES ($1, $2)")
        .bind(team_id)
        .bind(wallet)
        .execute(pool)
        .await
        .expect("assign wallet to team");

    team_id
}

/// #501: a transient DB error reading `team_budgets` for a team member must
/// FAIL CLOSED — deny the request as a transient `UsageError::Database`, roll
/// back the already-committed wallet counters, and NEVER cache an error-derived
/// "no team budget" sentinel (which would silently disable the team cap for
/// every member of the team for a full TTL).
///
/// The DB error is forced deterministically by dropping the `team_budgets`
/// table inside this test's isolated `#[sqlx::test]` database: membership still
/// resolves via the separate `team_wallets` table, but the team-budget SELECT
/// errors with "relation does not exist" — a real, non-absence DB error.
#[sqlx::test(migrations = "../../migrations")]
async fn check_budget_team_config_db_error_fails_closed_and_rolls_back(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();
    let now = Utc::now();

    // Wallet is a team member with a permissive individual cap — if the team
    // path fail-OPENED, this request would sail through on the wallet cap.
    let team_id = seed_team_for_wallet(&pool, &wallet).await;
    sqlx::query(
        "INSERT INTO wallet_budgets (wallet_address, hourly_limit_usdc, daily_limit_usdc) \
         VALUES ($1, 50.00, 100.00)",
    )
    .bind(&wallet)
    .execute(&pool)
    .await
    .expect("seed permissive wallet caps");

    // Force a real DB error on the team_budgets read only (membership via
    // team_wallets still resolves). CASCADE drops the updated_at trigger too.
    sqlx::query("DROP TABLE team_budgets CASCADE")
        .execute(&pool)
        .await
        .expect("drop team_budgets to force a DB error on the team-budget read");

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));

    let err = tracker
        .check_budget(&wallet, 0.10, None)
        .await
        .expect_err("team_budgets DB error must fail closed (deny)");
    match err {
        // Transient infra error, retry-safe — NOT a silent skip of team enforcement.
        UsageError::Database(_) => {}
        other => panic!("expected transient Database error, got {other:?}"),
    }

    // Rollback completeness: the wallet hourly + daily counters reserved before
    // the team gate erred must be released (no leaked spend on a denied request).
    let day_key = format!("spend:{}:{}", wallet, now.format("%Y-%m-%d"));
    let hour_key = format!("spend:{}:{}", wallet, now.format("%Y-%m-%dT%H"));
    let leaked_day = get_redis_spend(&client, &day_key).await.expect("get day");
    let leaked_hour = get_redis_spend(&client, &hour_key).await.expect("get hour");
    assert_eq!(
        leaked_day, 0.0,
        "denied request must leak no wallet daily spend"
    );
    assert_eq!(
        leaked_hour, 0.0,
        "denied request must leak no wallet hourly spend"
    );

    // The error-derived "no team budget" answer must NOT have been cached: a
    // single transient blip must not poison the team cap for a full TTL.
    let team_cache_key = format!("team_budget:{team_id}");
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("conn");
    let cached: Option<String> = redis::cmd("GET")
        .arg(&team_cache_key)
        .query_async(&mut conn)
        .await
        .expect("read team_budget cache");
    assert_eq!(
        cached, None,
        "a team_budgets DB error must NOT be cached under team_budget:{{team_id}}"
    );

    redis_del(
        &client,
        &[
            &day_key,
            &hour_key,
            &team_cache_key,
            &format!("budget_config:{wallet}"),
            &format!("team_member:{wallet}"),
        ],
    )
    .await;
}

/// #501 regression guard: real team-row ABSENCE (query OK, no `team_budgets`
/// row) is NOT an error — the request proceeds (team enforcement skipped) AND
/// the `"none"` sentinel IS cached so repeated misses don't re-hit the DB.
#[sqlx::test(migrations = "../../migrations")]
async fn check_budget_team_config_absent_proceeds_and_caches_none(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();

    // Team member, but NO team_budgets row provisioned for the team.
    let team_id = seed_team_for_wallet(&pool, &wallet).await;
    let team_cache_key = format!("team_budget:{team_id}");
    redis_del(&client, &[&team_cache_key]).await;

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));

    tracker
        .check_budget(&wallet, 0.50, None)
        .await
        .expect("no team budget row → request proceeds (team enforcement skipped)");

    // The "none" sentinel must be cached for the absence (happy-path cache fill).
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("conn");
    let cached: Option<String> = redis::cmd("GET")
        .arg(&team_cache_key)
        .query_async(&mut conn)
        .await
        .expect("read team_budget cache");
    assert_eq!(
        cached.as_deref(),
        Some("none"),
        "a confirmed-absent team budget must cache the \"none\" sentinel"
    );

    let now = Utc::now();
    redis_del(
        &client,
        &[
            &team_cache_key,
            &format!("budget_config:{wallet}"),
            &format!("team_member:{wallet}"),
            &format!("spend:{}:{}", wallet, now.format("%Y-%m-%d")),
            &format!("spend:{}:{}", wallet, now.format("%Y-%m-%dT%H")),
            &format!("spend:{}:{}", wallet, now.format("%Y-%m")),
        ],
    )
    .await;
}

/// #501: a corrupt/undeserializable `team_budget:{team_id}` cache entry must be
/// DEL'd and the DB re-queried (mirrors the wallet path's corrupt-cache DEL),
/// rather than silently falling through and leaving the bad key to be re-read.
#[sqlx::test(migrations = "../../migrations")]
async fn check_budget_team_config_corrupt_cache_is_deled_and_requeried(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();

    // Team member with a real team budget in the DB ($0.10/day).
    let team_id = seed_team_for_wallet(&pool, &wallet).await;
    sqlx::query("INSERT INTO team_budgets (team_id, daily_limit_usdc) VALUES ($1, 0.10)")
        .bind(team_id)
        .execute(&pool)
        .await
        .expect("seed team budget");

    let team_cache_key = format!("team_budget:{team_id}");
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .expect("conn");
    // Prime a corrupt cache entry (not "none", not valid JSON).
    let _: () = redis::cmd("SET")
        .arg(&team_cache_key)
        .arg("not-valid-json")
        .arg("EX")
        .arg(60)
        .query_async(&mut conn)
        .await
        .expect("prime corrupt team cache");

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));

    // Request within the team cap → must proceed once the corrupt cache is
    // discarded and the real ($0.10) DB row is re-read.
    tracker
        .check_budget(&wallet, 0.05, None)
        .await
        .expect("corrupt team cache must be discarded and DB re-queried");

    // The corrupt entry must have been replaced by a valid serialized config
    // (DEL on detect, then the fresh DB read is re-cached) — never left corrupt.
    let cached: Option<String> = redis::cmd("GET")
        .arg(&team_cache_key)
        .query_async(&mut conn)
        .await
        .expect("read team_budget cache after re-query");
    let cached = cached.expect("team budget must be re-cached after corrupt-cache DEL");
    assert_ne!(
        cached, "not-valid-json",
        "the corrupt cache entry must not survive"
    );
    assert!(
        serde_json::from_str::<serde_json::Value>(&cached).is_ok(),
        "re-cached team budget must be valid JSON, got {cached}"
    );

    let now = Utc::now();
    redis_del(
        &client,
        &[
            &team_cache_key,
            &format!("budget_config:{wallet}"),
            &format!("team_member:{wallet}"),
            &format!("team_spend:{}:{}", team_id, now.format("%Y-%m-%d")),
            &format!("team_spend:{}:{}", team_id, now.format("%Y-%m-%dT%H")),
            &format!("team_spend:{}:{}", team_id, now.format("%Y-%m")),
            &format!("spend:{}:{}", wallet, now.format("%Y-%m-%d")),
        ],
    )
    .await;
}

// ---------------------------------------------------------------------------
// Spend-counter-drift regression (unreserved windows settle to ACTUAL, never
// the negative `actual − estimate` delta). Driven through the REAL path
// (check_budget → log_spend) so a store()-seeded shortcut can't mask a missing
// production write; the `reserved` flags are sourced from the returned
// `BudgetReservation`, exactly as the chat handler threads them.
// ---------------------------------------------------------------------------

/// Poll a Redis counter until it reaches `want` (± 1e-9) or the attempts run
/// out, returning the last observed value. Missing key reads as 0.0.
async fn poll_spend_until(client: &redis::Client, key: &str, want: f64) -> f64 {
    let mut last = f64::NAN;
    for _ in 0..100 {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        if let Ok(v) = get_redis_spend(client, key).await {
            last = v;
            if (last - want).abs() < 1e-9 {
                break;
            }
        }
    }
    last
}

/// Test A — WALLET family. A wallet with ONLY a daily limit (hourly + monthly
/// NULL) reserves the estimate against the daily window alone. With an actual
/// cost UNDER the estimate, the UNRESERVED monthly counter must settle to the
/// ACTUAL cost — never negative. Before the fix, `log_spend` applied the single
/// `(actual − estimate)` delta to every window, so the never-reserved monthly
/// counter drifted to `actual − estimate` (< 0); its 31d TTL (refreshed on every
/// incr) meant it never self-healed. Prod proof: wallet 39sVox… monthly =
/// -0.017976 (daily=5, hourly=NULL, monthly=NULL).
#[sqlx::test(migrations = "../../migrations")]
async fn wallet_unreserved_monthly_window_settles_to_actual_not_negative(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();
    let now = Utc::now();

    // Only a DAILY limit — hourly + monthly are NULL (never reserved).
    sqlx::query("INSERT INTO wallet_budgets (wallet_address, daily_limit_usdc) VALUES ($1, 5.00)")
        .bind(&wallet)
        .execute(&pool)
        .await
        .expect("seed wallet daily-only budget");

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));

    let estimated = 0.0050;
    let reservation = tracker
        .check_budget(&wallet, estimated, None)
        .await
        .expect("estimate must fit the daily cap");
    let rw = reservation.reserved_windows();
    assert!(
        rw.wallet.daily && !rw.wallet.hourly && !rw.wallet.monthly,
        "daily-only wallet must reserve the daily window alone, got {rw:?}"
    );

    // Actual usage lands UNDER the estimate — the drift-producing case.
    let actual = 0.0025;
    tracker.log_spend(SpendLogEntry {
        wallet_address: wallet.clone(),
        model: "openai/gpt-4o".to_string(),
        provider: "openai".to_string(),
        input_tokens: 10,
        output_tokens: 5,
        cost_usdc: actual,
        tx_signature: None,
        request_id: None,
        session_id: None,
        tenant: None,
        tenant_enforced: reservation.tenant_enforced(),
        estimated_cost_usdc: Some(estimated),
        reserved: rw,
        vendor: None,
        routing_tier: None,
        routing_score: None,
    });

    let month_key = format!("spend:{}:{}", wallet, now.format("%Y-%m"));
    let day_key = format!("spend:{}:{}", wallet, now.format("%Y-%m-%d"));
    let hour_key = format!("spend:{}:{}", wallet, now.format("%Y-%m-%dT%H"));

    // UNRESERVED monthly counter must settle to the actual cost, never the
    // negative `actual − estimate` the old single-scalar delta produced.
    let month = poll_spend_until(&client, &month_key, actual).await;
    assert!(
        (month - actual).abs() < 1e-9,
        "unreserved monthly counter must settle to the actual cost {actual}, got {month} \
         (negative ⇒ the spend-counter-drift bug)"
    );

    // The RESERVED daily window must ALSO net to actual (reserve + delta).
    let day = get_redis_spend(&client, &day_key).await.expect("get day");
    assert!(
        (day - actual).abs() < 1e-9,
        "reserved daily counter must net to actual {actual}, got {day}"
    );

    redis_del(
        &client,
        &[
            &hour_key,
            &day_key,
            &month_key,
            &format!("budget_config:{wallet}"),
            &format!("team_member:{wallet}"),
        ],
    )
    .await;
}

/// Test B — PER-TENANT family. A provisioned tenant budget with ONLY a daily
/// limit reserves the tenant daily window alone, yet `log_spend` writes all
/// three tenant windows. The UNRESERVED tenant monthly counter must settle to
/// the ACTUAL cost, never the negative delta.
#[sqlx::test(migrations = "../../migrations")]
async fn tenant_unreserved_monthly_window_settles_to_actual_not_negative(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();
    let tenant = "acme";
    let now = Utc::now();

    sqlx::query(
        "INSERT INTO wallet_budgets (wallet_address, daily_limit_usdc) VALUES ($1, 100.00)",
    )
    .bind(&wallet)
    .execute(&pool)
    .await
    .expect("seed wallet");
    // Tenant: DAILY limit only (hourly + monthly NULL → never reserved).
    sqlx::query(
        "INSERT INTO tenant_budgets (wallet_address, tenant, daily_limit_usdc) VALUES ($1, $2, 10.00)",
    )
    .bind(&wallet)
    .bind(tenant)
    .execute(&pool)
    .await
    .expect("seed tenant daily-only budget");

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));

    let estimated = 0.0050;
    let reservation = tracker
        .check_budget(&wallet, estimated, Some(tenant))
        .await
        .expect("estimate must fit the tenant daily cap");
    let rw = reservation.reserved_windows();
    assert!(
        rw.tenant.daily && !rw.tenant.hourly && !rw.tenant.monthly,
        "tenant daily-only must reserve the tenant daily window alone, got {rw:?}"
    );
    assert!(
        reservation.tenant_enforced(),
        "a provisioned tenant with a daily limit must report enforced"
    );

    let actual = 0.0025;
    tracker.log_spend(SpendLogEntry {
        wallet_address: wallet.clone(),
        model: "openai/gpt-4o".to_string(),
        provider: "openai".to_string(),
        input_tokens: 10,
        output_tokens: 5,
        cost_usdc: actual,
        tx_signature: None,
        request_id: None,
        session_id: None,
        tenant: Some(tenant.to_string()),
        tenant_enforced: reservation.tenant_enforced(),
        estimated_cost_usdc: Some(estimated),
        reserved: rw,
        vendor: None,
        routing_tier: None,
        routing_score: None,
    });

    let tenant_month_key = format!("spend:{}:{}:{}", wallet, tenant, now.format("%Y-%m"));
    let month = poll_spend_until(&client, &tenant_month_key, actual).await;
    assert!(
        (month - actual).abs() < 1e-9,
        "unreserved tenant monthly counter must settle to the actual cost {actual}, got {month} \
         (negative ⇒ the spend-counter-drift bug)"
    );

    // The reserved tenant daily window nets to actual too.
    let tenant_day_key = format!("spend:{}:{}:{}", wallet, tenant, now.format("%Y-%m-%d"));
    let day = get_redis_spend(&client, &tenant_day_key)
        .await
        .expect("get tenant day");
    assert!(
        (day - actual).abs() < 1e-9,
        "reserved tenant daily counter must net to actual {actual}, got {day}"
    );

    redis_del_tenant(&client, &wallet, tenant).await;
}

/// Test C — TEAM family. A team budget with ONLY a daily limit reserves the team
/// daily window alone, yet `log_spend` writes all three team windows. The
/// UNRESERVED team monthly counter must settle to the ACTUAL cost, never the
/// negative delta.
#[sqlx::test(migrations = "../../migrations")]
async fn team_unreserved_monthly_window_settles_to_actual_not_negative(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();
    let now = Utc::now();

    // Wallet in a team; team has ONLY a daily limit. Wallet uses the default
    // $100/day cap (no wallet_budgets row).
    let team_id = seed_team_for_wallet(&pool, &wallet).await;
    sqlx::query("INSERT INTO team_budgets (team_id, daily_limit_usdc) VALUES ($1, 10.00)")
        .bind(team_id)
        .execute(&pool)
        .await
        .expect("seed team daily-only budget");

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));

    let estimated = 0.0050;
    let reservation = tracker
        .check_budget(&wallet, estimated, None)
        .await
        .expect("estimate must fit the team daily cap");
    let rw = reservation.reserved_windows();
    assert!(
        rw.team.daily && !rw.team.hourly && !rw.team.monthly,
        "team daily-only must reserve the team daily window alone, got {rw:?}"
    );

    let actual = 0.0025;
    tracker.log_spend(SpendLogEntry {
        wallet_address: wallet.clone(),
        model: "openai/gpt-4o".to_string(),
        provider: "openai".to_string(),
        input_tokens: 10,
        output_tokens: 5,
        cost_usdc: actual,
        tx_signature: None,
        request_id: None,
        session_id: None,
        tenant: None,
        tenant_enforced: reservation.tenant_enforced(),
        estimated_cost_usdc: Some(estimated),
        reserved: rw,
        vendor: None,
        routing_tier: None,
        routing_score: None,
    });

    let team_month_key = format!("team_spend:{}:{}", team_id, now.format("%Y-%m"));
    let month = poll_spend_until(&client, &team_month_key, actual).await;
    assert!(
        (month - actual).abs() < 1e-9,
        "unreserved team monthly counter must settle to the actual cost {actual}, got {month} \
         (negative ⇒ the spend-counter-drift bug)"
    );

    // The reserved team daily window nets to actual too.
    let team_day_key = format!("team_spend:{}:{}", team_id, now.format("%Y-%m-%d"));
    let day = get_redis_spend(&client, &team_day_key)
        .await
        .expect("get team day");
    assert!(
        (day - actual).abs() < 1e-9,
        "reserved team daily counter must net to actual {actual}, got {day}"
    );

    redis_del(
        &client,
        &[
            &team_month_key,
            &team_day_key,
            &format!("team_spend:{}:{}", team_id, now.format("%Y-%m-%dT%H")),
            &format!("budget_config:{wallet}"),
            &format!("team_member:{wallet}"),
            &format!("team_budget:{team_id}"),
            &format!("spend:{}:{}", wallet, now.format("%Y-%m-%d")),
            &format!("spend:{}:{}", wallet, now.format("%Y-%m-%dT%H")),
            &format!("spend:{}:{}", wallet, now.format("%Y-%m")),
        ],
    )
    .await;
}

/// Test D — regression guard. A FULLY-limited wallet (hourly + daily + monthly
/// all set) reserves the estimate on ALL three windows, so each must net to
/// EXACTLY the actual cost after `log_spend` (reserve + `(actual − estimate)`
/// delta) — no double count. This passed before the fix too and MUST keep
/// passing: it pins that the per-window reconciliation never regresses a
/// reserved window into applying the full cost on top of its reservation.
#[sqlx::test(migrations = "../../migrations")]
async fn fully_limited_wallet_all_windows_net_to_actual_no_double_count(pool: PgPool) {
    let client = redis_client();
    let wallet = unique_wallet();
    let now = Utc::now();

    sqlx::query(
        "INSERT INTO wallet_budgets \
         (wallet_address, hourly_limit_usdc, daily_limit_usdc, monthly_limit_usdc) \
         VALUES ($1, 50.00, 100.00, 500.00)",
    )
    .bind(&wallet)
    .execute(&pool)
    .await
    .expect("seed fully-limited wallet");

    let tracker = UsageTracker::new(Some(pool.clone()), Some(client.clone()));

    let estimated = 0.0050;
    let reservation = tracker
        .check_budget(&wallet, estimated, None)
        .await
        .expect("estimate must fit all three caps");
    let rw = reservation.reserved_windows();
    assert!(
        rw.wallet.hourly && rw.wallet.daily && rw.wallet.monthly,
        "a fully-limited wallet must reserve all three windows, got {rw:?}"
    );

    // Actual UNDER estimate exercises the negative delta on reserved windows.
    let actual = 0.0025;
    tracker.log_spend(SpendLogEntry {
        wallet_address: wallet.clone(),
        model: "openai/gpt-4o".to_string(),
        provider: "openai".to_string(),
        input_tokens: 10,
        output_tokens: 5,
        cost_usdc: actual,
        tx_signature: None,
        request_id: None,
        session_id: None,
        tenant: None,
        tenant_enforced: reservation.tenant_enforced(),
        estimated_cost_usdc: Some(estimated),
        reserved: rw,
        vendor: None,
        routing_tier: None,
        routing_score: None,
    });

    let hour_key = format!("spend:{}:{}", wallet, now.format("%Y-%m-%dT%H"));
    let day_key = format!("spend:{}:{}", wallet, now.format("%Y-%m-%d"));
    let month_key = format!("spend:{}:{}", wallet, now.format("%Y-%m"));

    // Every reserved window must net to EXACTLY actual — no window double-counts
    // (which would land at estimate + actual).
    for k in [&hour_key, &day_key, &month_key] {
        let settled = poll_spend_until(&client, k, actual).await;
        assert!(
            (settled - actual).abs() < 1e-9,
            "reserved window {k} must net to exactly actual {actual} (no double count), got {settled}"
        );
    }

    redis_del(
        &client,
        &[
            &hour_key,
            &day_key,
            &month_key,
            &format!("budget_config:{wallet}"),
            &format!("team_member:{wallet}"),
        ],
    )
    .await;
}
