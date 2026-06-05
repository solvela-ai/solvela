//! Per-wallet usage tracking and budget management.
//!
//! PostgreSQL for persistent spend logs, Redis for hot-path spend tracking.
//! All DB writes are async (tokio::spawn) — never on the request critical path.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};
use uuid::Uuid;

/// Default daily limit when no per-wallet budget row exists.
///
/// **Single source of truth for both the enforcement path** (this module's
/// `BudgetConfig::default()`) **and the display path** (the
/// `routes::orgs::budget::get_wallet_budget` handler). Diverging defaults
/// would let the API report a different limit than the gate enforces.
///
/// TODO(GHSA-86cr-h3rx-vj6j): migrate budget limits and spend counters from f64
/// USDC to integer atomic units (u64, 6 decimal places) throughout this module
/// and the Redis INCRBYFLOAT paths below.  This is the broader refactor deferred
/// from the GHSA-86cr-h3rx-vj6j advisory; the immediate input-validation fix for
/// `estimated_atomic_cost` is in `routes/chat/cost.rs`.
pub const DEFAULT_DAILY_LIMIT_USDC: f64 = 100.0;

/// TTL for cached wallet budget config in Redis (seconds).
const BUDGET_CONFIG_CACHE_TTL: u64 = 60;

/// TTL for cached team membership lookups in Redis (seconds).
const TEAM_MEMBER_CACHE_TTL: u64 = 60;

/// TTL for cached team budget config in Redis (seconds).
const TEAM_BUDGET_CACHE_TTL: u64 = 60;

/// Cached budget configuration for a wallet, stored in Redis as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetConfig {
    pub hourly: Option<f64>,
    pub daily: Option<f64>,
    pub monthly: Option<f64>,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            hourly: None,
            daily: Some(DEFAULT_DAILY_LIMIT_USDC),
            monthly: None,
        }
    }
}

/// Cached team budget configuration, stored in Redis as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamBudgetConfig {
    pub hourly: Option<f64>,
    pub daily: Option<f64>,
    pub monthly: Option<f64>,
}

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
    pub hourly_limit_usdc: Option<f64>,
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
    /// Cost that was tentatively committed to the Redis spend counters at
    /// `check_budget` time. When `Some`, `log_spend` increments the counters
    /// by `(cost_usdc - estimated_cost_usdc)` so the ledger settles to the
    /// actual cost without double-counting the reservation. When `None`, the
    /// counters were not pre-committed (legacy / proxy / test paths) and
    /// `log_spend` increments by `cost_usdc` directly.
    pub estimated_cost_usdc: Option<f64>,
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

    /// Access the Redis client, if configured.
    ///
    /// Used by budget management endpoints to read current spend counters.
    pub fn redis_client(&self) -> Option<&redis::Client> {
        self.redis_client.as_ref()
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

        // Session tokens are bearer-equivalent: anyone with a leaked log line
        // can replay session-authenticated requests without paying. Log only
        // an 8-char correlation prefix so entries can be matched without
        // exposing the full token. Wallet pubkey, request_id, and tx_signature
        // remain in full — they're public on-chain or correlation-only.
        let session_prefix = entry
            .session_id
            .as_deref()
            .map(|s| s.chars().take(8).collect::<String>())
            .unwrap_or_else(|| "none".to_string());
        info!(
            wallet = %entry.wallet_address,
            model = %entry.model,
            provider = %entry.provider,
            input_tokens = entry.input_tokens,
            output_tokens = entry.output_tokens,
            cost_usdc = entry.cost_usdc,
            tx_signature = entry.tx_signature.as_deref().unwrap_or("none"),
            request_id = entry.request_id.as_deref().unwrap_or("none"),
            session_prefix = %session_prefix,
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

        // Update Redis hot-path counters.
        //
        // If `estimated_cost_usdc` is `Some`, `check_budget` already committed
        // that amount to each window's counter via the atomic INCRBYFLOAT in
        // `incr_check_or_rollback` (the H1 fix). To avoid double-counting,
        // we increment by the *delta* (`cost - estimated`) here, which can
        // be negative if actual usage came in under the estimate. If
        // `estimated_cost_usdc` is `None` no reservation was committed
        // (legacy / proxy / test paths), so we increment by the full cost.
        if let Some(client) = &self.redis_client {
            let client = client.clone();
            let db_pool = self.db_pool.clone();
            let wallet = entry.wallet_address;
            let cost = match entry.estimated_cost_usdc {
                Some(reserved) => entry.cost_usdc - reserved,
                None => entry.cost_usdc,
            };
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

                let now = Utc::now();

                // Hourly spend counter
                let hour_key = format!("spend:{}:{}", wallet, now.format("%Y-%m-%dT%H"));
                incr_and_expire(&mut conn, &hour_key, cost, 7200).await;

                // Daily spend counter
                let day_key = format!("spend:{}:{}", wallet, now.format("%Y-%m-%d"));
                incr_and_expire(&mut conn, &day_key, cost, 86400).await;

                // Monthly spend counter
                let month_key = format!("spend:{}:{}", wallet, now.format("%Y-%m"));
                incr_and_expire(&mut conn, &month_key, cost, 86400 * 31).await;

                // Team-level counters: look up team membership.
                // log_spend runs in a fire-and-forget tokio::spawn after the
                // request has been served, so a DB error here can't deny the
                // request — we log and skip team bookkeeping (the wallet
                // counter is still updated). check_budget enforces the
                // strict fail-closed semantic on the request hot path.
                let team_id = match get_team_for_wallet(&mut conn, db_pool.as_ref(), &wallet).await
                {
                    Ok(t) => t,
                    Err(e) => {
                        warn!(
                            wallet = %wallet,
                            error = %e,
                            "team membership lookup failed during spend log; skipping team counters"
                        );
                        None
                    }
                };
                if let Some(tid) = team_id {
                    let tid_str = tid.to_string();
                    let team_hour_key =
                        format!("team_spend:{}:{}", tid_str, now.format("%Y-%m-%dT%H"));
                    incr_and_expire(&mut conn, &team_hour_key, cost, 7200).await;

                    let team_day_key = format!("team_spend:{}:{}", tid_str, now.format("%Y-%m-%d"));
                    incr_and_expire(&mut conn, &team_day_key, cost, 86400).await;

                    let team_month_key = format!("team_spend:{}:{}", tid_str, now.format("%Y-%m"));
                    incr_and_expire(&mut conn, &team_month_key, cost, 86400 * 31).await;
                }
            });
        }
    }

    /// Check if a wallet's budget allows a request with the estimated cost.
    ///
    /// Returns `Ok(())` if within budget, `Err(UsageError::BudgetExceeded)` if not.
    ///
    /// Checks wallet-level hourly, daily, and monthly limits (read from DB with
    /// Redis caching), then checks team-level limits if the wallet belongs to a team.
    ///
    /// **No-Redis fallback**: when Redis is unavailable and no client is configured,
    /// a conservative per-request cap of $1.00 USDC is applied to prevent runaway
    /// spend on high-cost models.  Requests with an estimated cost at or below $1.00
    /// are allowed through; above that they are rejected.
    ///
    /// **Fail-closed on Redis errors**: When a Redis client IS configured but a
    /// GET command fails at request time (e.g., Redis is temporarily down or
    /// returns an unexpected error), the budget check returns
    /// `Err(UsageError::Redis(...))` and the request is **denied**. We cannot
    /// verify that the wallet has budget headroom, so we must not allow the
    /// request through — an unverifiable spend limit is treated as exceeded.
    /// The connection-level failure path (unable to acquire a connection at all)
    /// is still logged as a warning and fails closed via `Err(UsageError::Redis)`.
    pub async fn check_budget(
        &self,
        wallet_address: &str,
        estimated_cost_usdc: f64,
    ) -> Result<BudgetReservation, UsageError> {
        // No Redis configured — apply a conservative per-request hard cap.
        // No counters are committed, so the reservation is empty (release is a no-op).
        if self.redis_client.is_none() {
            const NO_REDIS_REQUEST_CAP_USDC: f64 = 1.0;
            if estimated_cost_usdc > NO_REDIS_REQUEST_CAP_USDC {
                return Err(UsageError::BudgetExceeded {
                    wallet: wallet_address.to_string(),
                    limit: NO_REDIS_REQUEST_CAP_USDC,
                    spent: estimated_cost_usdc,
                });
            }
            return Ok(BudgetReservation::default());
        }

        // Try Redis hot-path budget check.
        let Some(client) = &self.redis_client else {
            return Ok(BudgetReservation::default());
        };

        let mut conn = match client.get_multiplexed_async_connection().await {
            Ok(c) => c,
            Err(e) => {
                // Fail-closed: deny the request when we cannot reach Redis.
                warn!(
                    wallet = %wallet_address,
                    estimated_cost_usdc = estimated_cost_usdc,
                    error = %e,
                    "budget_check_denied: Redis connection failed, denying request (fail-closed)"
                );
                return Err(UsageError::Redis(e.to_string()));
            }
        };

        let now = Utc::now();

        // Tracks counters we've already incremented in this call, so we can
        // roll them all back if a later window exceeds its limit. Without
        // this, a request that fits hourly+daily but not monthly would leave
        // hourly and daily over-counted by `estimated_cost_usdc`.
        let mut committed: Vec<(String, f64)> = Vec::new();

        // Helper: try to commit one window. On exceeded → roll back everything
        // accumulated so far + return the BudgetExceeded error. On Redis error
        // → roll back + propagate as Redis error.
        macro_rules! try_commit {
            ($key:expr, $amount:expr, $limit:expr, $ttl:expr) => {{
                let key: String = $key;
                match incr_check_or_rollback(&mut conn, &key, $amount, $limit, $ttl).await {
                    Ok(new_total) => {
                        committed.push((key, $amount));
                        new_total
                    }
                    Err(IncrCheckResult::Exceeded { current }) => {
                        // `current` is the post-add value the counter held
                        // before Lua rolled it back — i.e. what the spend
                        // would have been if we'd committed. That's exactly
                        // the "spent" amount the BudgetExceeded error wants
                        // to report.
                        rollback_committed(&mut conn, &committed).await;
                        return Err(UsageError::BudgetExceeded {
                            wallet: wallet_address.to_string(),
                            limit: $limit,
                            spent: current,
                        });
                    }
                    Err(IncrCheckResult::Redis(msg)) => {
                        rollback_committed(&mut conn, &committed).await;
                        return Err(UsageError::Redis(msg));
                    }
                }
            }};
        }

        // Load per-wallet budget config (DB-backed, cached in Redis 60s)
        let config =
            get_wallet_budget_config(&mut conn, self.db_pool.as_ref(), wallet_address).await;

        // --- Wallet hourly limit ---
        if let Some(hourly_limit) = config.hourly {
            let _ = try_commit!(
                format!("spend:{}:{}", wallet_address, now.format("%Y-%m-%dT%H")),
                estimated_cost_usdc,
                hourly_limit,
                7200
            );
        }

        // --- Wallet daily limit ---
        if let Some(daily_limit) = config.daily {
            let _ = try_commit!(
                format!("spend:{}:{}", wallet_address, now.format("%Y-%m-%d")),
                estimated_cost_usdc,
                daily_limit,
                86400
            );
        }

        // --- Wallet monthly limit ---
        if let Some(monthly_limit) = config.monthly {
            let _ = try_commit!(
                format!("spend:{}:{}", wallet_address, now.format("%Y-%m")),
                estimated_cost_usdc,
                monthly_limit,
                86400 * 31
            );
        }

        // --- Team-level budget enforcement ---
        // H4 fix: get_team_for_wallet now returns Result. A DB error
        // propagates as UsageError::Database, denying the request rather
        // than silently skipping team enforcement (the previous behavior
        // was fail-open: a transient DB blip would let the wallet's
        // permissive individual budget bypass a tighter team cap).
        match get_team_for_wallet(&mut conn, self.db_pool.as_ref(), wallet_address).await {
            Ok(Some(tid)) => {
                let team_config =
                    get_team_budget_config(&mut conn, self.db_pool.as_ref(), tid).await;

                if let Some(team_cfg) = team_config {
                    let tid_str = tid.to_string();

                    if let Some(hourly_limit) = team_cfg.hourly {
                        let _ = try_commit!(
                            format!("team_spend:{}:{}", tid_str, now.format("%Y-%m-%dT%H")),
                            estimated_cost_usdc,
                            hourly_limit,
                            7200
                        );
                    }
                    if let Some(daily_limit) = team_cfg.daily {
                        let _ = try_commit!(
                            format!("team_spend:{}:{}", tid_str, now.format("%Y-%m-%d")),
                            estimated_cost_usdc,
                            daily_limit,
                            86400
                        );
                    }
                    if let Some(monthly_limit) = team_cfg.monthly {
                        let _ = try_commit!(
                            format!("team_spend:{}:{}", tid_str, now.format("%Y-%m")),
                            estimated_cost_usdc,
                            monthly_limit,
                            86400 * 31
                        );
                    }
                }
            }
            Ok(None) => {
                // Wallet is not in any team — nothing to enforce.
            }
            Err(e) => {
                rollback_committed(&mut conn, &committed).await;
                warn!(
                    wallet = %wallet_address,
                    error = %e,
                    "team membership lookup failed; denying request (fail-closed)"
                );
                return Err(UsageError::Database(e));
            }
        }

        Ok(BudgetReservation { committed })
    }

    /// Release a budget reservation previously returned by [`check_budget`].
    ///
    /// Decrements exactly the Redis counters `check_budget` incremented, so the
    /// wallet's budget headroom is restored when a request is abandoned AFTER
    /// reserving but BEFORE the spend is realized — chiefly when on-chain
    /// payment settlement fails (M3). Without this, reserving before settlement
    /// would leak the reservation and permanently consume budget for a payment
    /// that never happened.
    ///
    /// Best-effort and infallible by design: Redis errors are logged, not
    /// propagated. A failed release at worst leaves a counter over-counted until
    /// its TTL expires (self-healing); returning an error here would only
    /// complicate an already-failing request path. No-op when the reservation is
    /// empty (no Redis, or no budget windows applied).
    ///
    /// [`check_budget`]: Self::check_budget
    pub async fn release_reservation(&self, reservation: &BudgetReservation) {
        if reservation.committed.is_empty() {
            return;
        }
        let Some(client) = &self.redis_client else {
            return;
        };
        let mut conn = match client.get_multiplexed_async_connection().await {
            Ok(c) => c,
            Err(e) => {
                warn!(
                    error = %e,
                    reserved_counters = ?reservation.committed,
                    "failed to acquire Redis connection to release budget reservation; \
                     these counters stay over-counted until their TTL expires"
                );
                return;
            }
        };
        rollback_committed(&mut conn, &reservation.committed).await;
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

            // Daily/monthly spend come from the same Redis counters the budget
            // path increments (`spend:{wallet}:{period}`). Read via the shared
            // `wallet_window_spend` helper so the stats HTTP endpoint and this
            // summary path use one key derivation — no copy-pasted key strings.
            // Redis-optional (Architectural Rule #12): absent Redis / missing /
            // undecodable counters fall back to 0.0, never an error.
            let (daily_cost_usdc, monthly_cost_usdc) =
                wallet_window_spend(self.redis_client(), wallet_address).await;

            return Ok(SpendSummary {
                wallet_address: wallet_address.to_string(),
                total_requests: row.0 as u64,
                total_input_tokens: row.1 as u64,
                total_output_tokens: row.2 as u64,
                total_cost_usdc: row.3,
                daily_cost_usdc,
                monthly_cost_usdc,
            });
        }

        Err(UsageError::NotConfigured)
    }
}

/// A committed budget reservation returned by [`UsageTracker::check_budget`].
///
/// Holds the exact Redis counters (key + amount) that were incremented, so they
/// can be released verbatim via [`UsageTracker::release_reservation`] if the
/// request fails before the spend is realized. Capturing the precise keys —
/// rather than re-deriving them at release time — makes the release correct even
/// if an hour/day boundary rolls over between reserve and release. Empty when no
/// Redis is configured or no budget windows applied; release is then a no-op.
///
/// Deliberately NOT `Clone`: a reservation maps 1:1 to committed Redis counters,
/// so a clone released separately would double-decrement them. There is exactly
/// one owner — the request that reserved.
#[derive(Debug, Default)]
pub struct BudgetReservation {
    committed: Vec<(String, f64)>,
}

// ---------------------------------------------------------------------------
// Redis + DB helper functions for budget enforcement
// ---------------------------------------------------------------------------

/// Increment a Redis key by `amount` and set its TTL atomically.
///
/// Both commands are sent in a single pipelined round-trip via `MULTI/EXEC`,
/// so a process crash between the two commands can never leave a spend key
/// without a TTL (which would otherwise permanently block future requests).
async fn incr_and_expire(
    conn: &mut redis::aio::MultiplexedConnection,
    key: &str,
    amount: f64,
    ttl_secs: u64,
) {
    let result: Result<(), redis::RedisError> = redis::pipe()
        .atomic()
        .cmd("INCRBYFLOAT")
        .arg(key)
        .arg(amount)
        .cmd("EXPIRE")
        .arg(key)
        .arg(ttl_secs)
        .query_async(conn)
        .await;

    if let Err(e) = result {
        warn!(error = %e, key = %key, "failed to atomically INCRBYFLOAT+EXPIRE in Redis");
    }
}

/// Lua script: atomically increment a counter, set TTL, then check the new
/// value against a limit. If the new value exceeds the limit, the increment
/// is rolled back inside the same atomic execution.
///
/// Closes the H1 TOCTOU: the previous `redis_get_f64 + arithmetic + later
/// log_spend INCRBYFLOAT` pattern let two concurrent requests both pass the
/// check before either committed, so total overshoot under burst could be
/// `N × estimated_cost`. Now the check and commit happen in a single
/// `EVAL`, serialized by Redis.
///
/// KEYS[1] = counter key
/// ARGV[1] = amount to add
/// ARGV[2] = limit + epsilon (the "exceeded" boundary)
/// ARGV[3] = TTL in seconds
///
/// Always returns the post-add value (the value the counter held immediately
/// after `INCRBYFLOAT amount`, even if a rollback was then issued). The Rust
/// caller compares this against the limit to decide ok vs. exceeded.
///
/// Note: returns the value as a STRING — Redis serializes Lua numbers as
/// int64 (truncating decimals), so we have to round-trip the float as a
/// string and parse it in Rust to preserve precision.
const INCR_CHECK_LUA: &str = r#"
local cur = redis.call('INCRBYFLOAT', KEYS[1], ARGV[1])
redis.call('EXPIRE', KEYS[1], ARGV[3])
if tonumber(cur) > tonumber(ARGV[2]) then
    redis.call('INCRBYFLOAT', KEYS[1], '-' .. ARGV[1])
end
return tostring(cur)
"#;

/// Atomically attempt to commit `amount` to a spend counter. Returns
/// `Ok(new_total)` on success — the counter was incremented and persists.
/// Returns `Err(BudgetExceeded { current })` when the counter would have
/// exceeded `limit + USDC_EPSILON`; in that case the increment was rolled
/// back and `current` reflects the pre-call value.
///
/// Errors other than budget-exceeded propagate as `UsageError::Redis` so the
/// caller can fail closed.
async fn incr_check_or_rollback(
    conn: &mut redis::aio::MultiplexedConnection,
    key: &str,
    amount: f64,
    limit: f64,
    ttl_secs: u64,
) -> Result<f64, IncrCheckResult> {
    let limit_with_epsilon = limit + USDC_EPSILON;
    // Lua returns the post-add value as a string (Redis truncates Lua
    // numbers to int64 on serialization, so we have to keep the float in
    // string form across the Redis reply boundary).
    let new_value_str: String = redis::Script::new(INCR_CHECK_LUA)
        .key(key)
        .arg(amount.to_string())
        .arg(limit_with_epsilon.to_string())
        .arg(ttl_secs)
        .invoke_async(conn)
        .await
        .map_err(|e| IncrCheckResult::Redis(e.to_string()))?;
    let new_value: f64 = new_value_str
        .parse()
        .map_err(|e| IncrCheckResult::Redis(format!("non-numeric script reply: {e}")))?;

    if new_value > limit_with_epsilon {
        // Lua already rolled back the INCRBYFLOAT; `new_value` is the
        // would-have-been value (post-add), which is exactly what the
        // BudgetExceeded error wants to report as "spent".
        Err(IncrCheckResult::Exceeded { current: new_value })
    } else {
        Ok(new_value)
    }
}

/// Internal result type for `incr_check_or_rollback` — separate from
/// `UsageError` so the caller can distinguish "budget exceeded" (a normal
/// flow control case) from "Redis unreachable" (fail-closed).
enum IncrCheckResult {
    Exceeded { current: f64 },
    Redis(String),
}

/// Roll back a list of previously-committed counters. Used when a later
/// budget window in the same `check_budget` call exceeds its limit, so the
/// earlier-committed reservations need to be released.
///
/// Each rollback is best-effort: if Redis errors here we log and continue.
/// The alternative is to return an error and have the caller retry the
/// rollback, which adds complexity for a tail case.
async fn rollback_committed(
    conn: &mut redis::aio::MultiplexedConnection,
    keys_and_amounts: &[(String, f64)],
) {
    for (key, amount) in keys_and_amounts {
        let neg = -*amount;
        let result: Result<f64, redis::RedisError> = redis::cmd("INCRBYFLOAT")
            .arg(key)
            .arg(neg)
            .query_async(conn)
            .await;
        if let Err(e) = result {
            warn!(
                error = %e,
                key = %key,
                amount,
                "failed to roll back budget reservation after later window exceeded"
            );
        }
    }
}

/// Read an f64 value from Redis for budget enforcement (fail-closed).
///
/// Returns `Ok(val)` on a cache hit, `Ok(0.0)` on a cache miss (key not set
/// yet means no spend has been recorded), and `Err(String)` on a Redis error.
///
/// Callers on the **enforcement path** (`check_budget`) must propagate the
/// error and deny the request — if we cannot verify spend we must not allow
/// the request through.  Display-only callers (budget GET endpoints) use
/// `get_redis_spend` which applies `.unwrap_or(0.0)` itself.
async fn redis_get_f64(
    conn: &mut redis::aio::MultiplexedConnection,
    key: &str,
) -> Result<f64, String> {
    match redis::cmd("GET")
        .arg(key)
        .query_async::<Option<f64>>(conn)
        .await
    {
        Ok(Some(val)) => Ok(val),
        Ok(None) => Ok(0.0), // key absent = no spend recorded yet
        Err(e) => {
            warn!(key = %key, error = %e, "Redis GET failed — denying request (fail-closed)");
            Err(e.to_string())
        }
    }
}

/// Restrictive budget config returned on DB errors to fail-closed.
/// $1/day prevents silent over-spending during DB outages.
fn restrictive_budget_fallback() -> BudgetConfig {
    BudgetConfig {
        hourly: Some(0.50),
        daily: Some(1.0),
        monthly: Some(10.0),
    }
}

/// Load per-wallet budget config. Checks Redis cache first (`budget_config:{wallet}`),
/// falls back to DB query, caches result in Redis with 60s TTL.
/// Returns default config ($100/day) if no row exists; restrictive fallback on DB error.
async fn get_wallet_budget_config(
    conn: &mut redis::aio::MultiplexedConnection,
    db_pool: Option<&sqlx::PgPool>,
    wallet: &str,
) -> BudgetConfig {
    let cache_key = format!("budget_config:{wallet}");

    // Try Redis cache first
    if let Ok(Some(json_str)) = redis::cmd("GET")
        .arg(&cache_key)
        .query_async::<Option<String>>(conn)
        .await
    {
        match serde_json::from_str::<BudgetConfig>(&json_str) {
            Ok(config) => return config,
            Err(e) => {
                tracing::warn!(cache_key = %cache_key, error = %e, "corrupted cache entry, falling through to DB");
                let _ = redis::cmd("DEL")
                    .arg(&cache_key)
                    .query_async::<()>(conn)
                    .await;
            }
        }
    }

    // Cache miss — query DB
    let config = if let Some(pool) = db_pool {
        match sqlx::query_as::<_, (Option<f64>, Option<f64>, Option<f64>)>(
            r#"SELECT
                hourly_limit_usdc::DOUBLE PRECISION,
                daily_limit_usdc::DOUBLE PRECISION,
                monthly_limit_usdc::DOUBLE PRECISION
            FROM wallet_budgets
            WHERE wallet_address = $1"#,
        )
        .bind(wallet)
        .fetch_optional(pool)
        .await
        {
            Ok(Some((hourly, daily, monthly))) => BudgetConfig {
                hourly,
                daily: daily.or(Some(DEFAULT_DAILY_LIMIT_USDC)),
                monthly,
            },
            Ok(None) => BudgetConfig::default(),
            Err(e) => {
                warn!(wallet = %wallet, error = %e, "failed to query wallet_budgets — using restrictive fallback");
                restrictive_budget_fallback()
            }
        }
    } else {
        BudgetConfig::default()
    };

    // Cache in Redis (best-effort)
    if let Ok(json_str) = serde_json::to_string(&config) {
        if let Err(e) = redis::cmd("SET")
            .arg(&cache_key)
            .arg(&json_str)
            .arg("EX")
            .arg(BUDGET_CONFIG_CACHE_TTL)
            .query_async::<()>(conn)
            .await
        {
            tracing::warn!(cache_key = %cache_key, error = %e, "failed to write to Redis cache");
        }
    }

    config
}

/// Look up the team_id for a wallet. Checks Redis cache (`team_member:{wallet}`),
/// falls back to DB query on `team_wallets`. Returns `None` if not in any team.
/// Look up the team a wallet belongs to.
///
/// Returns `Ok(Some(team_id))` when the wallet is mapped to a team,
/// `Ok(None)` when not, and `Err(msg)` on a DB error.
///
/// H4 fix: previously returned `Option<Uuid>` and silently swallowed DB
/// errors as `None`, which fail-opened team budget enforcement on
/// transient DB blips. The check_budget caller now propagates the error
/// as `UsageError::Database` and denies the request.
async fn get_team_for_wallet(
    conn: &mut redis::aio::MultiplexedConnection,
    db_pool: Option<&sqlx::PgPool>,
    wallet: &str,
) -> Result<Option<Uuid>, String> {
    let cache_key = format!("team_member:{wallet}");

    // Try Redis cache
    if let Ok(Some(tid_str)) = redis::cmd("GET")
        .arg(&cache_key)
        .query_async::<Option<String>>(conn)
        .await
    {
        // A cached "none" sentinel means the wallet has no team
        if tid_str == "none" {
            return Ok(None);
        }
        if let Ok(tid) = tid_str.parse::<Uuid>() {
            return Ok(Some(tid));
        }
    }

    // Cache miss — query DB
    let team_id = if let Some(pool) = db_pool {
        match sqlx::query_as::<_, (Uuid,)>(
            "SELECT team_id FROM team_wallets WHERE wallet_address = $1 LIMIT 1",
        )
        .bind(wallet)
        .fetch_optional(pool)
        .await
        {
            Ok(Some((tid,))) => Some(tid),
            Ok(None) => None,
            Err(e) => {
                // Propagate the error so check_budget can fail closed.
                error!(wallet = %wallet, error = %e, "failed to query team_wallets");
                return Err(format!("team_wallets lookup failed: {e}"));
            }
        }
    } else {
        // No DB pool — wallet has no team membership we can verify, but
        // this isn't an error: the operator chose to run without a DB.
        None
    };

    // Cache result (including "none" sentinel to avoid repeated DB misses)
    let cache_val = team_id
        .map(|tid| tid.to_string())
        .unwrap_or_else(|| "none".to_string());
    let _: Result<(), _> = redis::cmd("SET")
        .arg(&cache_key)
        .arg(&cache_val)
        .arg("EX")
        .arg(TEAM_MEMBER_CACHE_TTL)
        .query_async(conn)
        .await;

    Ok(team_id)
}

/// Load team budget config from `team_budgets` table. Cached in Redis with 60s TTL.
/// Returns `None` if no budget row exists for the team.
async fn get_team_budget_config(
    conn: &mut redis::aio::MultiplexedConnection,
    db_pool: Option<&sqlx::PgPool>,
    team_id: Uuid,
) -> Option<TeamBudgetConfig> {
    let cache_key = format!("team_budget:{team_id}");

    // Try Redis cache
    if let Ok(Some(json_str)) = redis::cmd("GET")
        .arg(&cache_key)
        .query_async::<Option<String>>(conn)
        .await
    {
        if json_str == "none" {
            return None;
        }
        if let Ok(config) = serde_json::from_str::<TeamBudgetConfig>(&json_str) {
            return Some(config);
        }
    }

    // Cache miss — query DB
    let config = if let Some(pool) = db_pool {
        match sqlx::query_as::<_, (Option<f64>, Option<f64>, Option<f64>)>(
            r#"SELECT
                hourly_limit_usdc::DOUBLE PRECISION,
                daily_limit_usdc::DOUBLE PRECISION,
                monthly_limit_usdc::DOUBLE PRECISION
            FROM team_budgets
            WHERE team_id = $1"#,
        )
        .bind(team_id)
        .fetch_optional(pool)
        .await
        {
            Ok(Some((hourly, daily, monthly))) => Some(TeamBudgetConfig {
                hourly,
                daily,
                monthly,
            }),
            Ok(None) => None,
            Err(e) => {
                warn!(team_id = %team_id, error = %e, "failed to query team_budgets");
                None
            }
        }
    } else {
        None
    };

    // Cache result
    let cache_val = match &config {
        Some(cfg) => serde_json::to_string(cfg).unwrap_or_else(|_| "none".to_string()),
        None => "none".to_string(),
    };
    let _: Result<(), _> = redis::cmd("SET")
        .arg(&cache_key)
        .arg(&cache_val)
        .arg("EX")
        .arg(TEAM_BUDGET_CACHE_TTL)
        .query_async(conn)
        .await;

    config
}

/// Read the current spend from Redis for a given key pattern.
/// Public helper used by budget API endpoints to report current spend.
pub async fn get_redis_spend(client: &redis::Client, key: &str) -> Result<f64, UsageError> {
    let mut conn = client
        .get_multiplexed_async_connection()
        .await
        .map_err(|e| UsageError::Redis(e.to_string()))?;
    redis_get_f64(&mut conn, key)
        .await
        .map_err(UsageError::Redis)
}

/// Read a wallet's current-window (today / this-month) spend from the
/// `spend:{wallet}:{period}` Redis counters the budget path increments.
///
/// Single source of truth for the per-wallet daily/monthly key derivation,
/// shared by [`UsageTracker::get_summary`] and the `GET /v1/wallet/:address/stats`
/// HTTP handler so neither re-derives (or copy-pastes) the key format.
///
/// Redis is OPTIONAL (Architectural Rule #12): when `client` is `None` — or a
/// counter is missing or fails to decode — the corresponding value falls back
/// to `0.0` rather than erroring, matching the budget GET endpoint's
/// `.unwrap_or(0.0)`. These are display counters; the authoritative ledger is
/// Postgres `spend_logs`, so a `0.0` fallback under-reports a window at worst —
/// it never bills. Returns `(daily, monthly)` decimal-USDC.
pub async fn wallet_window_spend(client: Option<&redis::Client>, wallet: &str) -> (f64, f64) {
    let Some(client) = client else {
        return (0.0, 0.0);
    };
    let now = Utc::now();
    let day_key = format!("spend:{}:{}", wallet, now.format("%Y-%m-%d"));
    let month_key = format!("spend:{}:{}", wallet, now.format("%Y-%m"));
    (
        get_redis_spend(client, &day_key).await.unwrap_or(0.0),
        get_redis_spend(client, &month_key).await.unwrap_or(0.0),
    )
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
            hourly_limit_usdc: Some(10.0),
            daily_limit_usdc: Some(50.0),
            monthly_limit_usdc: Some(500.0),
            total_spent_usdc: 12.50,
            created_at: Utc::now(),
        };

        let json = serde_json::to_string(&budget).expect("should serialize");
        let deserialized: WalletBudget = serde_json::from_str(&json).expect("should deserialize");

        assert_eq!(deserialized.wallet_address, budget.wallet_address);
        assert_eq!(deserialized.hourly_limit_usdc, Some(10.0));
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
            estimated_cost_usdc: None,
        });
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

    #[test]
    fn test_budget_config_default_has_100_daily() {
        let config = BudgetConfig::default();
        assert_eq!(config.daily, Some(100.0));
        assert!(config.hourly.is_none());
        assert!(config.monthly.is_none());
    }

    #[test]
    fn test_budget_config_serialization_roundtrip() {
        let config = BudgetConfig {
            hourly: Some(10.0),
            daily: Some(100.0),
            monthly: None,
        };
        let json = serde_json::to_string(&config).expect("should serialize");
        let deserialized: BudgetConfig = serde_json::from_str(&json).expect("should deserialize");
        assert_eq!(deserialized.hourly, Some(10.0));
        assert_eq!(deserialized.daily, Some(100.0));
        assert!(deserialized.monthly.is_none());
    }

    #[test]
    fn test_budget_config_cache_key_format() {
        let wallet = "So11111111111111111111111111111111111111112";
        let key = format!("budget_config:{wallet}");
        assert_eq!(
            key,
            "budget_config:So11111111111111111111111111111111111111112"
        );
    }

    #[test]
    fn test_team_member_cache_key_format() {
        let wallet = "WalletABC";
        let key = format!("team_member:{wallet}");
        assert_eq!(key, "team_member:WalletABC");
    }

    #[test]
    fn test_team_budget_config_serialization_roundtrip() {
        let config = TeamBudgetConfig {
            hourly: Some(50.0),
            daily: Some(500.0),
            monthly: Some(5000.0),
        };
        let json = serde_json::to_string(&config).expect("should serialize");
        let deserialized: TeamBudgetConfig =
            serde_json::from_str(&json).expect("should deserialize");
        assert_eq!(deserialized.hourly, Some(50.0));
        assert_eq!(deserialized.daily, Some(500.0));
        assert_eq!(deserialized.monthly, Some(5000.0));
    }

    #[test]
    fn test_budget_exceeded_error_includes_fields() {
        let err = UsageError::BudgetExceeded {
            wallet: "wallet_xyz".to_string(),
            limit: 50.0,
            spent: 75.0,
        };
        let msg = err.to_string();
        assert!(
            msg.contains("wallet_xyz"),
            "error should include wallet address"
        );
        assert!(msg.contains("50"), "error should include limit");
        assert!(msg.contains("75"), "error should include spent amount");
    }

    #[test]
    fn test_hourly_spend_key_format() {
        // Verify the hourly key format used in log_spend and check_budget
        let wallet = "WalletABC";
        let now = chrono::NaiveDate::from_ymd_opt(2026, 4, 5)
            .expect("valid date")
            .and_hms_opt(14, 30, 0)
            .expect("valid time");
        let key = format!("spend:{}:{}", wallet, now.format("%Y-%m-%dT%H"));
        assert_eq!(key, "spend:WalletABC:2026-04-05T14");
    }

    #[test]
    fn test_team_spend_key_format() {
        let team_id = "550e8400-e29b-41d4-a716-446655440000";
        let now = chrono::NaiveDate::from_ymd_opt(2026, 4, 5)
            .expect("valid date")
            .and_hms_opt(14, 30, 0)
            .expect("valid time");
        let hourly = format!("team_spend:{}:{}", team_id, now.format("%Y-%m-%dT%H"));
        assert_eq!(
            hourly,
            "team_spend:550e8400-e29b-41d4-a716-446655440000:2026-04-05T14"
        );
        let daily = format!("team_spend:{}:{}", team_id, now.format("%Y-%m-%d"));
        assert_eq!(
            daily,
            "team_spend:550e8400-e29b-41d4-a716-446655440000:2026-04-05"
        );
        let monthly = format!("team_spend:{}:{}", team_id, now.format("%Y-%m"));
        assert_eq!(
            monthly,
            "team_spend:550e8400-e29b-41d4-a716-446655440000:2026-04"
        );
    }

    /// Migration 007 SQL (loaded from file).
    const MIGRATION_007: &str = include_str!("../../../migrations/007_hourly_spend_limits.sql");

    #[test]
    fn test_migration_007_adds_hourly_limit_column() {
        assert!(
            MIGRATION_007.contains("ADD COLUMN IF NOT EXISTS hourly_limit_usdc"),
            "migration must add hourly_limit_usdc column to wallet_budgets"
        );
    }

    #[test]
    fn test_migration_007_creates_team_budgets_table() {
        assert!(
            MIGRATION_007.contains("CREATE TABLE IF NOT EXISTS team_budgets"),
            "migration must create team_budgets table"
        );
        assert!(
            MIGRATION_007.contains("REFERENCES teams(id) ON DELETE CASCADE"),
            "team_budgets.team_id must reference teams with cascade delete"
        );
        assert!(
            MIGRATION_007.contains("hourly_limit_usdc"),
            "team_budgets must have hourly_limit_usdc"
        );
        assert!(
            MIGRATION_007.contains("daily_limit_usdc"),
            "team_budgets must have daily_limit_usdc"
        );
        assert!(
            MIGRATION_007.contains("monthly_limit_usdc"),
            "team_budgets must have monthly_limit_usdc"
        );
    }

    #[test]
    fn test_migration_007_creates_updated_at_trigger() {
        assert!(
            MIGRATION_007.contains("trg_team_budgets_updated_at"),
            "migration must create updated_at trigger for team_budgets"
        );
        assert!(
            MIGRATION_007.contains("update_updated_at_column()"),
            "trigger must use the generic update_updated_at_column function"
        );
    }
}
