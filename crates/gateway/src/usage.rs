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

/// TTL for cached per-tenant budget config / provisioning lookups (seconds).
const TENANT_BUDGET_CACHE_TTL: u64 = 60;

// ---------------------------------------------------------------------------
// SECURITY BOUNDARY — per-tenant budgets are cooperative accounting, NOT isolation
// ---------------------------------------------------------------------------
//
// The `x-tenant` header that drives per-tenant budgets (see
// `tenant_enforcement_decision`, `check_budget`'s tenant bucket, and the
// `spend:{wallet}:{tenant}:{period}` counters) is UNAUTHENTICATED and FORGEABLE.
// Per-tenant budgets are **cooperative accounting under ONE trusted
// single-wallet proxy** (e.g. Telsi metering its own downstream customers) —
// they are **NOT a security boundary between mutually-distrusting tenants**.
// Anyone who controls the wallet's traffic can set any tenant tag, attributing
// spend to (or evading a cap under) an arbitrary tenant. The authoritative,
// non-forgeable budget is the wallet (and team) cap; the tenant bucket is a
// sub-allocation convenience for a cooperating proxy. Do not later mistake this
// for isolation.

/// Cached budget configuration for a wallet, stored in Redis as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetConfig {
    pub hourly: Option<f64>,
    pub daily: Option<f64>,
    pub monthly: Option<f64>,
    /// Whether this wallet is configured `require_tenant = TRUE` in
    /// `wallet_budgets` — i.e. it may only spend under a tagged, provisioned
    /// tenant. Read from the SAME `wallet_budgets` row as the limits so the
    /// tenant gate needs no extra Redis/DB round-trip (N2).
    ///
    /// `#[serde(default)]` so cached JSON written before this field existed
    /// (pre-N2) still deserializes — the absent field defaults to `false`
    /// (unenforced), which is the safe, backward-compatible value.
    #[serde(default)]
    pub require_tenant: bool,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            hourly: None,
            daily: Some(DEFAULT_DAILY_LIMIT_USDC),
            monthly: None,
            require_tenant: false,
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

/// Cached per-tenant budget configuration, stored in Redis as JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TenantBudgetConfig {
    pub hourly: Option<f64>,
    pub daily: Option<f64>,
    pub monthly: Option<f64>,
}

/// Result of looking up a `(wallet, tenant)` `tenant_budgets` row.
///
/// Distinguishes a CONFIRMED-ABSENT row from a transient DB ERROR (N3). The two
/// must be surfaced differently for an enforced wallet: a confirmed absence is a
/// genuine "not provisioned" config issue (`TenantNotProvisioned`), whereas a DB
/// error is a transient infrastructure blip that should map to
/// `UsageError::Database` so operators don't chase a phantom provisioning
/// problem. Both still deny the request for an enforced wallet (fail-closed) —
/// only the surfaced error variant differs.
#[derive(Debug, Clone)]
enum TenantLookup {
    /// A provisioned `tenant_budgets` row exists for `(wallet, tenant)`.
    Found(TenantBudgetConfig),
    /// The query SUCCEEDED and confirmed no row exists.
    Absent,
    /// The query ERRORED (transient DB problem) — provisioning state unknown.
    DbError,
}

/// Result of looking up a team's `team_budgets` row.
///
/// Distinguishes a CONFIRMED-ABSENT row from a transient DB ERROR (#501),
/// exactly as [`TenantLookup`] does for the per-tenant path. The two MUST be
/// surfaced differently: a confirmed absence means the team simply has no
/// budget configured (skip team enforcement — the legacy behavior), whereas a
/// DB error is a transient infrastructure blip whose answer is unknown.
///
/// Before #501 `get_team_budget_config` collapsed a DB error into a plain
/// `None` (indistinguishable from absence) AND cached that error-derived "none"
/// sentinel for the full TTL — silently disabling the team cap for every member
/// of the team for up to a minute (aggregate spend across N wallets escaped the
/// team bound). The caller now maps a `DbError` to `UsageError::Database` and
/// denies the request (fail-closed), and only `Found`/`Absent` are ever cached.
#[derive(Debug, Clone)]
enum TeamLookup {
    /// A `team_budgets` row exists for the team.
    Found(TeamBudgetConfig),
    /// The query SUCCEEDED and confirmed no row exists (no team budget).
    Absent,
    /// The query ERRORED (transient DB problem) — budget state unknown.
    DbError,
}

impl TeamLookup {
    /// The Redis cache value to write for this lookup, or `None` to write
    /// nothing. Mirrors the tenant path: a `Found` row caches its serialized
    /// JSON, a confirmed `Absent` caches the `"none"` sentinel, and a transient
    /// `DbError` caches NOTHING (so a blip can't poison the team cap for a full
    /// TTL — the next request re-attempts the DB read).
    fn cache_value(&self) -> Option<String> {
        match self {
            TeamLookup::Found(cfg) => {
                Some(serde_json::to_string(cfg).unwrap_or_else(|_| "none".to_string()))
            }
            TeamLookup::Absent => Some("none".to_string()),
            TeamLookup::DbError => None,
        }
    }
}

/// Outcome of the per-tenant enforcement decision matrix.
///
/// Pure function of three booleans so the money-path policy can be unit-tested
/// exhaustively with no Redis/DB. See [`tenant_enforcement_decision`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TenantDecision {
    /// A provisioned `(wallet, tenant)` budget row exists for the supplied tag —
    /// enforce its hourly/daily/monthly bucket atomically.
    Enforce,
    /// The wallet has `require_tenant = TRUE` but the request carried no tenant
    /// tag — reject (fail-closed) before any provider call/settlement.
    RejectRequired,
    /// The wallet has `require_tenant = TRUE`, a tag was supplied, but no
    /// `tenant_budgets` row exists for it — reject (provision-first).
    RejectNotProvisioned,
    /// No per-tenant enforcement applies (wallet/team enforcement only). This is
    /// the path every existing wallet takes today.
    Skip,
}

/// Per-tenant budget enforcement decision matrix (pure — the money-path
/// contract). Given:
///
/// - `require_tenant`: the wallet's `wallet_budgets.require_tenant` flag,
/// - `tag`: the validated `x-tenant` value (`None` if untagged),
/// - `has_row`: whether a `tenant_budgets` row exists for `(wallet, tag)`,
///
/// returns what the tenant bucket should do. A provisioned tenant budget is
/// always enforced when its tag is present (`Enforce`), regardless of
/// `require_tenant`. When `require_tenant` is set, an untagged request is
/// rejected (`RejectRequired`) and a tagged-but-unprovisioned request is
/// rejected (`RejectNotProvisioned`). Otherwise there is no tenant enforcement
/// (`Skip`) — byte-for-byte the legacy wallet/team-only path.
pub fn tenant_enforcement_decision(
    require_tenant: bool,
    tag: Option<&str>,
    has_row: bool,
) -> TenantDecision {
    match (require_tenant, tag, has_row) {
        // A provisioned tenant budget is enforced whenever its tag is present,
        // independent of require_tenant.
        (_, Some(_), true) => TenantDecision::Enforce,
        // Enforced wallet, no tag → fail-closed.
        (true, None, _) => TenantDecision::RejectRequired,
        // Enforced wallet, tag present but no provisioned row → provision-first.
        (true, Some(_), false) => TenantDecision::RejectNotProvisioned,
        // Unenforced wallet: no tag, or tag with no row → no tenant enforcement.
        (false, _, _) => TenantDecision::Skip,
    }
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
    /// Optional per-tenant attribution tag from the `x-tenant` header.
    ///
    /// Attribution only — recorded on the spend row for reporting; it does not
    /// gate or change billing. See `validate_tenant` for the accepted charset.
    pub tenant: Option<String>,
    /// Whether `check_budget` actually ENFORCED a per-tenant bucket for this
    /// request (i.e. the decision was `Enforce` — a provisioned `(wallet,
    /// tenant)` row exists). Thread this from
    /// [`BudgetReservation::tenant_enforced`]. `log_spend` reconciles the
    /// `spend:{wallet}:{tenant}:{period}` counters ONLY when this is `true`,
    /// so a tagged-but-unenforced (`Skip`-path) request does not accumulate
    /// per-tenant spend that a later-provisioned budget would mis-read as
    /// already-consumed. The per-tenant `tenant` field above is independent and
    /// still recorded on the Postgres spend row for ATTRIBUTION regardless.
    pub tenant_enforced: bool,
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

    #[error(
        "wallet {wallet} requires an x-tenant tag: this wallet is configured to \
         spend only under a provisioned tenant; supply a valid x-tenant header"
    )]
    TenantRequired { wallet: String },

    #[error(
        "tenant '{tenant}' is not provisioned for wallet {wallet}: this wallet \
         may only spend under a known, provisioned tenant"
    )]
    TenantNotProvisioned { wallet: String, tenant: String },
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
                    r#"INSERT INTO spend_logs (id, wallet_address, model, provider, input_tokens, output_tokens, cost_usdc, tx_signature, request_id, session_id, tenant, created_at)
                       VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)"#,
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
                .bind(&db_entry.tenant)
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
            // Reconcile the per-tenant counters ONLY when check_budget actually
            // enforced a provisioned tenant bucket (decision == Enforce). On the
            // pre-provisioning Skip path `tenant_enforced` is false even if a tag
            // is present, so we do not accumulate per-tenant spend that a
            // later-provisioned budget would mis-read as already-consumed.
            let tenant = if entry.tenant_enforced {
                entry.tenant
            } else {
                None
            };
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

                // Per-tenant counters: settle the same `cost` delta on the
                // `spend:{wallet}:{tenant}:{period}` keys that `check_budget`'s
                // `Enforce` arm reserved against. Mirrors the wallet/team
                // reconciliation. `tenant` is `Some` here ONLY when
                // `entry.tenant_enforced` was true (a provisioned bucket was
                // enforced) — see the gating above. On the Skip path we
                // deliberately write nothing so pre-provisioning spend does not
                // accumulate. The header is forgeable — see the module-level
                // security note.
                if let Some(tag) = tenant.as_deref() {
                    let tenant_hour_key =
                        format!("spend:{}:{}:{}", wallet, tag, now.format("%Y-%m-%dT%H"));
                    incr_and_expire(&mut conn, &tenant_hour_key, cost, 7200).await;

                    let tenant_day_key =
                        format!("spend:{}:{}:{}", wallet, tag, now.format("%Y-%m-%d"));
                    incr_and_expire(&mut conn, &tenant_day_key, cost, 86400).await;

                    let tenant_month_key =
                        format!("spend:{}:{}:{}", wallet, tag, now.format("%Y-%m"));
                    incr_and_expire(&mut conn, &tenant_month_key, cost, 86400 * 31).await;
                }

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
        tenant: Option<&str>,
    ) -> Result<BudgetReservation, UsageError> {
        // No Redis configured — apply a conservative per-request hard cap.
        // No counters are committed, so the reservation is empty (release is a no-op).
        //
        // NOTE: the per-tenant fail-closed gates (TenantRequired /
        // TenantNotProvisioned) are NOT applied on this no-Redis branch. They
        // depend on reading `wallet_budgets.require_tenant` and `tenant_budgets`
        // from the DB, which is read via the Redis-cached helpers below. With no
        // Redis the operator is already in degraded single-cap mode (Rule #12);
        // tenant enforcement is a feature of the Redis/DB-backed path. This keeps
        // the no-Redis path byte-for-byte unchanged from before PR2.
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
                // #501: a DB error reading `team_budgets` must NOT be collapsed
                // into "no team budget" (fail-open). `get_team_budget_config`
                // now returns a `TeamLookup` that distinguishes a configured
                // budget, a confirmed absence, and a transient DB error.
                match get_team_budget_config(&mut conn, self.db_pool.as_ref(), tid).await {
                    TeamLookup::Found(team_cfg) => {
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
                    TeamLookup::Absent => {
                        // Team has no budget configured — nothing to enforce.
                    }
                    TeamLookup::DbError => {
                        // Fail-closed: a transient DB error reading the team cap
                        // must deny (not silently skip team enforcement), and
                        // must release the wallet/tenant counters already
                        // committed above so a denied request leaks no budget.
                        rollback_committed(&mut conn, &committed).await;
                        warn!(
                            wallet = %wallet_address,
                            team_id = %tid,
                            "team_budget_db_error: team_budgets read failed; \
                             denying request (fail-closed) as a transient DB error"
                        );
                        return Err(UsageError::Database(
                            "team_budgets lookup failed".to_string(),
                        ));
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

        // --- Per-tenant budget enforcement ---
        //
        // SECURITY: see the module-level "cooperative accounting, NOT isolation"
        // note — `tenant` comes from the forgeable `x-tenant` header.
        //
        // Decision matrix (pure, see `tenant_enforcement_decision`):
        //   * provisioned `(wallet, tenant)` row present  → enforce the bucket
        //     (whether or not require_tenant is set).
        //   * require_tenant=TRUE, no tag                 → RejectRequired.
        //   * require_tenant=TRUE, tag but no row         → RejectNotProvisioned.
        //   * otherwise                                   → Skip (wallet/team only).
        //
        // Round-trip cost (N2): `require_tenant` rides on the `BudgetConfig` read
        // above (`get_wallet_budget_config`) — the same `wallet_budgets` row read
        // it already performs — so there is NO extra Redis/DB round-trip for the
        // no-tenant path. A tagged request additionally reads the
        // `tenant_budgets` row (cached 60s); an untagged request reads nothing
        // more here.
        let require_tenant = config.require_tenant;

        // The tenant_budgets row is read ONLY when a tag is present. The lookup
        // distinguishes a confirmed-absent row from a transient DB error (N3):
        // for an enforced wallet a DB error surfaces as `UsageError::Database`
        // (transient infra) rather than `TenantNotProvisioned` (a config issue),
        // while still denying the request.
        let tenant_lookup = match tenant {
            None => TenantLookup::Absent,
            Some(tag) => {
                get_tenant_budget_config(&mut conn, self.db_pool.as_ref(), wallet_address, tag)
                    .await
            }
        };

        // N3: a DB error reading `tenant_budgets` for an ENFORCED wallet is
        // fail-closed (deny) but surfaced as a transient `Database` error, not
        // `TenantNotProvisioned`. For an UNENFORCED wallet a DB error simply
        // skips tenant enforcement (the wallet/team cap is the authoritative
        // backstop) — handled below via `has_row = false` → `Skip`.
        if matches!(tenant_lookup, TenantLookup::DbError) && require_tenant {
            rollback_committed(&mut conn, &committed).await;
            warn!(
                wallet = %wallet_address,
                "tenant_lookup_db_error: enforced wallet, tenant_budgets read failed; \
                 denying request (fail-closed) as a transient DB error"
            );
            return Err(UsageError::Database(
                "tenant_budgets lookup failed".to_string(),
            ));
        }

        // For the decision matrix, a `Found` row means has_row=true; `Absent` and
        // (for an unenforced wallet) `DbError` both mean has_row=false → Skip.
        let tenant_config: Option<TenantBudgetConfig> = match tenant_lookup {
            TenantLookup::Found(cfg) => Some(cfg),
            TenantLookup::Absent | TenantLookup::DbError => None,
        };

        // Set true only when a provisioned tenant bucket was enforced AND at
        // least one window counter was actually committed (N4). Threaded out via
        // `BudgetReservation::tenant_enforced` so `log_spend` reconciles the
        // per-tenant counters ONLY for windows that were actually reserved — a
        // limitless provisioned row commits nothing, so it must NOT report
        // enforced (otherwise log_spend would write counters check_budget never
        // reserved).
        let mut tenant_enforced = false;

        match tenant_enforcement_decision(require_tenant, tenant, tenant_config.is_some()) {
            TenantDecision::Skip => {}
            TenantDecision::RejectRequired => {
                // Fail-closed BEFORE settlement: nothing has been broadcast yet,
                // but earlier wallet/team windows were reserved — release them so
                // a rejected request leaks no budget.
                rollback_committed(&mut conn, &committed).await;
                warn!(
                    wallet = %wallet_address,
                    "tenant_required: wallet requires an x-tenant tag; denying untagged request"
                );
                return Err(UsageError::TenantRequired {
                    wallet: wallet_address.to_string(),
                });
            }
            TenantDecision::RejectNotProvisioned => {
                rollback_committed(&mut conn, &committed).await;
                // `RejectNotProvisioned` is only reached when `tag` is `Some`
                // (the matrix arm is `(true, Some(_), false)`). Surface a
                // mis-keyed matrix as an immediate panic rather than a silent
                // `<none>`/empty-string mis-attribution.
                let tag = tenant.expect(
                    "RejectNotProvisioned implies Some(tag) per tenant_enforcement_decision",
                );
                warn!(
                    wallet = %wallet_address,
                    tenant = %tag,
                    "tenant_not_provisioned: enforced wallet, unknown tenant; denying request"
                );
                return Err(UsageError::TenantNotProvisioned {
                    wallet: wallet_address.to_string(),
                    tenant: tag.to_string(),
                });
            }
            TenantDecision::Enforce => {
                // `Enforce` is only returned when both `tenant` is `Some` and a
                // provisioned row exists. Using `.expect`/`unreachable!` (instead
                // of `unwrap_or_default()` / an all-None default) turns a future
                // matrix mistake into an immediate panic surfaced in tests rather
                // than a silent counter mis-key (`spend:{wallet}::{period}`) or a
                // silent no-op. Counters key `spend:{wallet}:{tenant}:{period}`.
                let tag =
                    tenant.expect("Enforce implies Some(tag) per tenant_enforcement_decision");
                let tcfg = tenant_config.unwrap_or_else(|| {
                    unreachable!("Enforce implies has_row=true (a provisioned tenant_budgets row)")
                });

                // A provisioned row with NO hourly/daily/monthly limit set means
                // enforcement is a silent no-op (no `try_commit!` fires). Surface
                // it for observability.
                let any_limit =
                    tcfg.hourly.is_some() || tcfg.daily.is_some() || tcfg.monthly.is_some();
                if !any_limit {
                    warn!(
                        wallet = %wallet_address,
                        tenant = %tag,
                        "tenant_budget_no_limits: provisioned tenant_budgets row has no \
                         hourly/daily/monthly limit set — enforcement is a silent no-op"
                    );
                }

                // N4: report enforced ONLY when at least one window counter was
                // actually committed below (i.e. at least one limit is Some). A
                // limitless provisioned row reserves zero counters, so it must NOT
                // report enforced — otherwise `log_spend` would write per-tenant
                // counters that `check_budget` never reserved, breaking the
                // reserve/settle symmetry.
                tenant_enforced = any_limit;

                if let Some(hourly_limit) = tcfg.hourly {
                    let _ = try_commit!(
                        format!(
                            "spend:{}:{}:{}",
                            wallet_address,
                            tag,
                            now.format("%Y-%m-%dT%H")
                        ),
                        estimated_cost_usdc,
                        hourly_limit,
                        7200
                    );
                }
                if let Some(daily_limit) = tcfg.daily {
                    let _ = try_commit!(
                        format!(
                            "spend:{}:{}:{}",
                            wallet_address,
                            tag,
                            now.format("%Y-%m-%d")
                        ),
                        estimated_cost_usdc,
                        daily_limit,
                        86400
                    );
                }
                if let Some(monthly_limit) = tcfg.monthly {
                    let _ = try_commit!(
                        format!("spend:{}:{}:{}", wallet_address, tag, now.format("%Y-%m")),
                        estimated_cost_usdc,
                        monthly_limit,
                        86400 * 31
                    );
                }
            }
        }

        Ok(BudgetReservation {
            committed,
            tenant_enforced,
        })
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

    /// Read the wallet's `require_tenant` flag from the SAME source
    /// [`check_budget`] uses (`get_wallet_budget_config` → `wallet_budgets` row,
    /// cached in Redis), so the answer cannot drift from the value the chat
    /// route's tenant gate enforces.
    ///
    /// Used by the paid paths that do NOT run the full `check_budget` tenant
    /// matrix — the A2A handler and the service-marketplace proxy (issue #499) —
    /// to REJECT (fail-closed) a request from a `require_tenant = TRUE` wallet
    /// before any settlement or provider call. Those paths deliberately do not
    /// implement per-tenant metering; rejecting is the documented fix.
    ///
    /// **Degradation mirrors [`check_budget`] exactly** so the two paths agree:
    /// - No Redis configured → returns `false` (do NOT reject). `check_budget`'s
    ///   no-Redis branch likewise skips the tenant gates (degraded single-cap
    ///   mode, Rule #12); enforcing here would diverge from the chat path.
    /// - On a Redis connection failure, or a DB error reading `wallet_budgets`,
    ///   `get_wallet_budget_config` returns the restrictive fallback whose
    ///   `require_tenant` is `false` (it fails OPEN on the gate, with the
    ///   restrictive hourly/daily/monthly caps as the non-forgeable backstop —
    ///   see `restrictive_budget_fallback`). We surface that same `false` here,
    ///   so a transient blip does not reject all traffic on these paths either.
    ///
    /// [`check_budget`]: Self::check_budget
    pub async fn require_tenant_for_wallet(&self, wallet_address: &str) -> bool {
        // No Redis → mirror check_budget's no-Redis branch: tenant gates are a
        // feature of the Redis/DB-backed path; return false (don't reject).
        let Some(client) = &self.redis_client else {
            return false;
        };

        let mut conn = match client.get_multiplexed_async_connection().await {
            Ok(c) => c,
            Err(e) => {
                // Fail OPEN for the gate on a Redis connection failure, matching
                // the restrictive-fallback policy in get_wallet_budget_config:
                // the wallet/team caps are the authoritative backstop, and
                // rejecting all traffic on a transient blip is worse. (The chat
                // path's check_budget denies on Redis failure via its own budget
                // counters; here we have no counter to consult, so we mirror the
                // get_wallet_budget_config fallback's require_tenant=false.)
                warn!(
                    wallet = %wallet_address,
                    error = %e,
                    "require_tenant_for_wallet: Redis connection failed; treating as \
                     require_tenant=false (fail-open gate, wallet caps remain the backstop)"
                );
                return false;
            }
        };

        let config =
            get_wallet_budget_config(&mut conn, self.db_pool.as_ref(), wallet_address).await;
        config.require_tenant
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
    /// Whether `check_budget` actually enforced a per-tenant bucket for this
    /// request (i.e. the decision was `Enforce` — a provisioned `(wallet,
    /// tenant)` row exists). When `false`, `log_spend` must NOT reconcile the
    /// `spend:{wallet}:{tenant}:{period}` counters: writing them on the
    /// pre-provisioning `Skip` path would accumulate tagged spend that a
    /// later-provisioned budget would then read as already-consumed, causing a
    /// spurious `BudgetExceeded` mid-window. Per-tenant ATTRIBUTION reporting
    /// reads Postgres `spend_logs`, not these Redis counters, so gating the
    /// counter writes here loses nothing for reporting.
    tenant_enforced: bool,
}

impl BudgetReservation {
    /// Whether a per-tenant budget bucket was actually enforced for the request
    /// that produced this reservation. The chat handler threads this into
    /// `SpendLogEntry.tenant_enforced` so `log_spend` only reconciles the
    /// per-tenant counters when enforcement was real.
    pub fn tenant_enforced(&self) -> bool {
        self.tenant_enforced
    }
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
        // Fail OPEN for the tenant gate on a wallet-config DB error: the
        // restrictive hourly/daily/monthly caps above are the authoritative,
        // non-forgeable backstop. Setting this `true` on a transient blip would
        // reject ALL untagged traffic gateway-wide. The error path that returns
        // this value MUST NOT cache it (see `get_wallet_budget_config`), so the
        // next request re-attempts the DB read rather than serving a poisoned
        // `require_tenant=false` for a full TTL.
        require_tenant: false,
    }
}

/// Load per-wallet budget config. Checks Redis cache first (`budget_config:{wallet}`),
/// falls back to DB query, caches result in Redis with 60s TTL.
/// Returns default config ($100/day) if no row exists; restrictive fallback on DB error.
///
/// Reads the wallet's `require_tenant` flag from the SAME `wallet_budgets` row,
/// surfaced on [`BudgetConfig::require_tenant`]. The tenant gate consumes it from
/// here, so the no-tenant path adds NO extra Redis/DB round-trip relative to
/// pre-PR2 (N2 — the previous separate `tenant_require:{wallet}` lookup was
/// removed). On a DB error the restrictive fallback (with `require_tenant=false`)
/// is returned but NOT cached, so a transient blip cannot poison the cache for a
/// full TTL.
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

    // Cache miss — query DB. `persist` is true only when the query SUCCEEDED
    // (a real row, a confirmed absence, or no DB pool). On a DB error we return
    // the restrictive fallback WITHOUT caching, so a transient error cannot
    // poison the cache (including its `require_tenant=false`) for a full TTL —
    // the next request re-attempts the DB read. Mirrors the `persist` pattern in
    // `get_tenant_enforcement` / `get_tenant_budget_config`.
    let (config, persist) = if let Some(pool) = db_pool {
        match sqlx::query_as::<_, (Option<f64>, Option<f64>, Option<f64>, bool)>(
            r#"SELECT
                hourly_limit_usdc::DOUBLE PRECISION,
                daily_limit_usdc::DOUBLE PRECISION,
                monthly_limit_usdc::DOUBLE PRECISION,
                require_tenant
            FROM wallet_budgets
            WHERE wallet_address = $1"#,
        )
        .bind(wallet)
        .fetch_optional(pool)
        .await
        {
            Ok(Some((hourly, daily, monthly, require_tenant))) => (
                BudgetConfig {
                    hourly,
                    daily: daily.or(Some(DEFAULT_DAILY_LIMIT_USDC)),
                    monthly,
                    require_tenant,
                },
                true,
            ),
            // No wallet_budgets row → default config, require_tenant=false
            // (matches the column's DEFAULT FALSE for unconfigured wallets).
            Ok(None) => (BudgetConfig::default(), true),
            Err(e) => {
                warn!(wallet = %wallet, error = %e, "failed to query wallet_budgets — using restrictive fallback (not caching)");
                (restrictive_budget_fallback(), false)
            }
        }
    } else {
        (BudgetConfig::default(), true)
    };

    // Cache in Redis (best-effort) — only when the value is authoritative (a
    // successful read), never an error-derived restrictive fallback.
    if persist {
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
///
/// Returns a [`TeamLookup`] that distinguishes a configured budget
/// ([`TeamLookup::Found`]) from a confirmed absence ([`TeamLookup::Absent`]) and
/// a transient DB error ([`TeamLookup::DbError`]) — #501. The caller maps a
/// `DbError` to `UsageError::Database` and denies the request (fail-closed)
/// rather than silently skipping the team cap. Only `Found`/`Absent` are cached;
/// a `DbError` is NEVER cached, so a transient error cannot poison the "none"
/// sentinel for a full TTL — the next request re-attempts the DB read.
///
/// Mirrors the `persist`/no-cache-on-error pattern of `get_wallet_budget_config`
/// and the `TenantLookup` shape of `get_tenant_budget_config`, including the
/// corrupt-cache `DEL` (a bad entry is deleted before falling through to DB).
async fn get_team_budget_config(
    conn: &mut redis::aio::MultiplexedConnection,
    db_pool: Option<&sqlx::PgPool>,
    team_id: Uuid,
) -> TeamLookup {
    let cache_key = format!("team_budget:{team_id}");

    // Try Redis cache
    if let Ok(Some(json_str)) = redis::cmd("GET")
        .arg(&cache_key)
        .query_async::<Option<String>>(conn)
        .await
    {
        if json_str == "none" {
            return TeamLookup::Absent;
        }
        match serde_json::from_str::<TeamBudgetConfig>(&json_str) {
            Ok(config) => return TeamLookup::Found(config),
            Err(e) => {
                // Corrupt cache entry — DEL it before falling through to DB so
                // it can't be re-read on the next request (mirrors the wallet
                // path). Do NOT serve it as an absence.
                tracing::warn!(cache_key = %cache_key, error = %e, "corrupted team budget cache entry, falling through to DB");
                let _ = redis::cmd("DEL")
                    .arg(&cache_key)
                    .query_async::<()>(conn)
                    .await;
            }
        }
    }

    // Cache miss — query DB. A DB error returns `DbError` WITHOUT caching, so a
    // transient error cannot poison the "none" sentinel for a full TTL. Only a
    // successful read (`Found`/`Absent`) is cached.
    let lookup = if let Some(pool) = db_pool {
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
            Ok(Some((hourly, daily, monthly))) => TeamLookup::Found(TeamBudgetConfig {
                hourly,
                daily,
                monthly,
            }),
            Ok(None) => TeamLookup::Absent,
            Err(e) => {
                warn!(team_id = %team_id, error = %e, "failed to query team_budgets — not caching");
                TeamLookup::DbError
            }
        }
    } else {
        // No DB pool — a stable "no team budget" answer; safe to cache.
        TeamLookup::Absent
    };

    // Cache result — only `Found`/`Absent`; never a transient error-derived value.
    if let Some(cache_val) = lookup.cache_value() {
        let _: Result<(), _> = redis::cmd("SET")
            .arg(&cache_key)
            .arg(&cache_val)
            .arg("EX")
            .arg(TEAM_BUDGET_CACHE_TTL)
            .query_async(conn)
            .await;
    }

    lookup
}

/// Load the `(wallet, tenant)` budget config from `tenant_budgets`. Cached in
/// Redis with 60s TTL (`tenant_budget:{wallet}:{tenant}`), with a "none"
/// sentinel for "no provisioned row" to avoid repeated DB misses.
///
/// Returns a [`TenantLookup`] that distinguishes a provisioned row
/// ([`TenantLookup::Found`]) from a confirmed absence ([`TenantLookup::Absent`])
/// and a transient DB error ([`TenantLookup::DbError`]) — N3. The caller maps a
/// `DbError` on an enforced wallet to `UsageError::Database` (transient infra)
/// rather than `TenantNotProvisioned` (a config issue), while still denying the
/// request. Only `Found`/`Absent` are cached; a `DbError` is never cached, so a
/// transient error cannot poison the "none" sentinel for a full TTL — the next
/// request re-attempts the DB read.
///
/// `require_tenant` is NOT read here: it now rides on
/// [`BudgetConfig::require_tenant`] from the wallet-config read that
/// `check_budget` already performs (N2), so the no-tenant path adds no extra
/// round-trip.
async fn get_tenant_budget_config(
    conn: &mut redis::aio::MultiplexedConnection,
    db_pool: Option<&sqlx::PgPool>,
    wallet: &str,
    tenant: &str,
) -> TenantLookup {
    let cache_key = format!("tenant_budget:{wallet}:{tenant}");

    if let Ok(Some(json_str)) = redis::cmd("GET")
        .arg(&cache_key)
        .query_async::<Option<String>>(conn)
        .await
    {
        if json_str == "none" {
            return TenantLookup::Absent;
        }
        if let Ok(config) = serde_json::from_str::<TenantBudgetConfig>(&json_str) {
            return TenantLookup::Found(config);
        }
    }

    // A DB error returns `DbError` WITHOUT caching, so a transient error cannot
    // poison the "none" sentinel for a full TTL — the next request re-attempts
    // the DB read. Only a successful read (`Found`/`Absent`) is cached.
    let lookup = if let Some(pool) = db_pool {
        match sqlx::query_as::<_, (Option<f64>, Option<f64>, Option<f64>)>(
            r#"SELECT
                hourly_limit_usdc::DOUBLE PRECISION,
                daily_limit_usdc::DOUBLE PRECISION,
                monthly_limit_usdc::DOUBLE PRECISION
            FROM tenant_budgets
            WHERE wallet_address = $1 AND tenant = $2"#,
        )
        .bind(wallet)
        .bind(tenant)
        .fetch_optional(pool)
        .await
        {
            Ok(Some((hourly, daily, monthly))) => TenantLookup::Found(TenantBudgetConfig {
                hourly,
                daily,
                monthly,
            }),
            Ok(None) => TenantLookup::Absent,
            Err(e) => {
                warn!(wallet = %wallet, tenant = %tenant, error = %e, "failed to query tenant_budgets — not caching");
                TenantLookup::DbError
            }
        }
    } else {
        // No DB pool — a stable "no provisioned row" answer; safe to cache.
        TenantLookup::Absent
    };

    let cache_val = match &lookup {
        TenantLookup::Found(cfg) => {
            Some(serde_json::to_string(cfg).unwrap_or_else(|_| "none".to_string()))
        }
        TenantLookup::Absent => Some("none".to_string()),
        // Never cache a transient error-derived value.
        TenantLookup::DbError => None,
    };
    if let Some(cache_val) = cache_val {
        let _: Result<(), _> = redis::cmd("SET")
            .arg(&cache_key)
            .arg(&cache_val)
            .arg("EX")
            .arg(TENANT_BUDGET_CACHE_TTL)
            .query_async(conn)
            .await;
    }

    lookup
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

/// Per-tenant row returned by [`get_stats_by_tenant`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatsTenantRow {
    pub tenant: String,
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

/// Fetch per-tenant spend breakdown for a wallet over the given number of days.
///
/// Rows with a NULL `tenant` (requests that carried no `x-tenant` tag) are
/// excluded — this is the attribution breakdown for tagged traffic only.
pub async fn get_stats_by_tenant(
    pool: &sqlx::PgPool,
    wallet: &str,
    days: i32,
) -> Result<Vec<StatsTenantRow>, sqlx::Error> {
    let rows: Vec<(String, i64, f64, i64, i64)> = sqlx::query_as(
        r#"SELECT tenant, COUNT(*) as requests,
                  COALESCE(SUM(cost_usdc), 0)::DOUBLE PRECISION as cost,
                  COALESCE(SUM(input_tokens), 0) as input_tokens,
                  COALESCE(SUM(output_tokens), 0) as output_tokens
           FROM spend_logs
           WHERE wallet_address = $1
             AND tenant IS NOT NULL
             AND created_at >= NOW() - make_interval(days => $2)
           GROUP BY tenant ORDER BY cost DESC"#,
    )
    .bind(wallet)
    .bind(days)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(tenant, requests, cost, input_tokens, output_tokens)| StatsTenantRow {
                tenant,
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
            tenant: None,
            tenant_enforced: false,
            estimated_cost_usdc: None,
        });
    }

    /// Issue #499 degradation pin: with no Redis configured,
    /// `require_tenant_for_wallet` must return `false` (do NOT reject),
    /// mirroring `check_budget`'s no-Redis branch which skips the tenant gates
    /// entirely (degraded single-cap mode, Rule #12). If this ever flipped to
    /// `true`, the A2A/proxy paths would reject ALL traffic whenever Redis is
    /// down — a divergence from the chat path.
    #[tokio::test]
    async fn test_require_tenant_for_wallet_noop_returns_false() {
        let tracker = UsageTracker::noop();
        assert!(
            !tracker
                .require_tenant_for_wallet("AnyWalletAddress11111111111111111111111111")
                .await,
            "no-Redis tracker must report require_tenant=false (do not reject), \
             mirroring check_budget's no-Redis branch"
        );
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
        let result = tracker.check_budget("wallet123", 1.0, None).await;
        assert!(result.is_ok(), "cost equal to cap should be allowed");
    }

    #[tokio::test]
    async fn test_noop_tracker_check_budget_rejects_above_cap() {
        // Without Redis, requests exceeding $1.00 must be rejected to prevent
        // runaway spend on high-cost models.
        let tracker = UsageTracker::noop();
        let result = tracker.check_budget("wallet123", 1.01, None).await;
        assert!(
            matches!(result, Err(UsageError::BudgetExceeded { .. })),
            "cost above cap should be rejected when Redis is unavailable"
        );
    }

    #[tokio::test]
    async fn test_noop_tracker_check_budget_passes_below_cap() {
        let tracker = UsageTracker::noop();
        let result = tracker.check_budget("wallet123", 0.50, None).await;
        assert!(result.is_ok(), "cost below cap should be allowed");
    }

    /// #486 second-pass: the no-charge route arms (`Internal` provider-call
    /// error, `AllProvidersFailed`, and the post-delivery `exact`
    /// settle-after-deliver-failed branches) reconcile the budget reservation by
    /// calling `release_reservation`. Without Redis the reservation is empty and
    /// release is a no-op — pin that it neither errors nor panics, so the arms
    /// can call it unconditionally on the failure path. (The Redis-backed
    /// rollback arithmetic is exercised under `#[sqlx::test]` with a live store.)
    #[tokio::test]
    async fn test_noop_tracker_release_reservation_is_safe_noop() {
        let tracker = UsageTracker::noop();
        // A within-cap check yields an (empty) reservation without Redis.
        let reservation = tracker
            .check_budget("wallet123", 0.50, None)
            .await
            .expect("within-cap check_budget must succeed without Redis");
        assert!(
            reservation.committed.is_empty(),
            "no-Redis reservation must commit no counters (release is then a no-op)"
        );
        // The no-Redis branch never enforces a tenant bucket, so log_spend must
        // not reconcile per-tenant counters for it — even when a tag is present.
        assert!(
            !reservation.tenant_enforced(),
            "no-Redis reservation must report tenant_enforced=false"
        );
        let tagged = tracker
            .check_budget("wallet123", 0.50, Some("acme"))
            .await
            .expect("within-cap tagged check_budget must succeed without Redis");
        assert!(
            !tagged.tenant_enforced(),
            "no-Redis branch must not enforce a tenant bucket even with a tag"
        );
        // A default reservation likewise reports no tenant enforcement.
        assert!(
            !BudgetReservation::default().tenant_enforced(),
            "default reservation must report tenant_enforced=false"
        );
        // Releasing it must be a safe no-op (the failure-path arms call this
        // unconditionally; it must never panic or block).
        tracker.release_reservation(&reservation).await;
        // Releasing a default/empty reservation directly is likewise safe.
        tracker
            .release_reservation(&BudgetReservation::default())
            .await;
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
        // N2: default wallet is unenforced.
        assert!(!config.require_tenant);
    }

    #[test]
    fn test_budget_config_serialization_roundtrip() {
        let config = BudgetConfig {
            hourly: Some(10.0),
            daily: Some(100.0),
            monthly: None,
            require_tenant: true,
        };
        let json = serde_json::to_string(&config).expect("should serialize");
        let deserialized: BudgetConfig = serde_json::from_str(&json).expect("should deserialize");
        assert_eq!(deserialized.hourly, Some(10.0));
        assert_eq!(deserialized.daily, Some(100.0));
        assert!(deserialized.monthly.is_none());
        assert!(deserialized.require_tenant);
    }

    /// N2 backward-compat: cached `BudgetConfig` JSON written BEFORE the
    /// `require_tenant` field existed (pre-N2) must still deserialize, with the
    /// absent field defaulting to `false` (unenforced) via `#[serde(default)]`.
    /// Without the attribute this would error and corrupt-fall-through to a DB
    /// read every request during a deploy that left old cache entries.
    #[test]
    fn test_budget_config_deserializes_legacy_json_without_require_tenant() {
        let legacy = r#"{"hourly":null,"daily":0.05,"monthly":null}"#;
        let cfg: BudgetConfig =
            serde_json::from_str(legacy).expect("legacy cached JSON must still deserialize");
        assert_eq!(cfg.daily, Some(0.05));
        assert!(
            !cfg.require_tenant,
            "absent require_tenant must default to false (unenforced)"
        );
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

    // -----------------------------------------------------------------------
    // PR2: per-tenant budget enforcement
    // -----------------------------------------------------------------------

    /// Exhaustive truth table for the money-path decision matrix. Every
    /// (require_tenant, tag_present, has_row) cell is pinned. This is the
    /// contract; it must stay byte-for-byte as specified in the PR brief.
    #[test]
    fn test_tenant_enforcement_decision_matrix_exhaustive() {
        use TenantDecision::*;
        // require_tenant = FALSE (default for every existing wallet)
        assert_eq!(
            tenant_enforcement_decision(false, None, false),
            Skip,
            "unenforced + untagged → Skip"
        );
        assert_eq!(
            tenant_enforcement_decision(false, None, true),
            Skip,
            "unenforced + untagged → Skip even if a (stale) row flag is set"
        );
        assert_eq!(
            tenant_enforcement_decision(false, Some("acme"), false),
            Skip,
            "unenforced + tagged + no row → Skip (wallet/team only)"
        );
        assert_eq!(
            tenant_enforcement_decision(false, Some("acme"), true),
            Enforce,
            "unenforced + tagged + provisioned row → Enforce"
        );

        // require_tenant = TRUE
        assert_eq!(
            tenant_enforcement_decision(true, None, false),
            RejectRequired,
            "enforced + untagged → RejectRequired"
        );
        assert_eq!(
            tenant_enforcement_decision(true, None, true),
            RejectRequired,
            "enforced + untagged → RejectRequired regardless of row flag"
        );
        assert_eq!(
            tenant_enforcement_decision(true, Some("ghost"), false),
            RejectNotProvisioned,
            "enforced + tagged + no row → RejectNotProvisioned"
        );
        assert_eq!(
            tenant_enforcement_decision(true, Some("acme"), true),
            Enforce,
            "enforced + tagged + provisioned row → Enforce"
        );
    }

    /// Backward-compat invariant at the policy layer: an unenforced wallet
    /// (require_tenant=FALSE) yields `Skip` whether or not a tag is present, as
    /// long as no provisioned row exists — identical to the pre-PR2
    /// wallet/team-only path. (The Redis-counter equivalent is proven in the
    /// integration suite.)
    #[test]
    fn test_tenant_decision_backward_compat_unenforced_wallet() {
        assert_eq!(
            tenant_enforcement_decision(false, None, false),
            tenant_enforcement_decision(false, Some("anything"), false),
            "unenforced wallet with no provisioned row must behave identically \
             tagged or untagged (both Skip)"
        );
    }

    #[test]
    fn test_tenant_error_messages() {
        let err = UsageError::TenantRequired {
            wallet: "WalletABC".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("WalletABC"), "must name the wallet");
        assert!(msg.contains("x-tenant"), "must mention the required header");

        let err = UsageError::TenantNotProvisioned {
            wallet: "WalletABC".to_string(),
            tenant: "ghost".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("WalletABC"), "must name the wallet");
        assert!(msg.contains("ghost"), "must name the unprovisioned tenant");
    }

    #[test]
    fn test_tenant_spend_key_format() {
        // The per-tenant counters key `spend:{wallet}:{tenant}:{period}`.
        let wallet = "WalletABC";
        let tenant = "acme";
        let now = chrono::NaiveDate::from_ymd_opt(2026, 4, 5)
            .expect("valid date")
            .and_hms_opt(14, 30, 0)
            .expect("valid time");
        assert_eq!(
            format!("spend:{}:{}:{}", wallet, tenant, now.format("%Y-%m-%dT%H")),
            "spend:WalletABC:acme:2026-04-05T14"
        );
        assert_eq!(
            format!("spend:{}:{}:{}", wallet, tenant, now.format("%Y-%m-%d")),
            "spend:WalletABC:acme:2026-04-05"
        );
        assert_eq!(
            format!("spend:{}:{}:{}", wallet, tenant, now.format("%Y-%m")),
            "spend:WalletABC:acme:2026-04"
        );
    }

    #[tokio::test]
    async fn test_noop_tracker_check_budget_ignores_tenant_without_redis() {
        // Backward-compat / no-Redis: the tenant fail-closed gates are NOT applied
        // on the no-Redis branch (they need the DB-backed flag). A within-cap
        // request passes whether tagged or untagged.
        let tracker = UsageTracker::noop();
        tracker
            .check_budget("wallet123", 0.50, None)
            .await
            .expect("untagged within-cap must pass without Redis");
        tracker
            .check_budget("wallet123", 0.50, Some("acme"))
            .await
            .expect("tagged within-cap must pass identically without Redis");
    }

    /// Migration 011 SQL (loaded from file).
    const MIGRATION_011: &str = include_str!("../../../migrations/011_tenant_budgets.sql");

    #[test]
    fn test_migration_011_creates_tenant_budgets_table() {
        assert!(
            MIGRATION_011.contains("CREATE TABLE IF NOT EXISTS tenant_budgets"),
            "migration must create tenant_budgets table"
        );
        assert!(
            MIGRATION_011.contains("PRIMARY KEY (wallet_address, tenant)"),
            "tenant_budgets PK must be (wallet_address, tenant)"
        );
        for col in [
            "hourly_limit_usdc",
            "daily_limit_usdc",
            "monthly_limit_usdc",
        ] {
            assert!(
                MIGRATION_011.contains(col),
                "tenant_budgets must have {col}"
            );
        }
    }

    #[test]
    fn test_migration_011_adds_require_tenant_column() {
        assert!(
            MIGRATION_011
                .contains("ADD COLUMN IF NOT EXISTS require_tenant BOOLEAN NOT NULL DEFAULT FALSE"),
            "migration must add require_tenant column defaulting to FALSE"
        );
    }

    #[test]
    fn test_migration_011_creates_updated_at_trigger() {
        assert!(
            MIGRATION_011.contains("trg_tenant_budgets_updated_at"),
            "migration must create updated_at trigger for tenant_budgets"
        );
        assert!(
            MIGRATION_011.contains("update_updated_at_column()"),
            "trigger must use the generic update_updated_at_column function"
        );
    }

    #[test]
    fn test_tenant_budget_config_serialization_roundtrip() {
        let config = TenantBudgetConfig {
            hourly: Some(1.0),
            daily: Some(10.0),
            monthly: None,
        };
        let json = serde_json::to_string(&config).expect("serialize");
        let back: TenantBudgetConfig = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.hourly, Some(1.0));
        assert_eq!(back.daily, Some(10.0));
        assert!(back.monthly.is_none());
    }

    /// #501: the cache-derivation contract for `get_team_budget_config`. A
    /// transient DB error must produce NO cache write (so a blip cannot poison
    /// the team cap for a full TTL), a confirmed absence caches the `"none"`
    /// sentinel, and a found row caches its serialized JSON. Mirrors the
    /// `TenantLookup` no-cache-on-error behavior.
    #[test]
    fn test_team_lookup_cache_value_never_caches_db_error() {
        // DB error → never cached.
        assert_eq!(
            TeamLookup::DbError.cache_value(),
            None,
            "a transient team_budgets DB error must NEVER be cached"
        );

        // Confirmed absence → "none" sentinel (regression guard for the
        // happy-path cache fill).
        assert_eq!(
            TeamLookup::Absent.cache_value(),
            Some("none".to_string()),
            "a confirmed-absent team budget must cache the \"none\" sentinel"
        );

        // Found → serialized JSON that round-trips back to the same config and
        // is distinct from the absence sentinel.
        let cfg = TeamBudgetConfig {
            hourly: Some(0.50),
            daily: Some(10.0),
            monthly: None,
        };
        let cached = TeamLookup::Found(cfg)
            .cache_value()
            .expect("a found team budget must be cached");
        assert_ne!(
            cached, "none",
            "a real budget must not serialize to \"none\""
        );
        let back: TeamBudgetConfig =
            serde_json::from_str(&cached).expect("cached team budget must round-trip");
        assert_eq!(back.hourly, Some(0.50));
        assert_eq!(back.daily, Some(10.0));
        assert!(back.monthly.is_none());
    }
}
