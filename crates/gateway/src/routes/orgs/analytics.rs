//! Analytics handlers for team and org spend statistics.

use super::*;

/// Query parameters for analytics endpoints.
#[derive(Debug, Deserialize)]
pub struct StatsQuery {
    #[serde(default = "default_stats_days")]
    pub days: i32,
}

fn default_stats_days() -> i32 {
    7
}

/// Response for team-scoped spend analytics.
#[derive(Debug, Serialize)]
pub struct TeamStatsResponse {
    pub team_id: Uuid,
    pub period_days: i32,
    pub total_spend_usdc: f64,
    pub total_requests: i64,
    pub by_model: Vec<ModelBreakdown>,
    pub by_wallet: Vec<WalletBreakdown>,
}

/// Per-model breakdown for team analytics.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ModelBreakdown {
    pub model: String,
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
    pub by_team: Vec<TeamBreakdown>,
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

    tracing::info!(org_id = %org_id, days = %days, "org stats request");

    // Summary query
    let summary = sqlx::query_as::<_, (i64, f64)>(
        r#"SELECT COUNT(*) AS total_requests,
                  COALESCE(SUM(s.cost_usdc), 0.0)::DOUBLE PRECISION AS total_spend_usdc
           FROM spend_logs s
           JOIN team_wallets tw ON tw.wallet_address = s.wallet_address
           JOIN teams t ON t.id = tw.team_id
           WHERE t.org_id = $1
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

    // Top wallets (top 10 by spend)
    let top_wallets = sqlx::query_as::<_, WalletBreakdown>(
        r#"SELECT tw.wallet_address,
                  COUNT(s.id) AS request_count,
                  COALESCE(SUM(s.cost_usdc), 0.0)::DOUBLE PRECISION AS total_cost_usdc
           FROM team_wallets tw
           JOIN teams t ON t.id = tw.team_id
           LEFT JOIN spend_logs s ON s.wallet_address = tw.wallet_address
               AND s.created_at >= NOW() - make_interval(days => $2)
           WHERE t.org_id = $1
           GROUP BY tw.wallet_address
           ORDER BY total_cost_usdc DESC
           LIMIT 10"#,
    )
    .bind(org_id)
    .bind(days)
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

    (
        StatusCode::OK,
        Json(OrgStatsResponse {
            org_id,
            period_days: days,
            total_spend_usdc,
            total_requests,
            by_team,
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
