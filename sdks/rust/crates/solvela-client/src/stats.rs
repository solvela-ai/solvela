//! Wallet spend-statistics response types.
//!
//! These deserialization structs mirror the gateway's `StatsResponse` shape
//! defined in `crates/gateway/src/routes/stats.rs` (the `GET
//! /v1/wallet/{address}/stats` endpoint). They are intentionally SDK-local
//! rather than promoted into `solvela-protocol`: the gateway types derive only
//! `Serialize` (they are response-only on the server), and promoting them would
//! be a cross-crate refactor of the gateway money-path crate beyond the scope
//! of adding this client helper.
//!
//! DRIFT WARNING: because these types are duplicated (gateway serializes, SDK
//! deserializes) they MUST stay byte-compatible with the gateway's field names
//! and types. If the gateway's `stats.rs` field set changes, update this file in
//! lockstep. All `*_cost_usdc` fields are decimal-USDC STRINGS on the wire (never
//! `f64`) — mirror them as `String` here to avoid float drift; do not parse them
//! to `f64` in these deserialization types. Callers that need numeric values
//! should parse the string explicitly with a checked decimal/atomic conversion.

use serde::Deserialize;

/// Top-level wallet spend-statistics response.
///
/// Mirrors `crates/gateway/src/routes/stats.rs::StatsResponse`.
#[derive(Debug, Clone, Deserialize)]
pub struct StatsResponse {
    pub wallet: String,
    pub period_days: i32,
    pub summary: StatsSummary,
    pub by_model: Vec<ModelStats>,
    pub by_day: Vec<DayStats>,
}

/// Aggregated spend summary for the requested period.
///
/// Mirrors `crates/gateway/src/routes/stats.rs::StatsSummary`. All
/// `*_cost_usdc` fields are decimal-USDC strings.
#[derive(Debug, Clone, Deserialize)]
pub struct StatsSummary {
    pub total_requests: i64,
    pub total_cost_usdc: String,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    /// Spend in the last 24h window (today, UTC) for this wallet, decimal USDC.
    pub daily_cost_usdc: String,
    /// Spend in the current calendar month (UTC) for this wallet, decimal USDC.
    pub monthly_cost_usdc: String,
}

/// Per-model spend breakdown.
///
/// Mirrors `crates/gateway/src/routes/stats.rs::ModelStats`.
#[derive(Debug, Clone, Deserialize)]
pub struct ModelStats {
    pub model: String,
    pub requests: i64,
    pub cost_usdc: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
}

/// Per-day spend breakdown.
///
/// Mirrors `crates/gateway/src/routes/stats.rs::DayStats`.
#[derive(Debug, Clone, Deserialize)]
pub struct DayStats {
    pub date: String,
    pub requests: i64,
    pub cost_usdc: String,
}
