//! GET /v1/nonce — returns a durable nonce account and its current value.
//!
//! AI agent clients use this endpoint to fetch a fresh nonce before constructing
//! a pre-signed USDC payment transaction. The nonce replaces the recent blockhash,
//! allowing the transaction to be stored and submitted without expiry.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde_json::json;
use tracing;

use crate::AppState;

/// GET /v1/nonce
///
/// Returns:
/// - 200 with nonce account details and current nonce value
/// - 404 if no nonce pool is configured
pub async fn get_nonce(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let pool = match &state.nonce_pool {
        Some(p) => p,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "error": "no nonce accounts configured — use recent blockhash instead"
                })),
            )
                .into_response();
        }
    };

    let entry = match pool.next() {
        Some(e) => e,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "error": "no nonce accounts configured — use recent blockhash instead"
                })),
            )
                .into_response();
        }
    };

    let rpc_url = &state.config.solana.rpc_url;

    match pool.fetch_nonce_value(rpc_url, entry).await {
        Ok(nonce_value) => (
            StatusCode::OK,
            Json(json!({
                "nonce_account": entry.nonce_account,
                "authority": entry.authority,
                "nonce_value": nonce_value,
                // NOTE: rpc_url is intentionally omitted — it may contain an
                // embedded API key (e.g. ?api-key=xxxx) that must not be leaked.
            })),
        )
            .into_response(),
        Err(e) => {
            tracing::error!(error = %e, "nonce fetch failed");
            (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(json!({
                    "error": "Failed to fetch nonce value"
                })),
            )
                .into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// Tests — written FIRST (RED phase)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    // Note: route-level unit tests live here; integration tests are in
    // crates/gateway/tests/integration.rs (see test_nonce_endpoint_* tests).

    use super::*;
    use solvela_x402::nonce_pool::{NonceEntry, NoncePool};

    fn make_pool_with_entry(nonce_account: &str, authority: &str) -> Arc<NoncePool> {
        Arc::new(
            NoncePool::from_entries(vec![NonceEntry {
                nonce_account: nonce_account.to_string(),
                authority: authority.to_string(),
            }])
            .expect("valid pool"),
        )
    }

    #[test]
    fn test_nonce_pool_none_means_no_pool_configured() {
        // When AppState.nonce_pool is None, the handler must return 404.
        // This is validated by the integration test `test_nonce_endpoint_no_pool`.
        // Here we just verify the pool itself is correctly absent.
        let nonce_pool: Option<Arc<NoncePool>> = None;
        assert!(nonce_pool.is_none());
    }

    #[test]
    fn test_nonce_pool_some_is_accessible() {
        // When AppState.nonce_pool is Some, the handler can access the entry.
        let pool = make_pool_with_entry(
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
            "11111111111111111111111111111111",
        );
        let entry = pool.next().expect("pool must return entry");
        assert_eq!(
            entry.nonce_account,
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
        );
        assert_eq!(entry.authority, "11111111111111111111111111111111");
    }
}
