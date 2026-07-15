//! Admin tenant-budget provisioning endpoint.
//!
//! `PUT /v1/wallet/{wallet}/tenants/{tenant}` upserts a `tenant_budgets` row
//! (migration 011) so an external provisioning system can create per-tenant
//! caps over HTTP instead of hand-run `ON CONFLICT` SQL. Idempotent by
//! design: the caller retries under Stripe webhook redelivery, so "already
//! exists" is SUCCESS — a fresh row returns 201, a replayed/updating PUT
//! returns 200. Protected by the admin token (same gate shape as
//! [`crate::routes::admin_stats`]: hidden 404 when unconfigured, 401 on
//! mismatch).
//!
//! Unlike the paid hot path (CLAUDE.md rule #9), the DB write here is
//! deliberately AWAITED: the endpoint's whole contract is "when it returns
//! 2xx the row durably exists before first traffic", so a fire-and-forget
//! write would let a 2xx race the insert. After a successful write the
//! `tenant_budget:{wallet}:{tenant}` Redis cache entry (60s TTL, including
//! the negative "none" sentinel — see `usage::get_tenant_budget_config`) is
//! deleted so a fresh provision is not rejected as unprovisioned under
//! `require_tenant` for up to a TTL. Cache-bust failure never fails the
//! request — the TTL bounds the staleness.

use std::str::FromStr;
use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;

use solvela_x402::solana_types::Pubkey;

use crate::routes::chat::validate_tenant;
use crate::AppState;

/// Request body for tenant-budget provisioning. Each window is independently
/// nullable (`null` = no cap on that window); values are 6-dp USDC decimal
/// strings, the same contract as the stats read side.
#[derive(Debug, Deserialize)]
pub struct ProvisionTenantBudgetRequest {
    #[serde(default)]
    pub hourly_limit_usdc: Option<String>,
    #[serde(default)]
    pub daily_limit_usdc: Option<String>,
    #[serde(default)]
    pub monthly_limit_usdc: Option<String>,
}

/// Why a cap string was rejected. Fail-closed (solvela-fintech): anything
/// that is not a plain non-negative decimal within the `DECIMAL(18,6)`
/// column bounds is refused — never coerced, clamped, or defaulted to $0.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
pub(crate) enum CapParseError {
    #[error("must not be empty")]
    Empty,
    #[error(
        "must be a plain non-negative decimal number \
         (digits with an optional `.` fraction; no sign, exponent, NaN, or infinity)"
    )]
    Malformed,
    #[error("must have at most 6 decimal places")]
    TooManyDecimals,
    #[error("integer part must not exceed 12 digits (column is DECIMAL(18,6))")]
    IntegerPartTooLong,
}

/// Parse a USDC cap string into atomic units (integer micro-USDC).
///
/// All-integer parsing — no f64 anywhere on this path (solvela-fintech §1),
/// so NaN/Inf/negative/exponent forms are structurally unrepresentable
/// rather than range-checked after the fact. Accepted grammar:
/// `digits{1,12}` optionally followed by `.` + `digits{1,6}`.
///
/// Max accepted value is `999999999999.999999` USDC =
/// `999_999_999_999_999_999` atomic, which fits u64 with no overflow
/// (12 integer digits × 10^6 + fraction < 2^63).
pub(crate) fn parse_cap_usdc(value: &str) -> Result<u64, CapParseError> {
    if value.is_empty() {
        return Err(CapParseError::Empty);
    }

    let (int_part, frac_part) = match value.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (value, None),
    };

    if int_part.is_empty() || !int_part.bytes().all(|b| b.is_ascii_digit()) {
        return Err(CapParseError::Malformed);
    }
    if int_part.len() > 12 {
        return Err(CapParseError::IntegerPartTooLong);
    }

    let frac_atomic: u64 = match frac_part {
        None => 0,
        Some(f) => {
            if f.is_empty() || !f.bytes().all(|b| b.is_ascii_digit()) {
                return Err(CapParseError::Malformed);
            }
            if f.len() > 6 {
                return Err(CapParseError::TooManyDecimals);
            }
            // Right-pad to exactly 6 digits: "5" → "500000" micro-USDC.
            format!("{f:0<6}")
                .parse::<u64>()
                .map_err(|_| CapParseError::Malformed)?
        }
    };

    let int_units: u64 = int_part
        .parse::<u64>()
        .map_err(|_| CapParseError::Malformed)?;

    Ok(int_units * 1_000_000 + frac_atomic)
}

/// Format atomic micro-USDC as the canonical 6-dp decimal string used across
/// the stats read side (`{:.6}`-style, e.g. `5_500_000` → `"5.500000"`).
pub(crate) fn format_cap_6dp(atomic: u64) -> String {
    format!("{}.{:06}", atomic / 1_000_000, atomic % 1_000_000)
}

/// Parse one optional cap field, mapping a parse failure to a descriptive
/// 400 that names the offending field.
#[allow(clippy::result_large_err)]
fn parse_cap_field(value: Option<&str>, field: &str) -> Result<Option<u64>, Response> {
    match value {
        None => Ok(None),
        Some(s) => parse_cap_usdc(s).map(Some).map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("invalid {field}: {e}") })),
            )
                .into_response()
        }),
    }
}

/// `PUT /v1/wallet/{wallet}/tenants/{tenant}`
///
/// Idempotently provision (upsert) a `tenant_budgets` row. Returns 201 with
/// the stored caps when the row was created, 200 when an existing row was
/// updated. Protected by admin token (Bearer auth).
pub async fn provision_tenant_budget(
    State(state): State<Arc<AppState>>,
    Path((wallet, tenant)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<ProvisionTenantBudgetRequest>,
) -> Result<Response, Response> {
    // Gate behind admin token — if not configured, hide the endpoint entirely
    // (same shape as `routes::admin_stats`).
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
        .is_some_and(|token| admin_token.verify(token.as_bytes()));

    if !authorized {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response());
    }

    // Validate the wallet path param: must be a base58 32-byte Solana pubkey.
    // Fail closed — a typo'd wallet must not provision a row that enforcement
    // (keyed on the paying wallet's address string) will never match.
    if Pubkey::from_str(&wallet).is_err() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "invalid wallet: must be a base58-encoded 32-byte Solana pubkey"
            })),
        )
            .into_response());
    }

    // Validate the tenant path param with the SAME rules the chat path uses
    // for tagging (`validate_tenant`: 1-64 chars of [a-zA-Z0-9._-]). On the
    // chat path an invalid tag means "proceed untagged"; HERE it is a hard
    // 400 — a provisioned row under a tag the chat path can never produce
    // would be dead config.
    if validate_tenant(&tenant).is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "invalid tenant: must be 1-64 characters of [a-zA-Z0-9._-]"
            })),
        )
            .into_response());
    }

    // Fail-closed cap parsing (all three windows independently nullable).
    let hourly = parse_cap_field(body.hourly_limit_usdc.as_deref(), "hourly_limit_usdc")?;
    let daily = parse_cap_field(body.daily_limit_usdc.as_deref(), "daily_limit_usdc")?;
    let monthly = parse_cap_field(body.monthly_limit_usdc.as_deref(), "monthly_limit_usdc")?;

    if hourly.is_none() && daily.is_none() && monthly.is_none() {
        // Accepted (mirrors the hand-run SQL path: the row's existence is what
        // `require_tenant` gates on), but worth flagging: this tenant spends
        // uncapped on every window.
        tracing::warn!(
            wallet = %wallet,
            tenant = %tenant,
            "provisioning tenant budget with all three cap windows null (uncapped)"
        );
    }

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

    // Normalized 6-dp strings: bound with a ::numeric cast so Postgres parses
    // the exact decimal (never an f64 in between), and echoed verbatim in the
    // response body.
    let hourly_str = hourly.map(format_cap_6dp);
    let daily_str = daily.map(format_cap_6dp);
    let monthly_str = monthly.map(format_cap_6dp);

    // NOT fire-and-forget (deliberate exception to CLAUDE.md rule #9, which
    // covers the paid hot path): the 2xx must mean the row durably exists.
    // `updated_at` is maintained by the migration-011 trigger — never set here.
    let result: Result<(bool,), sqlx::Error> = sqlx::query_as(
        r#"INSERT INTO tenant_budgets
             (wallet_address, tenant, hourly_limit_usdc, daily_limit_usdc, monthly_limit_usdc)
           VALUES ($1, $2, $3::numeric, $4::numeric, $5::numeric)
           ON CONFLICT (wallet_address, tenant) DO UPDATE SET
               hourly_limit_usdc = EXCLUDED.hourly_limit_usdc,
               daily_limit_usdc = EXCLUDED.daily_limit_usdc,
               monthly_limit_usdc = EXCLUDED.monthly_limit_usdc
           RETURNING (xmax = 0) AS inserted"#,
    )
    .bind(&wallet)
    .bind(&tenant)
    .bind(hourly_str.as_deref())
    .bind(daily_str.as_deref())
    .bind(monthly_str.as_deref())
    .fetch_one(pool)
    .await;

    let inserted = match result {
        Ok((inserted,)) => inserted,
        Err(e) => {
            tracing::error!(
                wallet = %wallet,
                tenant = %tenant,
                error = %e,
                "failed to provision tenant budget"
            );
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": "failed to provision tenant budget" })),
            )
                .into_response());
        }
    };

    // Bust the budget-config cache (60s TTL, including the negative "none"
    // sentinel) so the fresh provision takes effect immediately under
    // `require_tenant`. Failure here must NOT fail the request — the TTL
    // bounds the staleness.
    if let Some(redis_client) = state.usage.redis_client() {
        match redis_client.get_multiplexed_async_connection().await {
            Ok(mut conn) => {
                let cache_key = format!("tenant_budget:{wallet}:{tenant}");
                if let Err(e) = redis::cmd("DEL")
                    .arg(&cache_key)
                    .query_async::<()>(&mut conn)
                    .await
                {
                    tracing::warn!(
                        cache_key = %cache_key,
                        error = %e,
                        "failed to invalidate tenant budget cache (stale for up to its TTL)"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "Redis unavailable for tenant budget cache invalidation"
                );
            }
        }
    }

    tracing::info!(
        wallet = %wallet,
        tenant = %tenant,
        created = inserted,
        "tenant budget provisioned"
    );

    let status = if inserted {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((
        status,
        Json(json!({
            "wallet_address": wallet,
            "tenant": tenant,
            "hourly_limit_usdc": hourly_str,
            "daily_limit_usdc": daily_str,
            "monthly_limit_usdc": monthly_str,
            "created": inserted,
        })),
    )
        .into_response())
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // parse_cap_usdc — accepted forms
    // -----------------------------------------------------------------------

    #[test]
    fn parse_cap_plain_integer() {
        assert_eq!(parse_cap_usdc("5"), Ok(5_000_000));
    }

    #[test]
    fn parse_cap_zero() {
        assert_eq!(parse_cap_usdc("0"), Ok(0));
    }

    #[test]
    fn parse_cap_zero_six_dp() {
        assert_eq!(parse_cap_usdc("0.000000"), Ok(0));
    }

    #[test]
    fn parse_cap_full_six_decimals() {
        assert_eq!(parse_cap_usdc("5.000000"), Ok(5_000_000));
    }

    #[test]
    fn parse_cap_short_fraction_right_padded() {
        // "5.5" is 5.500000, NOT 5.000005 — fraction pads on the right.
        assert_eq!(parse_cap_usdc("5.5"), Ok(5_500_000));
    }

    #[test]
    fn parse_cap_smallest_atomic_unit() {
        assert_eq!(parse_cap_usdc("0.000001"), Ok(1));
    }

    #[test]
    fn parse_cap_max_decimal_18_6_magnitude() {
        // 12 integer digits + 6 fraction digits — the DECIMAL(18,6) ceiling.
        assert_eq!(
            parse_cap_usdc("999999999999.999999"),
            Ok(999_999_999_999_999_999)
        );
    }

    #[test]
    fn parse_cap_leading_zeros_normalize() {
        assert_eq!(parse_cap_usdc("007.25"), Ok(7_250_000));
    }

    // -----------------------------------------------------------------------
    // parse_cap_usdc — rejected forms (fail closed, never coerce)
    // -----------------------------------------------------------------------

    #[test]
    fn parse_cap_rejects_empty() {
        assert_eq!(parse_cap_usdc(""), Err(CapParseError::Empty));
    }

    #[test]
    fn parse_cap_rejects_negative() {
        assert_eq!(parse_cap_usdc("-5"), Err(CapParseError::Malformed));
        assert_eq!(parse_cap_usdc("-5.000000"), Err(CapParseError::Malformed));
        assert_eq!(parse_cap_usdc("-0.000001"), Err(CapParseError::Malformed));
    }

    #[test]
    fn parse_cap_rejects_explicit_plus_sign() {
        assert_eq!(parse_cap_usdc("+5"), Err(CapParseError::Malformed));
    }

    #[test]
    fn parse_cap_rejects_non_numeric() {
        assert_eq!(parse_cap_usdc("abc"), Err(CapParseError::Malformed));
        assert_eq!(parse_cap_usdc("5x"), Err(CapParseError::Malformed));
        assert_eq!(parse_cap_usdc("0x10"), Err(CapParseError::Malformed));
    }

    #[test]
    fn parse_cap_rejects_nan_and_infinity_spellings() {
        for s in ["NaN", "nan", "inf", "Inf", "infinity", "Infinity", "-inf"] {
            assert_eq!(parse_cap_usdc(s), Err(CapParseError::Malformed), "{s}");
        }
    }

    #[test]
    fn parse_cap_rejects_exponent_notation() {
        assert_eq!(parse_cap_usdc("1e6"), Err(CapParseError::Malformed));
        assert_eq!(parse_cap_usdc("1E6"), Err(CapParseError::Malformed));
        assert_eq!(parse_cap_usdc("1.5e2"), Err(CapParseError::Malformed));
    }

    #[test]
    fn parse_cap_rejects_seven_decimal_places() {
        assert_eq!(
            parse_cap_usdc("5.0000001"),
            Err(CapParseError::TooManyDecimals)
        );
        // Even an all-zero 7th place is rejected — the contract is 6dp.
        assert_eq!(
            parse_cap_usdc("5.0000000"),
            Err(CapParseError::TooManyDecimals)
        );
    }

    #[test]
    fn parse_cap_rejects_thirteen_integer_digits() {
        assert_eq!(
            parse_cap_usdc("1234567890123"),
            Err(CapParseError::IntegerPartTooLong)
        );
        assert_eq!(
            parse_cap_usdc("1000000000000.000001"),
            Err(CapParseError::IntegerPartTooLong)
        );
        // 13 leading-zero digits are still 13 digits: reject rather than
        // special-case normalization at the boundary.
        assert_eq!(
            parse_cap_usdc("0000000000001"),
            Err(CapParseError::IntegerPartTooLong)
        );
    }

    #[test]
    fn parse_cap_rejects_bare_or_dangling_dot() {
        assert_eq!(parse_cap_usdc("."), Err(CapParseError::Malformed));
        assert_eq!(parse_cap_usdc("5."), Err(CapParseError::Malformed));
        assert_eq!(parse_cap_usdc(".5"), Err(CapParseError::Malformed));
    }

    #[test]
    fn parse_cap_rejects_multiple_dots() {
        assert_eq!(parse_cap_usdc("1.2.3"), Err(CapParseError::Malformed));
        assert_eq!(parse_cap_usdc("5..0"), Err(CapParseError::Malformed));
    }

    #[test]
    fn parse_cap_rejects_whitespace() {
        assert_eq!(parse_cap_usdc(" 5"), Err(CapParseError::Malformed));
        assert_eq!(parse_cap_usdc("5 "), Err(CapParseError::Malformed));
        assert_eq!(parse_cap_usdc("5 .0"), Err(CapParseError::Malformed));
    }

    #[test]
    fn parse_cap_rejects_grouping_and_locale_separators() {
        assert_eq!(parse_cap_usdc("1,500"), Err(CapParseError::Malformed));
        assert_eq!(parse_cap_usdc("1_500"), Err(CapParseError::Malformed));
    }

    #[test]
    fn parse_cap_rejects_non_ascii_digits() {
        // Arabic-Indic five: char::is_numeric would accept it; is_ascii_digit
        // must not.
        assert_eq!(parse_cap_usdc("٥"), Err(CapParseError::Malformed));
    }

    // -----------------------------------------------------------------------
    // format_cap_6dp
    // -----------------------------------------------------------------------

    #[test]
    fn format_cap_whole_amount() {
        assert_eq!(format_cap_6dp(5_000_000), "5.000000");
    }

    #[test]
    fn format_cap_zero() {
        assert_eq!(format_cap_6dp(0), "0.000000");
    }

    #[test]
    fn format_cap_single_atomic_unit_zero_pads() {
        assert_eq!(format_cap_6dp(1), "0.000001");
    }

    #[test]
    fn format_cap_max_magnitude() {
        assert_eq!(
            format_cap_6dp(999_999_999_999_999_999),
            "999999999999.999999"
        );
    }

    #[test]
    fn parse_then_format_roundtrips_and_normalizes() {
        for (input, canonical) in [
            ("5.000000", "5.000000"),
            ("5.5", "5.500000"),
            ("5", "5.000000"),
            ("007.25", "7.250000"),
            ("0.000001", "0.000001"),
            ("999999999999.999999", "999999999999.999999"),
        ] {
            assert_eq!(
                format_cap_6dp(parse_cap_usdc(input).expect("valid input")),
                canonical,
                "input {input:?}"
            );
        }
    }
}
