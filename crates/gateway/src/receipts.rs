//! Client-facing payment receipts (settlement-platform P2).
//!
//! A receipt is the client-facing audit artifact for a PAID request: who
//! paid, what was actually charged (atomic USDC is canonical — see the
//! solvela-fintech rules), which x402 scheme settled it, and — for
//! vendor-settled marketplace services (settlement-platform P1) — the vendor
//! settlement leg plus Solvela's 5% fee receivable.
//!
//! Receipts are written **fire-and-forget** (`tokio::spawn`, never awaited on
//! the hot path) at the same points where spend logging fires: every paid
//! completion that writes a spend row writes a receipt row (the #541
//! streaming-spend-log bug class is the cautionary tale). They are served
//! back via `GET /v1/receipts/{receipt_id}`.
//!
//! The receipt id is **capability-by-unguessable-UUID**: knowing the UUIDv4
//! IS the authorization to read the receipt (agents have no other auth on the
//! public path). There is deliberately no listing/enumeration endpoint, and
//! unknown/malformed ids return one identical 404.
//!
//! Receipts exist only when PostgreSQL is configured (Architectural Rule
//! #12): with no `DATABASE_URL` the gateway never emits the
//! `x-solvela-receipt` header — a header promising an unfetchable receipt
//! would be a lie — and the GET route returns an honest 503.

use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::Row;
use tracing::{error, info};
use uuid::Uuid;

use crate::usage::VendorSettlement;

/// Response header advertising the receipt path on paid responses.
pub const RECEIPT_HEADER: &str = "x-solvela-receipt";

/// Public path at which a receipt can be fetched.
pub fn receipt_path(id: &Uuid) -> String {
    format!("/v1/receipts/{id}")
}

/// Format an atomic-USDC amount (6 decimals) as a decimal string using pure
/// integer math — no f64 anywhere near the money value.
pub fn atomic_to_usdc_string(atomic: u64) -> String {
    format!("{}.{:06}", atomic / 1_000_000, atomic % 1_000_000)
}

/// Everything needed to persist one receipt row. All amounts are atomic USDC.
///
/// `amount_paid_atomic` mirrors the spend ledger (discounted on an escrow
/// semantic-cache hit, full otherwise); the breakdown triple is the pricing
/// computation that produced the bill.
#[derive(Debug, Clone)]
pub struct ReceiptRecord {
    pub receipt_id: Uuid,
    /// Model id (chat path) or marketplace service id (proxy path).
    pub model: String,
    /// x402 scheme that settled the payment (`exact` / `escrow`).
    pub payment_scheme: String,
    /// Payment transaction reference as recorded on the spend ledger.
    pub tx_signature: Option<String>,
    pub payer_wallet: String,
    pub amount_paid_atomic: u64,
    pub provider_cost_atomic: u64,
    pub platform_fee_atomic: u64,
    pub total_atomic: u64,
    /// Vendor settlement + fee receivable for vendor-settled marketplace
    /// requests; `None` on every other path.
    pub vendor: Option<VendorSettlement>,
}

/// Errors from reading a receipt back.
#[derive(Debug, thiserror::Error)]
pub enum ReceiptError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),

    /// The stored row violates a receipt invariant (negative amount, partial
    /// vendor fields). Fail closed: never serve a half-truth receipt.
    #[error("stored receipt is corrupt: {0}")]
    Corrupt(String),
}

/// Wire shape returned by `GET /v1/receipts/{receipt_id}`.
///
/// Atomic integers are canonical; the `*_usdc` decimal strings are derived
/// from them at read time via [`atomic_to_usdc_string`].
#[derive(Debug, Clone, Serialize)]
pub struct Receipt {
    pub receipt_id: Uuid,
    pub created_at: DateTime<Utc>,
    pub model: String,
    pub payment_scheme: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx_signature: Option<String>,
    pub payer_wallet: String,
    pub amount_paid_atomic: u64,
    pub amount_paid_usdc: String,
    pub cost_breakdown: ReceiptCostBreakdown,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vendor: Option<ReceiptVendor>,
}

/// Agent-facing cost breakdown of a receipt (what the agent was quoted /
/// billed against — rule #5).
#[derive(Debug, Clone, Serialize)]
pub struct ReceiptCostBreakdown {
    pub provider_cost_atomic: u64,
    pub provider_cost_usdc: String,
    pub platform_fee_atomic: u64,
    pub platform_fee_usdc: String,
    pub total_atomic: u64,
    pub total_usdc: String,
    pub currency: String,
}

/// Vendor-settlement evidence on a receipt (settlement-platform P1): the
/// amount settled on-chain to the vendor wallet and Solvela's off-chain 5%
/// fee receivable.
#[derive(Debug, Clone, Serialize)]
pub struct ReceiptVendor {
    pub vendor_wallet: String,
    pub settled_atomic: u64,
    pub settled_usdc: String,
    pub fee_receivable_atomic: u64,
    pub fee_receivable_usdc: String,
}

/// All BIGINT binds for one receipt row, converted fail-closed from `u64`.
struct ReceiptBinds {
    amount_paid: i64,
    provider_cost: i64,
    platform_fee: i64,
    total: i64,
    vendor: Option<(String, i64, i64)>,
}

/// Convert every amount to `i64` up front, or `None` if any exceeds the
/// BIGINT range. Running this BEFORE the header is emitted means the header
/// is only ever advertised for a row that can actually be written.
fn receipt_binds(record: &ReceiptRecord) -> Option<ReceiptBinds> {
    let vendor = match &record.vendor {
        None => None,
        Some(v) => Some((
            v.vendor_wallet.clone(),
            i64::try_from(v.settled_atomic).ok()?,
            i64::try_from(v.fee_receivable_atomic).ok()?,
        )),
    };
    Some(ReceiptBinds {
        amount_paid: i64::try_from(record.amount_paid_atomic).ok()?,
        provider_cost: i64::try_from(record.provider_cost_atomic).ok()?,
        platform_fee: i64::try_from(record.platform_fee_atomic).ok()?,
        total: i64::try_from(record.total_atomic).ok()?,
        vendor,
    })
}

/// Persist a receipt fire-and-forget and return its public path, or `None`
/// when no receipt will be retrievable (no DB pool, or an amount the BIGINT
/// ledger cannot record — fail closed, never advertise an unfetchable
/// receipt).
///
/// Emits a synchronous `info!("receipt logged")` event before spawning the
/// write, mirroring `UsageTracker::log_spend`, so integration tests observe
/// the production write through the real route.
pub fn record_receipt(pool: Option<&sqlx::PgPool>, record: ReceiptRecord) -> Option<String> {
    let pool = pool?;
    let Some(binds) = receipt_binds(&record) else {
        metrics::counter!("solvela_receipt_write_failures_total", "reason" => "amount_overflow")
            .increment(1);
        error!(
            receipt_id = %record.receipt_id,
            model = %record.model,
            amount_paid_atomic = record.amount_paid_atomic,
            total_atomic = record.total_atomic,
            "receipt amounts exceed the BIGINT range — receipt not written, header not emitted"
        );
        return None;
    };
    let created_at = Utc::now();

    info!(
        receipt_id = %record.receipt_id,
        model = %record.model,
        payment_scheme = %record.payment_scheme,
        payer_wallet = %record.payer_wallet,
        amount_paid_atomic = record.amount_paid_atomic,
        provider_cost_atomic = record.provider_cost_atomic,
        platform_fee_atomic = record.platform_fee_atomic,
        total_atomic = record.total_atomic,
        vendor_wallet = record
            .vendor
            .as_ref()
            .map(|v| v.vendor_wallet.as_str())
            .unwrap_or("none"),
        vendor_settled_atomic = record.vendor.as_ref().map_or(0, |v| v.settled_atomic),
        vendor_fee_receivable_atomic =
            record.vendor.as_ref().map_or(0, |v| v.fee_receivable_atomic),
        "receipt logged"
    );

    let path = receipt_path(&record.receipt_id);
    let pool = pool.clone();
    tokio::spawn(async move {
        let result = sqlx::query(
            r#"INSERT INTO receipts (id, created_at, model, payment_scheme, tx_signature, payer_wallet, amount_paid_atomic, provider_cost_atomic, platform_fee_atomic, total_atomic, vendor_wallet, vendor_settled_atomic, vendor_fee_receivable_atomic)
               VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)"#,
        )
        .bind(record.receipt_id)
        .bind(created_at)
        .bind(&record.model)
        .bind(&record.payment_scheme)
        .bind(&record.tx_signature)
        .bind(&record.payer_wallet)
        .bind(binds.amount_paid)
        .bind(binds.provider_cost)
        .bind(binds.platform_fee)
        .bind(binds.total)
        .bind(binds.vendor.as_ref().map(|(w, _, _)| w.clone()))
        .bind(binds.vendor.as_ref().map(|(_, s, _)| *s))
        .bind(binds.vendor.as_ref().map(|(_, _, f)| *f))
        .execute(&pool)
        .await;

        if let Err(e) = result {
            // The header already promised this receipt; a lost write means a
            // 404 on a legitimately-issued id. Emit every field needed to
            // reconstruct the receipt by hand and count it for alerting.
            metrics::counter!("solvela_receipt_write_failures_total", "reason" => "db_error")
                .increment(1);
            error!(
                error = %e,
                receipt_id = %record.receipt_id,
                model = %record.model,
                payment_scheme = %record.payment_scheme,
                payer_wallet = %record.payer_wallet,
                amount_paid_atomic = record.amount_paid_atomic,
                provider_cost_atomic = record.provider_cost_atomic,
                platform_fee_atomic = record.platform_fee_atomic,
                total_atomic = record.total_atomic,
                "failed to write receipt to database — advertised receipt will 404"
            );
        }
    });
    Some(path)
}

/// Insert the [`RECEIPT_HEADER`] onto `response` when a receipt path exists.
/// No-op when `path` is `None` (no DB / receipt skipped) — the header is only
/// ever emitted for a retrievable receipt.
pub fn insert_receipt_header(response: &mut axum::response::Response, path: &Option<String>) {
    let Some(path) = path else { return };
    let Ok(value) = axum::http::HeaderValue::from_str(path) else {
        return;
    };
    response
        .headers_mut()
        .insert(axum::http::HeaderName::from_static(RECEIPT_HEADER), value);
}

/// Fetch a receipt by id. `Ok(None)` when no row exists (the route's 404).
pub async fn fetch_receipt(pool: &sqlx::PgPool, id: Uuid) -> Result<Option<Receipt>, ReceiptError> {
    let row = sqlx::query(
        r#"SELECT id, created_at, model, payment_scheme, tx_signature, payer_wallet, amount_paid_atomic, provider_cost_atomic, platform_fee_atomic, total_atomic, vendor_wallet, vendor_settled_atomic, vendor_fee_receivable_atomic
           FROM receipts WHERE id = $1"#,
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        return Ok(None);
    };

    let atomic = |column: &str| -> Result<u64, ReceiptError> {
        let value: i64 = row.try_get(column)?;
        u64::try_from(value)
            .map_err(|_| ReceiptError::Corrupt(format!("negative amount in column {column}")))
    };

    let amount_paid_atomic = atomic("amount_paid_atomic")?;
    let provider_cost_atomic = atomic("provider_cost_atomic")?;
    let platform_fee_atomic = atomic("platform_fee_atomic")?;
    let total_atomic = atomic("total_atomic")?;

    let vendor_wallet: Option<String> = row.try_get("vendor_wallet")?;
    let vendor_settled: Option<i64> = row.try_get("vendor_settled_atomic")?;
    let vendor_fee: Option<i64> = row.try_get("vendor_fee_receivable_atomic")?;
    let vendor = match (vendor_wallet, vendor_settled, vendor_fee) {
        (None, None, None) => None,
        (Some(wallet), Some(settled), Some(fee)) => {
            let settled = u64::try_from(settled)
                .map_err(|_| ReceiptError::Corrupt("negative vendor_settled_atomic".to_string()))?;
            let fee = u64::try_from(fee).map_err(|_| {
                ReceiptError::Corrupt("negative vendor_fee_receivable_atomic".to_string())
            })?;
            Some(ReceiptVendor {
                vendor_wallet: wallet,
                settled_atomic: settled,
                settled_usdc: atomic_to_usdc_string(settled),
                fee_receivable_atomic: fee,
                fee_receivable_usdc: atomic_to_usdc_string(fee),
            })
        }
        // The three vendor fields travel together (a receivable without its
        // wallet is uninvoiceable evidence) — a partial set is corruption,
        // not a servable receipt.
        _ => {
            return Err(ReceiptError::Corrupt(
                "partial vendor settlement fields".to_string(),
            ))
        }
    };

    Ok(Some(Receipt {
        receipt_id: row.try_get("id")?,
        created_at: row.try_get("created_at")?,
        model: row.try_get("model")?,
        payment_scheme: row.try_get("payment_scheme")?,
        tx_signature: row.try_get("tx_signature")?,
        payer_wallet: row.try_get("payer_wallet")?,
        amount_paid_atomic,
        amount_paid_usdc: atomic_to_usdc_string(amount_paid_atomic),
        cost_breakdown: ReceiptCostBreakdown {
            provider_cost_atomic,
            provider_cost_usdc: atomic_to_usdc_string(provider_cost_atomic),
            platform_fee_atomic,
            platform_fee_usdc: atomic_to_usdc_string(platform_fee_atomic),
            total_atomic,
            total_usdc: atomic_to_usdc_string(total_atomic),
            currency: "USDC".to_string(),
        },
        vendor,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_record() -> ReceiptRecord {
        ReceiptRecord {
            receipt_id: Uuid::new_v4(),
            model: "openai/gpt-4o".to_string(),
            payment_scheme: "exact".to_string(),
            tx_signature: Some("tx".to_string()),
            payer_wallet: "wallet".to_string(),
            amount_paid_atomic: 2625,
            provider_cost_atomic: 2500,
            platform_fee_atomic: 125,
            total_atomic: 2625,
            vendor: None,
        }
    }

    #[test]
    fn atomic_to_usdc_string_is_pure_integer_math() {
        assert_eq!(atomic_to_usdc_string(0), "0.000000");
        assert_eq!(atomic_to_usdc_string(1), "0.000001");
        assert_eq!(atomic_to_usdc_string(79), "0.000079");
        assert_eq!(atomic_to_usdc_string(1_050_000), "1.050000");
        assert_eq!(atomic_to_usdc_string(2625), "0.002625");
        // The full u64 range formats without panic or wrap.
        assert_eq!(atomic_to_usdc_string(u64::MAX), "18446744073709.551615");
    }

    #[test]
    fn receipt_path_is_the_public_get_route() {
        let id = Uuid::new_v4();
        assert_eq!(receipt_path(&id), format!("/v1/receipts/{id}"));
    }

    #[tokio::test]
    async fn record_receipt_without_pool_returns_none() {
        // No DB → no receipt and no path (the caller must not emit a header).
        assert!(record_receipt(None, sample_record()).is_none());
    }

    #[test]
    fn receipt_binds_fail_closed_on_bigint_overflow() {
        let mut record = sample_record();
        record.amount_paid_atomic = u64::MAX;
        assert!(
            receipt_binds(&record).is_none(),
            "amounts beyond i64 must refuse, not wrap"
        );

        let mut record = sample_record();
        record.vendor = Some(crate::usage::VendorSettlement {
            vendor_wallet: "v".to_string(),
            settled_atomic: u64::MAX,
            fee_receivable_atomic: 0,
        });
        assert!(
            receipt_binds(&record).is_none(),
            "vendor amounts beyond i64 must refuse, not wrap"
        );
    }

    #[test]
    fn insert_receipt_header_noop_without_path() {
        let mut response = axum::response::Response::new(axum::body::Body::empty());
        insert_receipt_header(&mut response, &None);
        assert!(response.headers().get(RECEIPT_HEADER).is_none());

        let path = Some("/v1/receipts/abc".to_string());
        insert_receipt_header(&mut response, &path);
        assert_eq!(
            response
                .headers()
                .get(RECEIPT_HEADER)
                .and_then(|v| v.to_str().ok()),
            Some("/v1/receipts/abc")
        );
    }
}
