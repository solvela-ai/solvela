//! GET /v1/escrow/config — public escrow configuration discovery endpoint.
//!
//! Returns the escrow program ID, Solana network, USDC mint, provider wallet,
//! and the current Solana slot. No authentication required. Clients use this
//! to discover escrow parameters without making a payment attempt.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Serialize;
use serde_json::json;
use tokio::sync::Mutex;

use crate::AppState;

/// Cached Solana slot value with a 5-second TTL.
///
/// Stored as `Option<(slot, fetched_at)>`. `None` means no cached value yet.
pub type SlotCache = Arc<Mutex<Option<(u64, Instant)>>>;

/// Time-to-live for the cached slot value.
const SLOT_CACHE_TTL: Duration = Duration::from_secs(5);

/// Response body for `GET /v1/escrow/config`.
#[derive(Debug, Clone, Serialize)]
pub struct EscrowConfig {
    pub escrow_program_id: String,
    pub current_slot: Option<u64>,
    pub network: String,
    pub usdc_mint: String,
    pub provider_wallet: String,
}

/// Create a new empty slot cache.
pub fn new_slot_cache() -> SlotCache {
    Arc::new(Mutex::new(None))
}

/// GET /v1/escrow/config
///
/// Returns:
/// - 200 with escrow configuration when `escrow_program_id` is set
/// - 404 when escrow is not configured
pub async fn escrow_config(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let escrow_program_id = match &state.config.solana.escrow_program_id {
        Some(id) if !id.is_empty() => id.clone(),
        _ => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "escrow not configured" })),
            )
                .into_response();
        }
    };

    let current_slot = fetch_cached_slot(&state).await;

    let config = EscrowConfig {
        escrow_program_id,
        current_slot,
        network: x402::types::SOLANA_NETWORK.to_string(),
        usdc_mint: state.config.solana.usdc_mint.clone(),
        provider_wallet: state.config.solana.recipient_wallet.clone(),
    };

    (StatusCode::OK, Json(json!(config))).into_response()
}

/// Fetch the current Solana slot, returning a cached value if still fresh.
///
/// On RPC failure, logs a warning and returns `None` — the endpoint still
/// returns the rest of the config without the slot.
async fn fetch_cached_slot(state: &AppState) -> Option<u64> {
    let cache = &state.slot_cache;
    let mut guard = cache.lock().await;

    // Return cached value if within TTL
    if let Some((slot, fetched_at)) = *guard {
        if fetched_at.elapsed() < SLOT_CACHE_TTL {
            return Some(slot);
        }
    }

    // Fetch fresh slot from RPC
    match fetch_slot_from_rpc(&state.config.solana.rpc_url).await {
        Ok(slot) => {
            *guard = Some((slot, Instant::now()));
            Some(slot)
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to fetch Solana slot for escrow config");
            // Return stale value if available, otherwise None
            guard.map(|(slot, _)| slot)
        }
    }
}

// ---------------------------------------------------------------------------
// GET /v1/escrow/health — operational health of the escrow subsystem
// ---------------------------------------------------------------------------

/// Response body for `GET /v1/escrow/health`.
#[derive(Debug, Clone, Serialize)]
pub struct EscrowHealthResponse {
    pub status: String,
    pub escrow_enabled: bool,
    pub claim_processor_running: bool,
    pub fee_payer_wallets: usize,
    pub claims: EscrowClaimStats,
}

/// Claim processing statistics embedded in the health response.
#[derive(Debug, Clone, Serialize)]
pub struct EscrowClaimStats {
    pub submitted: u64,
    pub succeeded: u64,
    pub failed: u64,
    pub retried: u64,
    pub pending_in_queue: Option<u64>,
}

/// GET /v1/escrow/health
///
/// Returns:
/// - 200 with escrow health when escrow is configured
/// - 404 when escrow is not configured
///
/// Status is "ok" when everything is healthy, "degraded" when the claim
/// processor is not running or fee payer pool is missing, and "down" when
/// escrow is not operational.
pub async fn escrow_health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // Return 404 when escrow is not configured at all
    let _escrow_program_id = match &state.config.solana.escrow_program_id {
        Some(id) if !id.is_empty() => id,
        _ => {
            return (
                StatusCode::NOT_FOUND,
                Json(json!({ "error": "escrow not configured" })),
            )
                .into_response();
        }
    };

    let escrow_enabled = state.escrow_claimer.is_some();
    let claim_processor_running = state.escrow_metrics.is_some() && state.db_pool.is_some();
    let fee_payer_wallets = state.fee_payer_pool.as_ref().map(|p| p.len()).unwrap_or(0);

    // Read claim metrics snapshot
    let (submitted, succeeded, failed, retried) = state
        .escrow_metrics
        .as_ref()
        .map(|m| {
            let snap = m.snapshot();
            (
                snap.claims_submitted,
                snap.claims_succeeded,
                snap.claims_failed,
                snap.claims_retried,
            )
        })
        .unwrap_or((0, 0, 0, 0));

    // Fetch pending claim count from DB if available (fire-and-forget-safe)
    let pending_in_queue = if let Some(ref pool) = state.db_pool {
        match sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM escrow_claim_queue WHERE status = 'pending'",
        )
        .fetch_one(pool)
        .await
        {
            Ok(count) => Some(count as u64),
            Err(e) => {
                tracing::warn!(error = %e, "failed to fetch pending claim count");
                None
            }
        }
    } else {
        None
    };

    // Determine overall status
    let status = if !escrow_enabled {
        "down"
    } else if !claim_processor_running || fee_payer_wallets == 0 {
        "degraded"
    } else {
        "ok"
    };

    let response = EscrowHealthResponse {
        status: status.to_string(),
        escrow_enabled,
        claim_processor_running,
        fee_payer_wallets,
        claims: EscrowClaimStats {
            submitted,
            succeeded,
            failed,
            retried,
            pending_in_queue,
        },
    };

    (StatusCode::OK, Json(json!(response))).into_response()
}

/// Make a `getSlot` JSON-RPC call to the Solana cluster.
async fn fetch_slot_from_rpc(rpc_url: &str) -> Result<u64, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .map_err(|e| format!("HTTP client error: {e}"))?;

    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getSlot",
        "params": []
    });

    let resp = client
        .post(rpc_url)
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("RPC request failed: {e}"))?;

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("RPC response parse failed: {e}"))?;

    json["result"]
        .as_u64()
        .ok_or_else(|| format!("unexpected RPC response: {json}"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_slot_cache_starts_empty() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            let cache = new_slot_cache();
            let guard = cache.lock().await;
            assert!(guard.is_none(), "new slot cache must start empty");
        });
    }

    #[test]
    fn test_escrow_config_serializes_correctly() {
        let config = EscrowConfig {
            escrow_program_id: "GTs7ik3NbW3xwSXq33jyVRGgmshNEyW1h9rxDNATiFLy".to_string(),
            current_slot: Some(298_765_432),
            network: "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp".to_string(),
            usdc_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            provider_wallet: "RecipientWallet111111111111111111111111111111".to_string(),
        };

        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(
            json["escrow_program_id"],
            "GTs7ik3NbW3xwSXq33jyVRGgmshNEyW1h9rxDNATiFLy"
        );
        assert_eq!(json["current_slot"], 298_765_432);
        assert_eq!(json["network"], "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp");
        assert_eq!(
            json["usdc_mint"],
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
        );
        assert_eq!(
            json["provider_wallet"],
            "RecipientWallet111111111111111111111111111111"
        );
    }

    #[test]
    fn test_escrow_config_null_slot_serializes() {
        let config = EscrowConfig {
            escrow_program_id: "ProgramId".to_string(),
            current_slot: None,
            network: "solana:test".to_string(),
            usdc_mint: "Mint".to_string(),
            provider_wallet: "Wallet".to_string(),
        };

        let json = serde_json::to_value(&config).unwrap();
        assert!(json["current_slot"].is_null());
    }

    #[test]
    fn test_slot_cache_ttl_is_five_seconds() {
        assert_eq!(SLOT_CACHE_TTL, Duration::from_secs(5));
    }
}
