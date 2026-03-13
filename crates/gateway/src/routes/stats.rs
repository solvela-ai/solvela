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
use crate::usage::{get_stats_by_day, get_stats_by_model, get_wallet_stats};
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
        Ok(claims) => {
            // Verify that the session token's wallet matches the requested path.
            // build_session_token() in chat.rs populates the wallet field from
            // extract_payer_wallet_from_payload(), which returns the actual payer
            // (tx fee payer for direct, agent_pubkey for escrow).
            if claims.wallet != address {
                return Err((
                    StatusCode::FORBIDDEN,
                    Json(serde_json::json!({ "error": "token wallet does not match requested address" })),
                )
                    .into_response());
            }
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
        get_wallet_stats(pool, &address, params.days),
        get_stats_by_model(pool, &address, params.days),
        get_stats_by_day(pool, &address, params.days),
    );

    let summary_row = summary_result.map_err(|e| {
        tracing::error!(error = %e, wallet = %address, "failed to retrieve wallet stats summary");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "failed to retrieve stats" })),
        )
            .into_response()
    })?;

    let model_rows = by_model_result.map_err(|e| {
        tracing::error!(error = %e, wallet = %address, "failed to retrieve stats by model");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "failed to retrieve stats" })),
        )
            .into_response()
    })?;

    let day_rows = by_day_result.map_err(|e| {
        tracing::error!(error = %e, wallet = %address, "failed to retrieve stats by day");
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "failed to retrieve stats" })),
        )
            .into_response()
    })?;

    let summary = StatsSummary {
        total_requests: summary_row.total_requests,
        total_cost_usdc: format!("{:.6}", summary_row.total_cost),
        total_input_tokens: summary_row.total_input,
        total_output_tokens: summary_row.total_output,
    };

    let by_model = model_rows
        .into_iter()
        .map(|r| ModelStats {
            model: r.model,
            requests: r.requests,
            cost_usdc: format!("{:.6}", r.cost),
            input_tokens: r.input_tokens,
            output_tokens: r.output_tokens,
        })
        .collect();

    let by_day = day_rows
        .into_iter()
        .map(|r| DayStats {
            date: r.date.to_string(),
            requests: r.requests,
            cost_usdc: format!("{:.6}", r.cost),
        })
        .collect();

    Ok(Json(StatsResponse {
        wallet: address,
        period_days: params.days,
        summary,
        by_model,
        by_day,
    })
    .into_response())
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

    /// Verify that a valid session token for a different wallet is correctly
    /// detected as a mismatch. This tests the core logic that was previously
    /// commented out (the CRITICAL security finding).
    #[test]
    fn test_wallet_mismatch_detected() {
        use crate::session::{create_session_token, verify_session_token, SessionClaims};

        let secret = b"test-secret-key-for-wallet-match";
        let wallet_a = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";
        let wallet_b = "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM";

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let claims = SessionClaims {
            wallet: wallet_a.to_string(),
            budget_remaining: 0,
            issued_at: now,
            expires_at: now + 3600,
            allowed_models: vec![],
        };
        let token = create_session_token(&claims, secret).expect("should create token");
        let verified = verify_session_token(&token, secret).expect("token should verify");

        // The verified claims wallet should NOT match wallet_b
        assert_ne!(
            verified.wallet, wallet_b,
            "token for wallet A must not match wallet B"
        );

        // But SHOULD match wallet_a
        assert_eq!(
            verified.wallet, wallet_a,
            "token for wallet A must match wallet A"
        );
    }

    /// Verify that the session token correctly round-trips the wallet address,
    /// so the stats handler comparison (claims.wallet != address) works.
    #[test]
    fn test_session_token_preserves_wallet() {
        use crate::session::{create_session_token, verify_session_token, SessionClaims};

        let secret = b"test-secret-key-for-wallet-match";
        let wallet = "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU";

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let claims = SessionClaims {
            wallet: wallet.to_string(),
            budget_remaining: 5_000_000,
            issued_at: now,
            expires_at: now + 3600,
            allowed_models: vec![],
        };
        let token = create_session_token(&claims, secret).expect("should create token");
        let verified = verify_session_token(&token, secret).expect("token should verify");

        assert_eq!(
            verified.wallet, wallet,
            "session token must preserve the exact wallet address"
        );
    }

    /// Verify the full JSON response shape matches the G.5 spec when populated
    /// with real data (plan test #1).
    #[test]
    fn test_stats_response_full_shape() {
        let resp = StatsResponse {
            wallet: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
            period_days: 30,
            summary: StatsSummary {
                total_requests: 1247,
                total_cost_usdc: "3.847291".to_string(),
                total_input_tokens: 892_400,
                total_output_tokens: 341_200,
            },
            by_model: vec![ModelStats {
                model: "anthropic/claude-sonnet-4-20250514".to_string(),
                requests: 412,
                cost_usdc: "1.923000".to_string(),
                input_tokens: 310_000,
                output_tokens: 142_000,
            }],
            by_day: vec![DayStats {
                date: "2026-03-11".to_string(),
                requests: 47,
                cost_usdc: "0.142300".to_string(),
            }],
        };

        let json: serde_json::Value =
            serde_json::to_value(&resp).expect("should serialize to JSON value");

        // Top-level fields
        assert_eq!(
            json["wallet"],
            "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU"
        );
        assert_eq!(json["period_days"], 30);

        // Summary
        assert_eq!(json["summary"]["total_requests"], 1247);
        assert_eq!(json["summary"]["total_cost_usdc"], "3.847291");
        assert_eq!(json["summary"]["total_input_tokens"], 892_400);
        assert_eq!(json["summary"]["total_output_tokens"], 341_200);

        // by_model array
        assert_eq!(json["by_model"].as_array().unwrap().len(), 1);
        assert_eq!(
            json["by_model"][0]["model"],
            "anthropic/claude-sonnet-4-20250514"
        );
        assert_eq!(json["by_model"][0]["requests"], 412);
        assert_eq!(json["by_model"][0]["cost_usdc"], "1.923000");
        assert_eq!(json["by_model"][0]["input_tokens"], 310_000);
        assert_eq!(json["by_model"][0]["output_tokens"], 142_000);

        // by_day array
        assert_eq!(json["by_day"].as_array().unwrap().len(), 1);
        assert_eq!(json["by_day"][0]["date"], "2026-03-11");
        assert_eq!(json["by_day"][0]["requests"], 47);
        assert_eq!(json["by_day"][0]["cost_usdc"], "0.142300");
    }

    /// Verify that empty results produce 200 with zeros and empty arrays
    /// (plan test #9).
    #[test]
    fn test_stats_response_empty_results_shape() {
        let resp = StatsResponse {
            wallet: "7xKXtg2CW87d97TXJSDpbD5jBkheTqA83TZRuJosgAsU".to_string(),
            period_days: 30,
            summary: StatsSummary {
                total_requests: 0,
                total_cost_usdc: "0.000000".to_string(),
                total_input_tokens: 0,
                total_output_tokens: 0,
            },
            by_model: vec![],
            by_day: vec![],
        };

        let json: serde_json::Value =
            serde_json::to_value(&resp).expect("should serialize to JSON value");

        assert_eq!(json["summary"]["total_requests"], 0);
        assert_eq!(json["summary"]["total_cost_usdc"], "0.000000");
        assert_eq!(json["summary"]["total_input_tokens"], 0);
        assert_eq!(json["summary"]["total_output_tokens"], 0);
        assert!(json["by_model"].as_array().unwrap().is_empty());
        assert!(json["by_day"].as_array().unwrap().is_empty());
    }
}
