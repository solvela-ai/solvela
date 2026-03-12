//! Per-wallet spend statistics endpoint.
//!
//! `GET /v1/wallet/{address}/stats?days=30` returns aggregated spend data
//! grouped by summary, model, and day. Requires a valid session token.

use std::sync::Arc;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::session::verify_session_token;
use crate::AppState;

/// Base58 character set for wallet address validation.
const BASE58_ALPHABET: &str = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

/// Query parameters for the stats endpoint.
#[derive(Debug, Deserialize)]
pub struct StatsQuery {
    #[serde(default = "default_days")]
    pub days: i32,
}

fn default_days() -> i32 {
    30
}

/// Top-level stats response.
#[derive(Debug, Serialize)]
pub struct StatsResponse {
    pub wallet: String,
    pub period_days: i32,
    pub summary: StatsSummary,
    pub by_model: Vec<ModelStats>,
    pub by_day: Vec<DayStats>,
}

/// Aggregated spend summary for the period.
#[derive(Debug, Serialize)]
pub struct StatsSummary {
    pub total_requests: i64,
    pub total_cost_usdc: String,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
}

/// Per-model breakdown.
#[derive(Debug, Serialize)]
pub struct ModelStats {
    pub model: String,
    pub requests: i64,
    pub cost_usdc: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// Per-day breakdown.
#[derive(Debug, Serialize)]
pub struct DayStats {
    pub date: String,
    pub requests: i64,
    pub cost_usdc: String,
}

/// Validate that a string looks like a Solana wallet address.
///
/// Checks length (32-44 chars) and base58 character set.
pub fn is_valid_wallet_address(address: &str) -> bool {
    let len = address.len();
    (32..=44).contains(&len) && address.chars().all(|c| BASE58_ALPHABET.contains(c))
}

/// `GET /v1/wallet/:address/stats`
///
/// Returns spend statistics for the given wallet over the requested period.
pub async fn wallet_stats(
    State(state): State<Arc<AppState>>,
    Path(address): Path<String>,
    Query(params): Query<StatsQuery>,
    headers: HeaderMap,
) -> Result<Response, Response> {
    // Validate wallet address format
    if !is_valid_wallet_address(&address) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "invalid wallet address format" })),
        )
            .into_response());
    }

    // Validate days parameter
    if params.days < 1 || params.days > 365 {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": format!("days must be between 1 and 365, got {}", params.days)
            })),
        )
            .into_response());
    }

    // Auth: require a valid session token
    let token = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));

    let token = match token {
        Some(t) => t,
        None => {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({ "error": "missing or invalid Authorization header" })),
            )
                .into_response());
        }
    };

    match verify_session_token(token, &state.session_secret) {
        Ok(_claims) => {
            // TODO: Wallet matching is disabled because build_session_token() currently
            // populates the wallet field from payload.accepted.pay_to (the gateway's
            // recipient wallet), not the payer's wallet. Once extract_payment_info()
            // returns the actual payer wallet address, re-enable this check:
            //
            // if claims.wallet != address {
            //     return Err((
            //         StatusCode::FORBIDDEN,
            //         Json(serde_json::json!({ "error": "token wallet does not match path" })),
            //     ).into_response());
            // }
        }
        Err(_) => {
            return Err((
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({ "error": "invalid or expired session token" })),
            )
                .into_response());
        }
    }

    // Check for database availability
    let pool = match &state.db_pool {
        Some(pool) => pool,
        None => {
            return Err((
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({ "error": "stats unavailable — no database configured" })),
            )
                .into_response());
        }
    };

    // Run the three queries concurrently
    let (summary_result, by_model_result, by_day_result) = tokio::join!(
        query_summary(pool, &address, params.days),
        query_by_model(pool, &address, params.days),
        query_by_day(pool, &address, params.days),
    );

    let summary = summary_result.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("database error: {e}") })),
        )
            .into_response()
    })?;

    let by_model = by_model_result.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("database error: {e}") })),
        )
            .into_response()
    })?;

    let by_day = by_day_result.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("database error: {e}") })),
        )
            .into_response()
    })?;

    Ok(Json(StatsResponse {
        wallet: address,
        period_days: params.days,
        summary,
        by_model,
        by_day,
    })
    .into_response())
}

/// Query aggregate summary for a wallet over a time period.
async fn query_summary(
    pool: &sqlx::PgPool,
    wallet: &str,
    days: i32,
) -> Result<StatsSummary, sqlx::Error> {
    let row: (i64, f64, i64, i64) = sqlx::query_as(
        r#"SELECT COUNT(*) as total_requests,
                  COALESCE(SUM(cost_usdc), 0) as total_cost,
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

    Ok(StatsSummary {
        total_requests: row.0,
        total_cost_usdc: format!("{:.6}", row.1),
        total_input_tokens: row.2,
        total_output_tokens: row.3,
    })
}

/// Query per-model breakdown for a wallet over a time period.
async fn query_by_model(
    pool: &sqlx::PgPool,
    wallet: &str,
    days: i32,
) -> Result<Vec<ModelStats>, sqlx::Error> {
    let rows: Vec<(String, i64, f64, i64, i64)> = sqlx::query_as(
        r#"SELECT model, COUNT(*) as requests,
                  COALESCE(SUM(cost_usdc), 0) as cost,
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
            |(model, requests, cost, input_tokens, output_tokens)| ModelStats {
                model,
                requests,
                cost_usdc: format!("{cost:.6}"),
                input_tokens,
                output_tokens,
            },
        )
        .collect())
}

/// Query per-day breakdown for a wallet over a time period.
async fn query_by_day(
    pool: &sqlx::PgPool,
    wallet: &str,
    days: i32,
) -> Result<Vec<DayStats>, sqlx::Error> {
    let rows: Vec<(chrono::NaiveDate, i64, f64)> = sqlx::query_as(
        r#"SELECT DATE(created_at) as date,
                  COUNT(*) as requests,
                  COALESCE(SUM(cost_usdc), 0) as cost
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
        .map(|(date, requests, cost)| DayStats {
            date: date.to_string(),
            requests,
            cost_usdc: format!("{cost:.6}"),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_wallet_addresses() {
        // Typical Solana address (44 chars)
        assert!(is_valid_wallet_address(
            "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"
        ));
        // System program (32 chars of '1')
        assert!(is_valid_wallet_address("11111111111111111111111111111111"));
    }

    #[test]
    fn test_invalid_wallet_addresses() {
        // Too short
        assert!(!is_valid_wallet_address("abc"));
        // Too long
        assert!(!is_valid_wallet_address(&"A".repeat(45)));
        // Invalid characters (0, O, I, l are not in base58)
        assert!(!is_valid_wallet_address(
            "0xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAs"
        ));
        // Empty
        assert!(!is_valid_wallet_address(""));
    }

    #[test]
    fn test_default_days() {
        assert_eq!(default_days(), 30);
    }

    #[test]
    fn test_stats_response_serialization() {
        let resp = StatsResponse {
            wallet: "test_wallet".to_string(),
            period_days: 7,
            summary: StatsSummary {
                total_requests: 0,
                total_cost_usdc: "0.000000".to_string(),
                total_input_tokens: 0,
                total_output_tokens: 0,
            },
            by_model: vec![],
            by_day: vec![],
        };
        let json = serde_json::to_string(&resp).expect("should serialize");
        assert!(json.contains("test_wallet"));
        assert!(json.contains("period_days"));
    }
}
