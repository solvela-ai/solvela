//! Dashboard API routes for observability.

use axum::extract::Query;
use axum::Json;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct SpendQuery {
    pub wallet: String,
    #[serde(default = "default_days")]
    pub days: u32,
}

fn default_days() -> u32 {
    7
}

#[derive(Debug, Serialize)]
pub struct SpendSummary {
    pub wallet: String,
    pub total_usdc: String,
    pub request_count: u64,
    pub period_days: u32,
    pub by_day: Vec<DailySpend>,
}

#[derive(Debug, Serialize)]
pub struct DailySpend {
    pub date: String,
    pub total_usdc: String,
    pub request_count: u64,
}

/// GET /v1/dashboard/spend?wallet=<address>&days=7
pub async fn spend_summary(Query(params): Query<SpendQuery>) -> Json<SpendSummary> {
    // Stub implementation -- will be backed by usage_logs queries later
    Json(SpendSummary {
        wallet: params.wallet,
        total_usdc: "0.000000".to_string(),
        request_count: 0,
        period_days: params.days,
        by_day: vec![],
    })
}
