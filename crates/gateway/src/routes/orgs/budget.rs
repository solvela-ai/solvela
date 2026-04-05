//! Budget management handlers for teams and wallets.

use super::*;

/// Request body for setting budget limits.
#[derive(Debug, Deserialize, Serialize)]
pub struct SetBudgetRequest {
    pub hourly: Option<f64>,
    pub daily: Option<f64>,
    pub monthly: Option<f64>,
}

/// Validate a single budget field value — must be non-negative and finite.
fn validate_budget_value(
    val: Option<f64>,
    field_name: &str,
) -> Result<(), (StatusCode, Json<serde_json::Value>)> {
    if let Some(v) = val {
        if v.is_nan() || v.is_infinite() || v < 0.0 {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": format!("{} must be a non-negative finite number", field_name)
                })),
            ));
        }
    }
    Ok(())
}

/// Response body for budget endpoints (limits + current spend).
#[derive(Debug, Serialize)]
pub struct BudgetResponse {
    pub hourly_limit: Option<f64>,
    pub daily_limit: Option<f64>,
    pub monthly_limit: Option<f64>,
    pub hourly_spend: f64,
    pub daily_spend: f64,
    pub monthly_spend: f64,
}

/// `PUT /v1/orgs/:id/teams/:tid/budget` — Set team budget limits.
pub async fn set_team_budget(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    org_ctx: Option<Extension<OrgContext>>,
    Path((org_id, team_id)): Path<(Uuid, Uuid)>,
    Json(body): Json<SetBudgetRequest>,
) -> Response {
    let auth = match require_auth(&state, &headers, org_ctx.as_ref().map(|e| &e.0)) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_org_admin_access(&auth, org_id) {
        return resp;
    }

    // Validate all budget fields before touching the DB.
    if let Err((status, body_err)) = validate_budget_value(body.hourly, "hourly") {
        return (status, body_err).into_response();
    }
    if let Err((status, body_err)) = validate_budget_value(body.daily, "daily") {
        return (status, body_err).into_response();
    }
    if let Err((status, body_err)) = validate_budget_value(body.monthly, "monthly") {
        return (status, body_err).into_response();
    }

    let pool = require_db!(state);

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
            Json(json!({ "error": "team not found in this organization" })),
        )
            .into_response();
    }

    let now = chrono::Utc::now();
    let result = sqlx::query(
        r#"INSERT INTO team_budgets (team_id, hourly_limit_usdc, daily_limit_usdc, monthly_limit_usdc, created_at, updated_at)
           VALUES ($1, $2, $3, $4, $5, $6)
           ON CONFLICT (team_id) DO UPDATE SET
               hourly_limit_usdc = EXCLUDED.hourly_limit_usdc,
               daily_limit_usdc = EXCLUDED.daily_limit_usdc,
               monthly_limit_usdc = EXCLUDED.monthly_limit_usdc,
               updated_at = EXCLUDED.updated_at"#,
    )
    .bind(team_id)
    .bind(body.hourly)
    .bind(body.daily)
    .bind(body.monthly)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await;

    match result {
        Ok(_) => {
            // Invalidate the cached team budget config so the next read reflects
            // the new limits immediately.
            if let Some(redis_client) = state.usage.redis_client() {
                match redis_client.get_multiplexed_async_connection().await {
                    Ok(mut conn) => {
                        let cache_key = format!("team_budget:{}", team_id);
                        if let Err(e) = redis::cmd("DEL")
                            .arg(&cache_key)
                            .query_async::<()>(&mut conn)
                            .await
                        {
                            tracing::warn!(cache_key = %cache_key, error = %e, "failed to invalidate budget cache");
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Redis unavailable for budget cache invalidation");
                    }
                }
            }

            let actor_api_key = match &auth {
                AuthContext::OrgKey(ctx) => Some(ctx.api_key_id),
                AuthContext::Admin => None,
            };

            log_audit(
                pool,
                AuditEntry {
                    org_id: Some(org_id),
                    actor_wallet: None,
                    actor_api_key,
                    action: "budget.team_updated".to_string(),
                    resource_type: "team_budget".to_string(),
                    resource_id: Some(team_id.to_string()),
                    details: Some(serde_json::to_value(&body).unwrap_or_default()),
                    ip_address: None,
                },
            );

            (StatusCode::OK, Json(json!({ "updated": true }))).into_response()
        }
        Err(e) => {
            tracing::warn!(team_id = %team_id, error = %e, "failed to set team budget");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to set team budget" })),
            )
                .into_response()
        }
    }
}

/// `GET /v1/orgs/:id/teams/:tid/budget` — Get team budget + current spend.
pub async fn get_team_budget(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    org_ctx: Option<Extension<OrgContext>>,
    Path((org_id, team_id)): Path<(Uuid, Uuid)>,
) -> Response {
    let auth = match require_auth(&state, &headers, org_ctx.as_ref().map(|e| &e.0)) {
        Ok(a) => a,
        Err(resp) => return resp,
    };
    if let Err(resp) = require_org_access(&auth, org_id) {
        return resp;
    }
    let pool = require_db!(state);

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
            Json(json!({ "error": "team not found in this organization" })),
        )
            .into_response();
    }

    let row = sqlx::query_as::<_, (Option<f64>, Option<f64>, Option<f64>)>(
        r#"SELECT
            hourly_limit_usdc::DOUBLE PRECISION,
            daily_limit_usdc::DOUBLE PRECISION,
            monthly_limit_usdc::DOUBLE PRECISION
        FROM team_budgets
        WHERE team_id = $1"#,
    )
    .bind(team_id)
    .fetch_optional(pool)
    .await;

    let (hourly_limit, daily_limit, monthly_limit) = match row {
        Ok(Some(r)) => r,
        Ok(None) => (None, None, None),
        Err(e) => {
            tracing::warn!(team_id = %team_id, error = %e, "failed to query team budget");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to query team budget" })),
            )
                .into_response();
        }
    };

    let now = chrono::Utc::now();
    let tid_str = team_id.to_string();
    let (hourly_spend, daily_spend, monthly_spend) =
        if let Some(client) = &state.usage.redis_client() {
            let hour_key = format!("team_spend:{}:{}", tid_str, now.format("%Y-%m-%dT%H"));
            let day_key = format!("team_spend:{}:{}", tid_str, now.format("%Y-%m-%d"));
            let month_key = format!("team_spend:{}:{}", tid_str, now.format("%Y-%m"));
            (
                crate::usage::get_redis_spend(client, &hour_key)
                    .await
                    .unwrap_or(0.0),
                crate::usage::get_redis_spend(client, &day_key)
                    .await
                    .unwrap_or(0.0),
                crate::usage::get_redis_spend(client, &month_key)
                    .await
                    .unwrap_or(0.0),
            )
        } else {
            (0.0, 0.0, 0.0)
        };

    (
        StatusCode::OK,
        Json(BudgetResponse {
            hourly_limit,
            daily_limit,
            monthly_limit,
            hourly_spend,
            daily_spend,
            monthly_spend,
        }),
    )
        .into_response()
}

/// `PUT /v1/wallets/:wallet/budget` — Set wallet budget limits.
///
/// Note: wallet budget endpoints use admin-only auth because the URL path
/// (`/v1/wallets/{wallet}/budget`) is not org-scoped. Org-scoped API keys
/// cannot be verified against a specific org without an org_id in the path.
pub async fn set_wallet_budget(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(wallet): Path<String>,
    Json(body): Json<SetBudgetRequest>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers) {
        return resp;
    }

    // Validate all budget fields before touching the DB.
    if let Err((status, body_err)) = validate_budget_value(body.hourly, "hourly") {
        return (status, body_err).into_response();
    }
    if let Err((status, body_err)) = validate_budget_value(body.daily, "daily") {
        return (status, body_err).into_response();
    }
    if let Err((status, body_err)) = validate_budget_value(body.monthly, "monthly") {
        return (status, body_err).into_response();
    }

    let pool = require_db!(state);

    let now = chrono::Utc::now();
    let result = sqlx::query(
        r#"INSERT INTO wallet_budgets (wallet_address, hourly_limit_usdc, daily_limit_usdc, monthly_limit_usdc, created_at)
           VALUES ($1, $2, $3, $4, $5)
           ON CONFLICT (wallet_address) DO UPDATE SET
               hourly_limit_usdc = EXCLUDED.hourly_limit_usdc,
               daily_limit_usdc = EXCLUDED.daily_limit_usdc,
               monthly_limit_usdc = EXCLUDED.monthly_limit_usdc"#,
    )
    .bind(&wallet)
    .bind(body.hourly)
    .bind(body.daily)
    .bind(body.monthly)
    .bind(now)
    .execute(pool)
    .await;

    match result {
        Ok(_) => {
            // Invalidate the cached wallet budget config so the next read reflects
            // the new limits immediately.
            if let Some(redis_client) = state.usage.redis_client() {
                match redis_client.get_multiplexed_async_connection().await {
                    Ok(mut conn) => {
                        let cache_key = format!("budget_config:{}", wallet);
                        if let Err(e) = redis::cmd("DEL")
                            .arg(&cache_key)
                            .query_async::<()>(&mut conn)
                            .await
                        {
                            tracing::warn!(cache_key = %cache_key, error = %e, "failed to invalidate budget cache");
                        }
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "Redis unavailable for budget cache invalidation");
                    }
                }
            }

            log_audit(
                pool,
                AuditEntry {
                    org_id: None,
                    actor_wallet: None,
                    actor_api_key: None,
                    action: "budget.wallet_updated".to_string(),
                    resource_type: "wallet_budget".to_string(),
                    resource_id: Some(wallet.clone()),
                    details: Some(serde_json::to_value(&body).unwrap_or_default()),
                    ip_address: None,
                },
            );

            (StatusCode::OK, Json(json!({ "updated": true }))).into_response()
        }
        Err(e) => {
            tracing::warn!(wallet = %wallet, error = %e, "failed to set wallet budget");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to set wallet budget" })),
            )
                .into_response()
        }
    }
}

/// `GET /v1/wallets/:wallet/budget` — Get wallet budget + current spend.
///
/// Note: wallet budget endpoints use admin-only auth because the URL path
/// (`/v1/wallets/{wallet}/budget`) is not org-scoped. Org-scoped API keys
/// cannot be verified against a specific org without an org_id in the path.
pub async fn get_wallet_budget(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(wallet): Path<String>,
) -> Response {
    if let Err(resp) = require_admin(&state, &headers) {
        return resp;
    }
    let pool = require_db!(state);

    let row = sqlx::query_as::<_, (Option<f64>, Option<f64>, Option<f64>)>(
        r#"SELECT
            hourly_limit_usdc::DOUBLE PRECISION,
            daily_limit_usdc::DOUBLE PRECISION,
            monthly_limit_usdc::DOUBLE PRECISION
        FROM wallet_budgets
        WHERE wallet_address = $1"#,
    )
    .bind(&wallet)
    .fetch_optional(pool)
    .await;

    let (hourly_limit, daily_limit, monthly_limit) = match row {
        Ok(Some(r)) => r,
        Ok(None) => (None, Some(100.0), None), // Default $100/day
        Err(e) => {
            tracing::warn!(wallet = %wallet, error = %e, "failed to query wallet budget");
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to query wallet budget" })),
            )
                .into_response();
        }
    };

    let now = chrono::Utc::now();
    let (hourly_spend, daily_spend, monthly_spend) =
        if let Some(client) = &state.usage.redis_client() {
            let hour_key = format!("spend:{}:{}", wallet, now.format("%Y-%m-%dT%H"));
            let day_key = format!("spend:{}:{}", wallet, now.format("%Y-%m-%d"));
            let month_key = format!("spend:{}:{}", wallet, now.format("%Y-%m"));
            (
                crate::usage::get_redis_spend(client, &hour_key)
                    .await
                    .unwrap_or(0.0),
                crate::usage::get_redis_spend(client, &day_key)
                    .await
                    .unwrap_or(0.0),
                crate::usage::get_redis_spend(client, &month_key)
                    .await
                    .unwrap_or(0.0),
            )
        } else {
            (0.0, 0.0, 0.0)
        };

    (
        StatusCode::OK,
        Json(BudgetResponse {
            hourly_limit,
            daily_limit,
            monthly_limit,
            hourly_spend,
            daily_spend,
            monthly_spend,
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
    use super::validate_budget_value;

    // -----------------------------------------------------------------------
    // validate_budget_value unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn validate_budget_value_none_passes() {
        assert!(validate_budget_value(None, "hourly").is_ok());
    }

    #[test]
    fn validate_budget_value_zero_passes() {
        assert!(validate_budget_value(Some(0.0), "hourly").is_ok());
    }

    #[test]
    fn validate_budget_value_positive_passes() {
        assert!(validate_budget_value(Some(50.0), "daily").is_ok());
    }

    #[test]
    fn validate_budget_value_negative_returns_err() {
        assert!(validate_budget_value(Some(-1.0), "hourly").is_err());
    }

    #[test]
    fn validate_budget_value_nan_returns_err() {
        assert!(validate_budget_value(Some(f64::NAN), "daily").is_err());
    }

    #[test]
    fn validate_budget_value_infinity_returns_err() {
        assert!(validate_budget_value(Some(f64::INFINITY), "monthly").is_err());
        assert!(validate_budget_value(Some(f64::NEG_INFINITY), "monthly").is_err());
    }

    #[tokio::test]
    async fn set_team_budget_requires_auth() {
        let app = test_router(Some("tok"));
        let org_id = Uuid::new_v4();
        let team_id = Uuid::new_v4();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/v1/orgs/{org_id}/teams/{team_id}/budget"))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"daily":200.0}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn get_team_budget_requires_auth() {
        let app = test_router(Some("tok"));
        let org_id = Uuid::new_v4();
        let team_id = Uuid::new_v4();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/orgs/{org_id}/teams/{team_id}/budget"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn set_team_budget_no_db_returns_503() {
        let app = test_router(Some("mytoken"));
        let org_id = Uuid::new_v4();
        let team_id = Uuid::new_v4();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri(format!("/v1/orgs/{org_id}/teams/{team_id}/budget"))
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer mytoken")
                    .body(Body::from(r#"{"daily":200.0}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn get_team_budget_no_db_returns_503() {
        let app = test_router(Some("mytoken"));
        let org_id = Uuid::new_v4();
        let team_id = Uuid::new_v4();

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(format!("/v1/orgs/{org_id}/teams/{team_id}/budget"))
                    .header("authorization", "Bearer mytoken")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn set_wallet_budget_requires_auth() {
        let app = test_router(Some("tok"));

        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/v1/wallets/WalletABC/budget")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"daily":50.0}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn get_wallet_budget_requires_auth() {
        let app = test_router(Some("tok"));

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/wallets/WalletABC/budget")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn set_wallet_budget_no_db_returns_503() {
        let app = test_router(Some("mytoken"));

        let resp = app
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/v1/wallets/WalletABC/budget")
                    .header("content-type", "application/json")
                    .header("authorization", "Bearer mytoken")
                    .body(Body::from(r#"{"daily":50.0}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn get_wallet_budget_no_db_returns_503() {
        let app = test_router(Some("mytoken"));

        let resp = app
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri("/v1/wallets/WalletABC/budget")
                    .header("authorization", "Bearer mytoken")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }
}
