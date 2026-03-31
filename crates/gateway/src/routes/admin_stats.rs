//! Admin aggregate statistics endpoint.
//!
//! `GET /v1/admin/stats?days=30` returns platform-wide aggregated spend data
//! grouped by summary, model, day, and top wallets. Requires admin token auth.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::security;
use crate::AppState;

/// Query parameters for the admin stats endpoint.
#[derive(Debug, Deserialize)]
pub struct AdminStatsQuery {
    #[serde(default = "default_days")]
    pub days: i32,
}

fn default_days() -> i32 {
    30
}

/// Top-level admin stats response.
#[derive(Debug, Clone, Serialize)]
pub struct AdminStatsResponse {
    pub period_days: i32,
    pub summary: AdminSummary,
    pub by_model: Vec<AdminModelStats>,
    pub by_day: Vec<AdminDayStats>,
    pub top_wallets: Vec<AdminWalletStats>,
}

/// Aggregated spend summary for the period (platform-wide).
#[derive(Debug, Clone, Serialize)]
pub struct AdminSummary {
    pub total_requests: i64,
    pub total_cost_usdc: String,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub unique_wallets: i64,
    pub cache_hit_rate: Option<f64>,
}

/// Per-model breakdown.
#[derive(Debug, Clone, Serialize)]
pub struct AdminModelStats {
    pub model: String,
    pub provider: String,
    pub requests: i64,
    pub cost_usdc: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// Per-day breakdown.
#[derive(Debug, Clone, Serialize)]
pub struct AdminDayStats {
    pub date: String,
    pub requests: i64,
    pub cost_usdc: String,
    pub spend: f64,
}

/// Top wallet breakdown.
#[derive(Debug, Clone, Serialize)]
pub struct AdminWalletStats {
    pub wallet: String,
    pub requests: i64,
    pub cost_usdc: String,
}

/// `GET /v1/admin/stats`
///
/// Returns platform-wide spend statistics for the given period.
/// Protected by admin token (Bearer auth).
pub async fn admin_stats(
    State(state): State<Arc<AppState>>,
    Query(params): Query<AdminStatsQuery>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    // Gate behind admin token — if not configured, hide the endpoint entirely
    let admin_token = match &state.admin_token {
        Some(t) => t,
        None => {
            return Err(
                (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response(),
            );
        }
    };

    // Validate Bearer token using constant-time comparison
    let authorized = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|token| security::constant_time_eq(token.as_bytes(), admin_token.as_bytes()));

    if !authorized {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response());
    }

    // Validate days parameter
    if params.days < 1 || params.days > 365 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": format!("days must be between 1 and 365, got {}", params.days)
            })),
        )
            .into_response());
    }

    tracing::info!(days = %params.days, "admin stats request");

    // Check for database availability
    let pool = match &state.db_pool {
        Some(pool) => pool,
        None => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({ "error": "database not configured" })),
            )
                .into_response());
        }
    };

    let days = params.days;

    // Run all four queries concurrently
    let (summary_result, by_model_result, by_day_result, top_wallets_result) = tokio::join!(
        get_admin_summary(pool, days),
        get_admin_stats_by_model(pool, days),
        get_admin_stats_by_day(pool, days),
        get_admin_top_wallets(pool, days),
    );

    let summary_row = summary_result.map_err(|e| {
        tracing::error!(error = %e, "failed to retrieve admin stats summary");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "failed to retrieve stats summary" })),
        )
            .into_response()
    })?;

    let model_rows = by_model_result.map_err(|e| {
        tracing::error!(error = %e, "failed to retrieve admin stats by model");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "failed to retrieve stats by model" })),
        )
            .into_response()
    })?;

    let day_rows = by_day_result.map_err(|e| {
        tracing::error!(error = %e, "failed to retrieve admin stats by day");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "failed to retrieve stats by day" })),
        )
            .into_response()
    })?;

    let wallet_rows = top_wallets_result.map_err(|e| {
        tracing::error!(error = %e, "failed to retrieve admin top wallets");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": "failed to retrieve top wallets" })),
        )
            .into_response()
    })?;

    let summary = AdminSummary {
        total_requests: summary_row.total_requests,
        total_cost_usdc: format!("{:.6}", summary_row.total_cost),
        total_input_tokens: summary_row.total_input,
        total_output_tokens: summary_row.total_output,
        unique_wallets: summary_row.unique_wallets,
        cache_hit_rate: None, // Cache hit rate not yet tracked in DB
    };

    let by_model = model_rows
        .into_iter()
        .map(|r| AdminModelStats {
            model: r.model,
            provider: r.provider,
            requests: r.requests,
            cost_usdc: format!("{:.6}", r.cost),
            input_tokens: r.input_tokens,
            output_tokens: r.output_tokens,
        })
        .collect();

    let by_day = day_rows
        .into_iter()
        .map(|r| AdminDayStats {
            date: r.date.to_string(),
            requests: r.requests,
            cost_usdc: format!("{:.6}", r.cost),
            spend: r.cost,
        })
        .collect();

    let top_wallets = wallet_rows
        .into_iter()
        .map(|r| AdminWalletStats {
            wallet: r.wallet,
            requests: r.requests,
            cost_usdc: format!("{:.6}", r.cost),
        })
        .collect();

    Ok(Json(AdminStatsResponse {
        period_days: days,
        summary,
        by_model,
        by_day,
        top_wallets,
    })
    .into_response())
}

// ---------------------------------------------------------------------------
// Query helper types and functions
// ---------------------------------------------------------------------------

/// Summary row for admin aggregate stats.
struct AdminSummaryRow {
    total_requests: i64,
    total_cost: f64,
    total_input: i64,
    total_output: i64,
    unique_wallets: i64,
}

/// Per-model row for admin stats.
struct AdminModelRow {
    model: String,
    provider: String,
    requests: i64,
    cost: f64,
    input_tokens: i64,
    output_tokens: i64,
}

/// Per-day row for admin stats.
struct AdminDayRow {
    date: chrono::NaiveDate,
    requests: i64,
    cost: f64,
}

/// Top wallet row for admin stats.
struct AdminWalletRow {
    wallet: String,
    requests: i64,
    cost: f64,
}

/// Fetch platform-wide aggregate summary over the given number of days.
async fn get_admin_summary(pool: &sqlx::PgPool, days: i32) -> Result<AdminSummaryRow, sqlx::Error> {
    let row: (i64, f64, i64, i64, i64) = sqlx::query_as(
        r#"SELECT COUNT(*) as total_requests,
                  COALESCE(SUM(cost_usdc), 0) as total_cost,
                  COALESCE(SUM(input_tokens), 0) as total_input,
                  COALESCE(SUM(output_tokens), 0) as total_output,
                  COUNT(DISTINCT wallet_address) as unique_wallets
           FROM spend_logs
           WHERE created_at >= NOW() - make_interval(days => $1)"#,
    )
    .bind(days)
    .fetch_one(pool)
    .await?;

    Ok(AdminSummaryRow {
        total_requests: row.0,
        total_cost: row.1,
        total_input: row.2,
        total_output: row.3,
        unique_wallets: row.4,
    })
}

/// Fetch per-model breakdown (platform-wide) over the given number of days.
async fn get_admin_stats_by_model(
    pool: &sqlx::PgPool,
    days: i32,
) -> Result<Vec<AdminModelRow>, sqlx::Error> {
    let rows: Vec<(String, String, i64, f64, i64, i64)> = sqlx::query_as(
        r#"SELECT model, provider, COUNT(*) as requests,
                  COALESCE(SUM(cost_usdc), 0) as cost,
                  COALESCE(SUM(input_tokens), 0) as input_tokens,
                  COALESCE(SUM(output_tokens), 0) as output_tokens
           FROM spend_logs
           WHERE created_at >= NOW() - make_interval(days => $1)
           GROUP BY model, provider
           ORDER BY cost DESC"#,
    )
    .bind(days)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(model, provider, requests, cost, input_tokens, output_tokens)| AdminModelRow {
                model,
                provider,
                requests,
                cost,
                input_tokens,
                output_tokens,
            },
        )
        .collect())
}

/// Fetch per-day breakdown (platform-wide) over the given number of days.
async fn get_admin_stats_by_day(
    pool: &sqlx::PgPool,
    days: i32,
) -> Result<Vec<AdminDayRow>, sqlx::Error> {
    let rows: Vec<(chrono::NaiveDate, i64, f64)> = sqlx::query_as(
        r#"SELECT DATE(created_at) as date,
                  COUNT(*) as requests,
                  COALESCE(SUM(cost_usdc), 0) as cost
           FROM spend_logs
           WHERE created_at >= NOW() - make_interval(days => $1)
           GROUP BY DATE(created_at)
           ORDER BY date"#,
    )
    .bind(days)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(date, requests, cost)| AdminDayRow {
            date,
            requests,
            cost,
        })
        .collect())
}

/// Fetch top 10 wallets by spend (platform-wide) over the given number of days.
async fn get_admin_top_wallets(
    pool: &sqlx::PgPool,
    days: i32,
) -> Result<Vec<AdminWalletRow>, sqlx::Error> {
    let rows: Vec<(String, i64, f64)> = sqlx::query_as(
        r#"SELECT wallet_address, COUNT(*) as requests,
                  COALESCE(SUM(cost_usdc), 0) as cost
           FROM spend_logs
           WHERE created_at >= NOW() - make_interval(days => $1)
           GROUP BY wallet_address
           ORDER BY cost DESC
           LIMIT 10"#,
    )
    .bind(days)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(wallet, requests, cost)| AdminWalletRow {
            wallet,
            requests,
            cost,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_days() {
        assert_eq!(default_days(), 30);
    }

    #[test]
    fn test_admin_stats_response_serialization() {
        let resp = AdminStatsResponse {
            period_days: 30,
            summary: AdminSummary {
                total_requests: 1247,
                total_cost_usdc: "3.847291".to_string(),
                total_input_tokens: 892_400,
                total_output_tokens: 341_200,
                unique_wallets: 12,
                cache_hit_rate: Some(0.23),
            },
            by_model: vec![AdminModelStats {
                model: "anthropic/claude-sonnet-4-20250514".to_string(),
                provider: "anthropic".to_string(),
                requests: 412,
                cost_usdc: "1.923000".to_string(),
                input_tokens: 310_000,
                output_tokens: 142_000,
            }],
            by_day: vec![AdminDayStats {
                date: "2026-03-11".to_string(),
                requests: 47,
                cost_usdc: "0.142300".to_string(),
                spend: 0.1423,
            }],
            top_wallets: vec![AdminWalletStats {
                wallet: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
                requests: 200,
                cost_usdc: "0.843291".to_string(),
            }],
        };

        let json: serde_json::Value =
            serde_json::to_value(&resp).expect("should serialize to JSON value");

        // Top-level fields
        assert_eq!(json["period_days"], 30);

        // Summary
        assert_eq!(json["summary"]["total_requests"], 1247);
        assert_eq!(json["summary"]["total_cost_usdc"], "3.847291");
        assert_eq!(json["summary"]["total_input_tokens"], 892_400);
        assert_eq!(json["summary"]["total_output_tokens"], 341_200);
        assert_eq!(json["summary"]["unique_wallets"], 12);
        assert_eq!(json["summary"]["cache_hit_rate"], 0.23);

        // by_model array
        assert_eq!(json["by_model"].as_array().unwrap().len(), 1);
        assert_eq!(
            json["by_model"][0]["model"],
            "anthropic/claude-sonnet-4-20250514"
        );
        assert_eq!(json["by_model"][0]["provider"], "anthropic");
        assert_eq!(json["by_model"][0]["requests"], 412);
        assert_eq!(json["by_model"][0]["cost_usdc"], "1.923000");
        assert_eq!(json["by_model"][0]["input_tokens"], 310_000);
        assert_eq!(json["by_model"][0]["output_tokens"], 142_000);

        // by_day array
        assert_eq!(json["by_day"].as_array().unwrap().len(), 1);
        assert_eq!(json["by_day"][0]["date"], "2026-03-11");
        assert_eq!(json["by_day"][0]["requests"], 47);
        assert_eq!(json["by_day"][0]["cost_usdc"], "0.142300");
        assert_eq!(json["by_day"][0]["spend"], 0.1423);

        // top_wallets array
        assert_eq!(json["top_wallets"].as_array().unwrap().len(), 1);
        assert_eq!(
            json["top_wallets"][0]["wallet"],
            "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"
        );
        assert_eq!(json["top_wallets"][0]["requests"], 200);
        assert_eq!(json["top_wallets"][0]["cost_usdc"], "0.843291");
    }

    #[test]
    fn test_admin_stats_response_empty_results() {
        let resp = AdminStatsResponse {
            period_days: 30,
            summary: AdminSummary {
                total_requests: 0,
                total_cost_usdc: "0.000000".to_string(),
                total_input_tokens: 0,
                total_output_tokens: 0,
                unique_wallets: 0,
                cache_hit_rate: None,
            },
            by_model: vec![],
            by_day: vec![],
            top_wallets: vec![],
        };

        let json: serde_json::Value =
            serde_json::to_value(&resp).expect("should serialize to JSON value");

        assert_eq!(json["summary"]["total_requests"], 0);
        assert_eq!(json["summary"]["total_cost_usdc"], "0.000000");
        assert_eq!(json["summary"]["total_input_tokens"], 0);
        assert_eq!(json["summary"]["total_output_tokens"], 0);
        assert_eq!(json["summary"]["unique_wallets"], 0);
        assert!(json["summary"]["cache_hit_rate"].is_null());
        assert!(json["by_model"].as_array().unwrap().is_empty());
        assert!(json["by_day"].as_array().unwrap().is_empty());
        assert!(json["top_wallets"].as_array().unwrap().is_empty());
    }

    #[test]
    fn test_admin_stats_cache_hit_rate_null_when_none() {
        let summary = AdminSummary {
            total_requests: 0,
            total_cost_usdc: "0.000000".to_string(),
            total_input_tokens: 0,
            total_output_tokens: 0,
            unique_wallets: 0,
            cache_hit_rate: None,
        };

        let json = serde_json::to_value(&summary).expect("should serialize");
        assert!(json["cache_hit_rate"].is_null());
    }

    #[test]
    fn test_admin_query_default_days() {
        let query: AdminStatsQuery = serde_json::from_str("{}").expect("should deserialize");
        assert_eq!(query.days, 30);
    }

    #[test]
    fn test_admin_query_custom_days() {
        let query: AdminStatsQuery =
            serde_json::from_str(r#"{"days": 7}"#).expect("should deserialize");
        assert_eq!(query.days, 7);
    }
}
