//! GET /v1/receipts/{receipt_id} — fetch a client-facing payment receipt.
//!
//! Public route with a capability-by-unguessable-UUID auth model: the UUIDv4
//! receipt id (issued in the `x-solvela-receipt` header on paid responses) IS
//! the bearer credential — agents have no other auth on the public path.
//! Unknown and malformed ids return one identical 404 (no existence or format
//! oracle), and there is deliberately no listing/enumeration endpoint.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;
use tracing::{error, warn};
use uuid::Uuid;

use crate::error::GatewayError;
use crate::receipts::{self, Receipt, ReceiptError};
use crate::AppState;

/// Fixed 404 body shared by unknown and malformed ids — the single signal.
const RECEIPT_NOT_FOUND: &str = "receipt not found";

/// GET /v1/receipts/{receipt_id}
pub async fn get_receipt(
    State(state): State<Arc<AppState>>,
    Path(receipt_id): Path<String>,
) -> Result<Json<Receipt>, GatewayError> {
    // Rule 12 graceful degradation, honestly surfaced: with no DATABASE_URL
    // receipts cannot exist on this gateway AT ALL (the header is never
    // emitted), so the truthful answer is a 503 about the service — not a 404
    // pretending the specific id was looked up and missed.
    let Some(pool) = &state.db_pool else {
        return Err(GatewayError::ServiceUnavailable(
            "receipts are not available: this gateway has no receipt storage configured"
                .to_string(),
        ));
    };

    // A non-UUID id can never match a stored receipt; reject it with the SAME
    // 404 as an unknown id so the response shape leaks nothing about format
    // validity.
    let Ok(id) = Uuid::parse_str(&receipt_id) else {
        return Err(GatewayError::NotFound(RECEIPT_NOT_FOUND.to_string()));
    };

    match receipts::fetch_receipt(pool, id).await {
        Ok(Some(receipt)) => Ok(Json(receipt)),
        Ok(None) => Err(GatewayError::NotFound(RECEIPT_NOT_FOUND.to_string())),
        Err(e) => {
            // Database/corruption detail stays server-side; the client gets
            // the generic 500 envelope. Severity is split by variant: a DB
            // error is infrastructure (error!), a Corrupt row is a
            // data-integrity violation fetch_receipt refused to serve (warn!).
            match &e {
                ReceiptError::Database(_) => {
                    metrics::counter!("solvela_receipt_read_failures_total", "reason" => "db_error")
                        .increment(1);
                    error!(
                        error = %e,
                        receipt_id = %receipt_id,
                        "failed to load receipt — database error"
                    );
                }
                ReceiptError::Corrupt(_) => {
                    metrics::counter!("solvela_receipt_read_failures_total", "reason" => "corrupt")
                        .increment(1);
                    warn!(
                        error = %e,
                        receipt_id = %receipt_id,
                        "failed to load receipt — stored row violates a receipt invariant"
                    );
                }
            }
            Err(GatewayError::Internal("failed to load receipt".to_string()))
        }
    }
}
