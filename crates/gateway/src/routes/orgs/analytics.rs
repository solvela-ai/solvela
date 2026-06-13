//! Analytics handlers for team and org spend statistics.

use super::*;

/// Query parameters for analytics endpoints.
#[derive(Debug, Deserialize)]
pub struct StatsQuery {
    #[serde(default = "default_stats_days")]
    pub days: i32,
    /// Max number of rows for paginated lists (`top_wallets`). Clamped to
    /// `1..=MAX_STATS_LIMIT`; defaults to `DEFAULT_STATS_LIMIT`.
    #[serde(default)]
    pub limit: Option<i64>,
    /// Row offset for paginated lists. Clamped to `>= 0`; defaults to 0.
    #[serde(default)]
    pub offset: Option<i64>,
}

fn default_stats_days() -> i32 {
    7
}

/// Default page size for `top_wallets` (preserves the historical LIMIT 10).
const DEFAULT_STATS_LIMIT: i64 = 10;
/// Hard ceiling on a single page of `top_wallets` rows. Bounds the response
/// size regardless of client input (fail-safe; mirrors the `days` clamp).
const MAX_STATS_LIMIT: i64 = 100;

/// Response for team-scoped spend analytics.
#[derive(Debug, Serialize)]
pub struct TeamStatsResponse {
    pub team_id: Uuid,
    pub period_days: i32,
    pub total_spend_usdc: f64,
    pub total_requests: i64,
    pub by_model: Vec<ModelBreakdown>,
    pub by_provider: Vec<ProviderBreakdown>,
    pub by_wallet: Vec<WalletBreakdown>,
}

/// Per-model breakdown for team analytics.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ModelBreakdown {
    pub model: String,
    pub request_count: i64,
    pub total_cost_usdc: f64,
}

/// Per-provider breakdown for team analytics.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ProviderBreakdown {
    pub provider: String,
    pub request_count: i64,
    pub total_cost_usdc: f64,
}

/// Per-wallet breakdown for team analytics.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct WalletBreakdown {
    pub wallet_address: String,
    pub request_count: i64,
    pub total_cost_usdc: f64,
}

/// Response for org-level aggregate spend analytics.
#[derive(Debug, Serialize)]
pub struct OrgStatsResponse {
    pub org_id: Uuid,
    pub period_days: i32,
    pub total_spend_usdc: f64,
    pub total_requests: i64,
    /// Derived org budget rollup (sum of team budgets — see [`OrgBudgetRollup`]).
    pub budget: OrgBudgetRollup,
    pub by_team: Vec<TeamBreakdown>,
    /// Daily spend time series over the period (ascending by date).
    pub by_day: Vec<DayBucket>,
    /// Per-tenant breakdown (only rows tagged with a tenant).
    pub by_tenant: Vec<TenantBreakdown>,
    /// Per-vendor settlement/fee-receivable breakdown (vendor-settled rows).
    pub by_vendor: Vec<VendorBreakdown>,
    /// Top wallets by spend, paginated via `limit`/`offset`.
    pub top_wallets: Vec<WalletBreakdown>,
}

/// Per-team breakdown for org analytics.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct TeamBreakdown {
    pub team_id: Uuid,
    pub team_name: String,
    pub request_count: i64,
    pub total_cost_usdc: f64,
}

/// Derived org-level budget figure.
///
/// **This is NOT an enforced org budget.** Solvela has no `org_budgets` table:
/// enforcement lives at the leaf (wallet/team) level. This rollup is a
/// reporting-only **sum of the org's team budgets**, computed per period from
/// `team_budgets`. A team with no budget row (or a NULL limit for a given
/// period) is treated as **unlimited** and contributes nothing to the sum —
/// exactly as Postgres `SUM` skips NULLs. Because of that, the aggregate limit
/// for a period is only a meaningful ceiling when **every** team has a budget
/// for that period; otherwise it understates the true (partly-unlimited) cap.
/// The `teams_with_*_budget` counts (vs `teams_total`) make that gap explicit
/// so the number is never silently misleading.
///
/// All limits are nullable: `None` means "no team in the org set a limit for
/// this period" — never `0.0`, which would falsely read as a zero cap.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct OrgBudgetRollup {
    /// Sum of teams' hourly limits (USDC). `None` if no team set one.
    pub hourly_limit_usdc: Option<f64>,
    /// Sum of teams' daily limits (USDC). `None` if no team set one.
    pub daily_limit_usdc: Option<f64>,
    /// Sum of teams' monthly limits (USDC). `None` if no team set one.
    pub monthly_limit_usdc: Option<f64>,
    /// Total number of teams in the org (the denominator for the counts below).
    pub teams_total: i64,
    /// Teams contributing a non-NULL hourly limit to the sum.
    pub teams_with_hourly_budget: i64,
    /// Teams contributing a non-NULL daily limit to the sum.
    pub teams_with_daily_budget: i64,
    /// Teams contributing a non-NULL monthly limit to the sum.
    pub teams_with_monthly_budget: i64,
}

/// One day of the org spend time series.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct DayBucket {
    /// Calendar day (UTC), formatted `YYYY-MM-DD`.
    pub date: String,
    pub spend_usdc: f64,
    pub requests: i64,
}

/// Per-tenant breakdown for org analytics (tagged rows only).
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct TenantBreakdown {
    pub tenant: String,
    pub request_count: i64,
    pub total_cost_usdc: f64,
}

/// Per-vendor settlement breakdown for org analytics.
///
/// `settled_usdc` and `fee_receivable_usdc` are summed from the integer atomic
/// columns (`vendor_settled_atomic`, `vendor_fee_receivable_atomic`) and
/// rendered to a fixed-6-decimal USDC string via the canonical receipts helper
/// — no f64 ever touches these money values (solvela-fintech).
#[derive(Debug, Serialize)]
pub struct VendorBreakdown {
    pub vendor_wallet: String,
    pub request_count: i64,
    pub settled_usdc: String,
    pub fee_receivable_usdc: String,
}

/// `GET /v1/orgs/:id/teams/:tid/stats?days=7`
///
/// Returns team-scoped spend analytics for the given period.
/// Protected by admin token or org-scoped API key.
pub async fn get_team_stats(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    org_ctx: Option<Extension<OrgContext>>,
    Path((org_id, team_id)): Path<(Uuid, Uuid)>,
    Query(params): Query<StatsQuery>,
) -> Response {
    let auth = match require_auth(&state, &headers, org_ctx.as_ref().map(|e| &e.0)) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_org_access(&auth, org_id) {
        return resp;
    }
    let pool = require_db!(state);

    let days = params.days.clamp(1, 90);

    tracing::info!(org_id = %org_id, team_id = %team_id, days = %days, "team stats request");

    // Verify the team belongs to the org
    let team_exists: bool = match sqlx::query_as::<_, (bool,)>(
        "SELECT EXISTS(SELECT 1 FROM teams WHERE id = $1 AND org_id = $2)",
    )
    .bind(team_id)
    .bind(org_id)
    .fetch_one(pool)
    .await
    {
        Ok((exists,)) => exists,
        Err(e) => {
            tracing::warn!(team_id = %team_id, error = %e, "failed to verify team");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to verify team" })),
            )
                .into_response();
        }
    };

    if !team_exists {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "team not found" })),
        )
            .into_response();
    }

    // Summary query
    let summary = sqlx::query_as::<_, (i64, f64)>(
        r#"SELECT COUNT(*) AS total_requests,
                  COALESCE(SUM(s.cost_usdc), 0.0)::DOUBLE PRECISION AS total_spend_usdc
           FROM spend_logs s
           JOIN team_wallets tw ON tw.wallet_address = s.wallet_address
           WHERE tw.team_id = $1
             AND s.created_at >= NOW() - make_interval(days => $2)"#,
    )
    .bind(team_id)
    .bind(days)
    .fetch_one(pool)
    .await;

    let (total_requests, total_spend_usdc) = match summary {
        Ok(row) => row,
        Err(e) => {
            tracing::error!(team_id = %team_id, error = %e, "failed to fetch team stats summary");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to fetch team stats" })),
            )
                .into_response();
        }
    };

    // By-model breakdown
    let by_model = sqlx::query_as::<_, ModelBreakdown>(
        r#"SELECT s.model,
                  COUNT(*) AS request_count,
                  COALESCE(SUM(s.cost_usdc), 0.0)::DOUBLE PRECISION AS total_cost_usdc
           FROM spend_logs s
           JOIN team_wallets tw ON tw.wallet_address = s.wallet_address
           WHERE tw.team_id = $1
             AND s.created_at >= NOW() - make_interval(days => $2)
           GROUP BY s.model
           ORDER BY total_cost_usdc DESC"#,
    )
    .bind(team_id)
    .bind(days)
    .fetch_all(pool)
    .await;

    let by_model = match by_model {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(team_id = %team_id, error = %e, "failed to fetch team stats by model");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to fetch team stats by model" })),
            )
                .into_response();
        }
    };

    // By-provider breakdown
    let by_provider = sqlx::query_as::<_, ProviderBreakdown>(
        r#"SELECT s.provider,
                  COUNT(*) AS request_count,
                  COALESCE(SUM(s.cost_usdc), 0.0)::DOUBLE PRECISION AS total_cost_usdc
           FROM spend_logs s
           JOIN team_wallets tw ON tw.wallet_address = s.wallet_address
           WHERE tw.team_id = $1
             AND s.created_at >= NOW() - make_interval(days => $2)
           GROUP BY s.provider
           ORDER BY total_cost_usdc DESC"#,
    )
    .bind(team_id)
    .bind(days)
    .fetch_all(pool)
    .await;

    let by_provider = match by_provider {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(team_id = %team_id, error = %e, "failed to fetch team stats by provider");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to fetch team stats by provider" })),
            )
                .into_response();
        }
    };

    // By-wallet breakdown
    let by_wallet = sqlx::query_as::<_, WalletBreakdown>(
        r#"SELECT tw.wallet_address,
                  COUNT(*) AS request_count,
                  COALESCE(SUM(s.cost_usdc), 0.0)::DOUBLE PRECISION AS total_cost_usdc
           FROM team_wallets tw
           LEFT JOIN spend_logs s ON s.wallet_address = tw.wallet_address
               AND s.created_at >= NOW() - make_interval(days => $2)
           WHERE tw.team_id = $1
           GROUP BY tw.wallet_address
           ORDER BY total_cost_usdc DESC"#,
    )
    .bind(team_id)
    .bind(days)
    .fetch_all(pool)
    .await;

    let by_wallet = match by_wallet {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(team_id = %team_id, error = %e, "failed to fetch team stats by wallet");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to fetch team stats by wallet" })),
            )
                .into_response();
        }
    };

    (
        StatusCode::OK,
        Json(TeamStatsResponse {
            team_id,
            period_days: days,
            total_spend_usdc,
            total_requests,
            by_model,
            by_provider,
            by_wallet,
        }),
    )
        .into_response()
}

/// `GET /v1/orgs/:id/stats?days=7`
///
/// Returns org-level aggregate spend analytics for the given period.
/// Protected by admin token or org-scoped API key.
pub async fn get_org_stats(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    org_ctx: Option<Extension<OrgContext>>,
    Path(org_id): Path<Uuid>,
    Query(params): Query<StatsQuery>,
) -> Response {
    let auth = match require_auth(&state, &headers, org_ctx.as_ref().map(|e| &e.0)) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_org_access(&auth, org_id) {
        return resp;
    }
    let pool = require_db!(state);

    let days = params.days.clamp(1, 90);
    // Pagination for top_wallets: clamp to a bounded page and non-negative
    // offset so a client can never request an unbounded scan or a negative
    // bind (which Postgres would reject). Mirrors the `days` clamp pattern.
    let limit = params
        .limit
        .unwrap_or(DEFAULT_STATS_LIMIT)
        .clamp(1, MAX_STATS_LIMIT);
    let offset = params.offset.unwrap_or(0).max(0);

    tracing::info!(org_id = %org_id, days = %days, limit = %limit, offset = %offset, "org stats request");

    // ── Summary ─────────────────────────────────────────────────────────────
    // Count each spend row ONCE even when its wallet belongs to multiple teams
    // of the org. The naive `spend_logs JOIN team_wallets` fans a row out once
    // per team membership and double-counts the org total (team_wallets only
    // has UNIQUE(team_id, wallet_address), so multi-team membership is legal).
    // Restricting to the org's DISTINCT wallet set keeps the row multiplicity
    // at exactly one. See `org_stats_does_not_double_count_multi_team_wallet`.
    let summary = sqlx::query_as::<_, (i64, f64)>(
        r#"SELECT COUNT(*) AS total_requests,
                  COALESCE(SUM(s.cost_usdc), 0.0)::DOUBLE PRECISION AS total_spend_usdc
           FROM spend_logs s
           WHERE s.wallet_address IN (
                   SELECT DISTINCT tw.wallet_address
                   FROM team_wallets tw
                   JOIN teams t ON t.id = tw.team_id
                   WHERE t.org_id = $1
                 )
             AND s.created_at >= NOW() - make_interval(days => $2)"#,
    )
    .bind(org_id)
    .bind(days)
    .fetch_one(pool)
    .await;

    let (total_requests, total_spend_usdc) = match summary {
        Ok(row) => row,
        Err(e) => {
            tracing::error!(org_id = %org_id, error = %e, "failed to fetch org stats summary");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to fetch org stats" })),
            )
                .into_response();
        }
    };

    // ── Budget rollup (derived sum-of-team-budgets; reporting only) ──────────
    // NOT an enforced org budget — see OrgBudgetRollup docs. SUM skips NULLs,
    // so absent/period-NULL team budgets count as unlimited and the
    // teams_with_*_budget counts vs teams_total expose any partial coverage.
    let budget = sqlx::query_as::<_, OrgBudgetRollup>(
        r#"SELECT
               SUM(tb.hourly_limit_usdc)::DOUBLE PRECISION  AS hourly_limit_usdc,
               SUM(tb.daily_limit_usdc)::DOUBLE PRECISION   AS daily_limit_usdc,
               SUM(tb.monthly_limit_usdc)::DOUBLE PRECISION AS monthly_limit_usdc,
               COUNT(t.id)                                  AS teams_total,
               COUNT(tb.hourly_limit_usdc)                  AS teams_with_hourly_budget,
               COUNT(tb.daily_limit_usdc)                   AS teams_with_daily_budget,
               COUNT(tb.monthly_limit_usdc)                 AS teams_with_monthly_budget
           FROM teams t
           LEFT JOIN team_budgets tb ON tb.team_id = t.id
           WHERE t.org_id = $1"#,
    )
    .bind(org_id)
    .fetch_one(pool)
    .await;

    let budget = match budget {
        Ok(row) => row,
        Err(e) => {
            tracing::error!(org_id = %org_id, error = %e, "failed to fetch org budget rollup");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to fetch org budget rollup" })),
            )
                .into_response();
        }
    };

    // By-team breakdown
    let by_team = sqlx::query_as::<_, TeamBreakdown>(
        r#"SELECT t.id AS team_id,
                  t.name AS team_name,
                  COUNT(s.id) AS request_count,
                  COALESCE(SUM(s.cost_usdc), 0.0)::DOUBLE PRECISION AS total_cost_usdc
           FROM teams t
           LEFT JOIN team_wallets tw ON tw.team_id = t.id
           LEFT JOIN spend_logs s ON s.wallet_address = tw.wallet_address
               AND s.created_at >= NOW() - make_interval(days => $2)
           WHERE t.org_id = $1
           GROUP BY t.id, t.name
           ORDER BY total_cost_usdc DESC"#,
    )
    .bind(org_id)
    .bind(days)
    .fetch_all(pool)
    .await;

    let by_team = match by_team {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(org_id = %org_id, error = %e, "failed to fetch org stats by team");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to fetch org stats by team" })),
            )
                .into_response();
        }
    };

    // Top wallets by spend, paginated. Aggregate over the org's DISTINCT wallet
    // set (subquery) so a multi-team wallet is one row with its true total — not
    // multiplied by the team_wallets fan-out (the same bug fixed in the summary).
    let top_wallets = sqlx::query_as::<_, WalletBreakdown>(
        r#"SELECT ow.wallet_address,
                  COUNT(s.id) AS request_count,
                  COALESCE(SUM(s.cost_usdc), 0.0)::DOUBLE PRECISION AS total_cost_usdc
           FROM (
                   SELECT DISTINCT tw.wallet_address
                   FROM team_wallets tw
                   JOIN teams t ON t.id = tw.team_id
                   WHERE t.org_id = $1
                 ) ow
           LEFT JOIN spend_logs s ON s.wallet_address = ow.wallet_address
               AND s.created_at >= NOW() - make_interval(days => $2)
           GROUP BY ow.wallet_address
           ORDER BY total_cost_usdc DESC, ow.wallet_address ASC
           LIMIT $3 OFFSET $4"#,
    )
    .bind(org_id)
    .bind(days)
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await;

    let top_wallets = match top_wallets {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(org_id = %org_id, error = %e, "failed to fetch org top wallets");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to fetch org top wallets" })),
            )
                .into_response();
        }
    };

    // Daily time series (mirrors usage::get_stats_by_day). Aggregated over the
    // org's DISTINCT wallet set so days are not double-counted either.
    let by_day = sqlx::query_as::<_, DayBucket>(
        r#"SELECT TO_CHAR(DATE(s.created_at), 'YYYY-MM-DD') AS date,
                  COALESCE(SUM(s.cost_usdc), 0.0)::DOUBLE PRECISION AS spend_usdc,
                  COUNT(*) AS requests
           FROM spend_logs s
           WHERE s.wallet_address IN (
                   SELECT DISTINCT tw.wallet_address
                   FROM team_wallets tw
                   JOIN teams t ON t.id = tw.team_id
                   WHERE t.org_id = $1
                 )
             AND s.created_at >= NOW() - make_interval(days => $2)
           GROUP BY DATE(s.created_at)
           ORDER BY DATE(s.created_at) ASC"#,
    )
    .bind(org_id)
    .bind(days)
    .fetch_all(pool)
    .await;

    let by_day = match by_day {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(org_id = %org_id, error = %e, "failed to fetch org stats by day");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to fetch org stats by day" })),
            )
                .into_response();
        }
    };

    // Per-tenant breakdown (tagged rows only; mirrors usage::get_stats_by_tenant).
    let by_tenant = sqlx::query_as::<_, TenantBreakdown>(
        r#"SELECT s.tenant,
                  COUNT(*) AS request_count,
                  COALESCE(SUM(s.cost_usdc), 0.0)::DOUBLE PRECISION AS total_cost_usdc
           FROM spend_logs s
           WHERE s.wallet_address IN (
                   SELECT DISTINCT tw.wallet_address
                   FROM team_wallets tw
                   JOIN teams t ON t.id = tw.team_id
                   WHERE t.org_id = $1
                 )
             AND s.created_at >= NOW() - make_interval(days => $2)
             AND s.tenant IS NOT NULL
           GROUP BY s.tenant
           ORDER BY total_cost_usdc DESC, s.tenant ASC"#,
    )
    .bind(org_id)
    .bind(days)
    .fetch_all(pool)
    .await;

    let by_tenant = match by_tenant {
        Ok(rows) => rows,
        Err(e) => {
            tracing::error!(org_id = %org_id, error = %e, "failed to fetch org stats by tenant");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to fetch org stats by tenant" })),
            )
                .into_response();
        }
    };

    // Per-vendor settlement breakdown. The atomic columns are integers; sum
    // them in SQL as BIGINT, then format atomic->USDC string with the canonical
    // receipts helper (no f64 on the money value — solvela-fintech). The summed
    // BIGINT could in principle exceed u64 only past ~1.8e13 USDC, which the
    // per-column CHECK(>=0) and realistic volumes preclude; a negative or
    // out-of-range sum is rejected via try_from rather than silently wrapped.
    let by_vendor_rows = sqlx::query_as::<_, (String, i64, i64, i64)>(
        // SUM over BIGINT returns NUMERIC in Postgres; cast back to BIGINT so
        // it decodes as i64 (volumes are far below the i64 ceiling, and the
        // out-of-range guard below fails closed if that ever changes).
        r#"SELECT s.vendor_wallet,
                  COUNT(*) AS request_count,
                  COALESCE(SUM(s.vendor_settled_atomic), 0)::BIGINT AS settled_atomic,
                  COALESCE(SUM(s.vendor_fee_receivable_atomic), 0)::BIGINT AS fee_atomic
           FROM spend_logs s
           WHERE s.wallet_address IN (
                   SELECT DISTINCT tw.wallet_address
                   FROM team_wallets tw
                   JOIN teams t ON t.id = tw.team_id
                   WHERE t.org_id = $1
                 )
             AND s.created_at >= NOW() - make_interval(days => $2)
             AND s.vendor_wallet IS NOT NULL
           GROUP BY s.vendor_wallet
           ORDER BY settled_atomic DESC, s.vendor_wallet ASC"#,
    )
    .bind(org_id)
    .bind(days)
    .fetch_all(pool)
    .await;

    let by_vendor = match by_vendor_rows {
        Ok(rows) => {
            let mut out = Vec::with_capacity(rows.len());
            for (vendor_wallet, request_count, settled_atomic, fee_atomic) in rows {
                // Fail-closed: a negative/out-of-range sum must not silently
                // wrap into a tiny or huge USDC figure (solvela-fintech).
                let (settled_u64, fee_u64) =
                    match (u64::try_from(settled_atomic), u64::try_from(fee_atomic)) {
                        (Ok(s), Ok(f)) => (s, f),
                        _ => {
                            tracing::error!(
                                org_id = %org_id,
                                vendor_wallet = %vendor_wallet,
                                settled_atomic,
                                fee_atomic,
                                "vendor receivable sum out of u64 range"
                            );
                            return (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                Json(json!({ "error": "failed to fetch org stats by vendor" })),
                            )
                                .into_response();
                        }
                    };
                out.push(VendorBreakdown {
                    vendor_wallet,
                    request_count,
                    settled_usdc: crate::receipts::atomic_to_usdc_string(settled_u64),
                    fee_receivable_usdc: crate::receipts::atomic_to_usdc_string(fee_u64),
                });
            }
            out
        }
        Err(e) => {
            tracing::error!(org_id = %org_id, error = %e, "failed to fetch org stats by vendor");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to fetch org stats by vendor" })),
            )
                .into_response();
        }
    };

    (
        StatusCode::OK,
        Json(OrgStatsResponse {
            org_id,
            period_days: days,
            total_spend_usdc,
            total_requests,
            budget,
            by_team,
            by_day,
            by_tenant,
            by_vendor,
            top_wallets,
        }),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::super::test_helpers::test_router;

    #[tokio::test]
    async fn get_team_stats_requires_auth() {
        let app = test_router(Some("tok"));
        let org_id = Uuid::new_v4();
        let team_id = Uuid::new_v4();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/orgs/{org_id}/teams/{team_id}/stats"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn get_team_stats_no_db_returns_503() {
        let app = test_router(Some("mytoken"));
        let org_id = Uuid::new_v4();
        let team_id = Uuid::new_v4();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/orgs/{org_id}/teams/{team_id}/stats"))
                    .header("authorization", "Bearer mytoken")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn get_org_stats_requires_auth() {
        let app = test_router(Some("tok"));
        let org_id = Uuid::new_v4();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/orgs/{org_id}/stats"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn get_org_stats_no_db_returns_503() {
        let app = test_router(Some("mytoken"));
        let org_id = Uuid::new_v4();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/orgs/{org_id}/stats"))
                    .header("authorization", "Bearer mytoken")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
