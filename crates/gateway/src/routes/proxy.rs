//! External service proxy handler.
//!
//! `POST /v1/services/{service_id}/proxy` — accepts an arbitrary JSON body,
//! verifies x402 payment, forwards the request to the external service's
//! endpoint, and returns the response.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use tracing::{info, warn};

use x402::types::{
    CostBreakdown, PaymentAccept, PaymentRequired, Resource, PLATFORM_FEE_PERCENT, SOLANA_NETWORK,
    USDC_MINT, X402_VERSION,
};

use crate::error::GatewayError;
use crate::middleware::x402::decode_payment_header;
use crate::usage::SpendLogEntry;
use crate::AppState;

/// Upstream request timeout for external service proxying.
const PROXY_TIMEOUT: Duration = Duration::from_secs(60);

/// Platform fee multiplier (5% on top of service cost).
const PLATFORM_FEE_MULTIPLIER: f64 = 1.05;

/// POST /v1/services/{service_id}/proxy — proxy a paid request to an external service.
///
/// Flow:
/// 1. Look up service in registry by `service_id` path parameter
/// 2. Reject internal services (400) and unhealthy services (503)
/// 3. If no PAYMENT-SIGNATURE header → return 402 with cost breakdown
/// 4. If payment present → decode, validate, replay-protect, verify via Facilitator
/// 5. Forward request body to service endpoint with 60s timeout
/// 6. Return upstream response (2xx passthrough, 5xx → 502, timeout → 504)
/// 7. Fire-and-forget spend log
pub async fn proxy_service(
    State(state): State<Arc<AppState>>,
    Path(service_id): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Result<Response, GatewayError> {
    // Step 1: Look up service in registry
    let registry = state.service_registry.read().await;
    let service = registry
        .get(&service_id)
        .ok_or_else(|| GatewayError::ModelNotFound(format!("service '{service_id}' not found")))?
        .clone();
    // Release the read lock early
    drop(registry);

    // Step 2a: Reject internal services
    if service.internal {
        return Err(GatewayError::BadRequest(
            "use /v1/chat/completions for internal services".to_string(),
        ));
    }

    // Step 2b: Reject services without x402 support
    if !service.x402_enabled {
        return Err(GatewayError::BadRequest(
            "service does not support x402 payments".to_string(),
        ));
    }

    // Step 2c: Reject unhealthy services
    if service.healthy == Some(false) {
        return Ok((
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(json!({
                "error": "service is currently unavailable"
            })),
        )
            .into_response());
    }

    // Step 2d: Verify service has pricing configured
    let price_usdc = service.price_per_request_usdc.ok_or_else(|| {
        GatewayError::Internal(format!(
            "service '{service_id}' has no price_per_request_usdc configured"
        ))
    })?;

    // Step 3: Check for payment
    let payment_header = headers
        .get("payment-signature")
        .and_then(|v| v.to_str().ok());

    if payment_header.is_none() {
        info!(service_id = %service_id, "no payment signature, returning 402");

        let platform_fee = price_usdc * (PLATFORM_FEE_PERCENT as f64 / 100.0);
        let total = price_usdc + platform_fee;
        let atomic_amount = (total * 1_000_000.0) as u64;

        let payment_required = PaymentRequired {
            x402_version: X402_VERSION,
            resource: Resource {
                url: format!("/v1/services/{service_id}/proxy"),
                method: "POST".to_string(),
            },
            accepts: vec![PaymentAccept {
                scheme: "exact".to_string(),
                network: SOLANA_NETWORK.to_string(),
                amount: atomic_amount.to_string(),
                asset: USDC_MINT.to_string(),
                pay_to: state.config.solana.recipient_wallet.clone(),
                max_timeout_seconds: x402::types::MAX_TIMEOUT_SECONDS,
                escrow_program_id: None,
            }],
            cost_breakdown: CostBreakdown {
                provider_cost: format!("{price_usdc:.6}"),
                platform_fee: format!("{platform_fee:.6}"),
                total: format!("{total:.6}"),
                currency: "USDC".to_string(),
                fee_percent: PLATFORM_FEE_PERCENT,
            },
            error: "Payment required".to_string(),
        };

        return Err(GatewayError::InvalidPayment(
            serde_json::to_string(&payment_required)
                .unwrap_or_else(|_| "payment required".to_string()),
        ));
    }

    // Step 4: Payment present — decode and verify
    let raw_header = payment_header.expect("checked above");
    let payload = decode_payment_header(raw_header).map_err(|e| {
        GatewayError::InvalidPayment(format!(
            "PAYMENT-SIGNATURE header could not be decoded: {e}"
        ))
    })?;

    // Validate resource URL matches this proxy endpoint
    let expected_url = format!("/v1/services/{service_id}/proxy");
    if payload.resource.url != expected_url {
        return Err(GatewayError::InvalidPayment(format!(
            "payment resource '{}' does not match this endpoint",
            payload.resource.url
        )));
    }

    // Validate resource method is POST
    if !payload.resource.method.eq_ignore_ascii_case("POST") {
        return Err(GatewayError::BadRequest(format!(
            "payment resource method must be POST, got '{}'",
            payload.resource.method
        )));
    }

    // Validate network is Solana
    if !payload
        .accepted
        .network
        .eq_ignore_ascii_case(SOLANA_NETWORK)
    {
        return Err(GatewayError::BadRequest(format!(
            "payment network must be '{SOLANA_NETWORK}', got '{}'",
            payload.accepted.network
        )));
    }

    // Validate asset is USDC-SPL mint
    if payload.accepted.asset != USDC_MINT {
        return Err(GatewayError::BadRequest(format!(
            "payment asset must be USDC mint '{USDC_MINT}', got '{}'",
            payload.accepted.asset
        )));
    }

    // Validate pay_to matches the gateway's recipient wallet
    if payload.accepted.pay_to != state.config.solana.recipient_wallet {
        return Err(GatewayError::BadRequest(format!(
            "payment pay_to must be '{}', got '{}'",
            state.config.solana.recipient_wallet, payload.accepted.pay_to
        )));
    }

    // Validate payment amount covers the service cost + platform fee
    let total_cost = price_usdc * PLATFORM_FEE_MULTIPLIER;
    let expected_atomic = (total_cost * 1_000_000.0) as u64;
    let client_amount: u64 = payload.accepted.amount.parse().unwrap_or(0);
    if client_amount < expected_atomic {
        return Err(GatewayError::BadRequest(format!(
            "payment amount insufficient: paid {client_amount} but cost is {expected_atomic} atomic USDC"
        )));
    }

    // Validate scheme matches payload variant
    match (payload.accepted.scheme.as_str(), &payload.payload) {
        ("exact", x402::types::PayloadData::Escrow(_)) => {
            return Err(GatewayError::BadRequest(
                "scheme is 'exact' but payload contains escrow data".to_string(),
            ));
        }
        ("escrow", x402::types::PayloadData::Direct(_)) => {
            return Err(GatewayError::BadRequest(
                "scheme is 'escrow' but payload contains direct transfer data".to_string(),
            ));
        }
        _ => {}
    }

    // Replay protection
    let tx_raw = match &payload.payload {
        x402::types::PayloadData::Direct(p) => &p.transaction,
        x402::types::PayloadData::Escrow(p) => &p.deposit_tx,
    };

    let replay_detected = if let Some(cache) = &state.cache {
        cache.check_and_record_tx(tx_raw).await.is_err()
    } else {
        let mut replay_set = state.replay_set.lock().expect("replay_set mutex poisoned");
        if replay_set.get(tx_raw).is_some() {
            true
        } else {
            replay_set.put(tx_raw.to_string(), ());
            warn!(
                tx = %tx_raw,
                "payment accepted under degraded in-memory replay protection (no Redis)"
            );
            false
        }
    };

    if replay_detected {
        warn!(tx = %tx_raw, "replay attack detected — transaction already used");
        return Err(GatewayError::InvalidPayment(
            "transaction has already been used; each payment signature may only be submitted once"
                .to_string(),
        ));
    }

    // Verify and settle via Facilitator
    match state.facilitator.verify_and_settle(&payload).await {
        Ok(settlement) => {
            info!(
                tx_signature = ?settlement.tx_signature,
                network = %settlement.network,
                service_id = %service_id,
                "proxy payment verified and settled"
            );
        }
        Err(e) => {
            warn!(error = %e, service_id = %service_id, "proxy payment verification failed");
            return Err(GatewayError::InvalidPayment(format!(
                "payment verification failed: {e}"
            )));
        }
    }

    // Extract wallet address for spend logging
    let wallet_address = payload.accepted.pay_to.clone();
    let tx_signature = match &payload.payload {
        x402::types::PayloadData::Direct(p) => Some(p.transaction.clone()),
        x402::types::PayloadData::Escrow(p) => Some(p.deposit_tx.clone()),
    };

    // Extract request ID for traceability
    let request_id: Option<String> = headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    // Step 5: Forward request to upstream service
    info!(
        service_id = %service_id,
        endpoint = %service.endpoint,
        "forwarding request to external service"
    );

    // Collect the body bytes to forward
    let body_bytes = axum::body::to_bytes(body, 10 * 1024 * 1024)
        .await
        .map_err(|e| GatewayError::BadRequest(format!("failed to read request body: {e}")))?;

    // Build upstream request
    let mut upstream_req = state
        .http_client
        .post(&service.endpoint)
        .timeout(PROXY_TIMEOUT)
        .body(body_bytes.clone());

    // Forward Content-Type header
    if let Some(ct) = headers.get("content-type") {
        if let Ok(ct_str) = ct.to_str() {
            upstream_req = upstream_req.header("content-type", ct_str);
        }
    }

    // Forward Accept header
    if let Some(accept) = headers.get("accept") {
        if let Ok(accept_str) = accept.to_str() {
            upstream_req = upstream_req.header("accept", accept_str);
        }
    }

    // Attach request ID for traceability
    if let Some(ref rid) = request_id {
        upstream_req = upstream_req.header("x-rcr-request-id", rid.as_str());
    }

    // Step 6: Send request and handle response
    let upstream_response = match upstream_req.send().await {
        Ok(resp) => resp,
        Err(e) if e.is_timeout() => {
            warn!(
                service_id = %service_id,
                error = %e,
                "upstream service timed out"
            );
            return Ok((
                StatusCode::GATEWAY_TIMEOUT,
                axum::Json(json!({
                    "error": "upstream service timed out"
                })),
            )
                .into_response());
        }
        Err(e) => {
            warn!(
                service_id = %service_id,
                error = %e,
                "failed to reach upstream service"
            );
            return Ok((
                StatusCode::BAD_GATEWAY,
                axum::Json(json!({
                    "error": "service unreachable"
                })),
            )
                .into_response());
        }
    };

    let upstream_status = upstream_response.status();

    // Step 7: Fire-and-forget spend log (log_spend internally uses tokio::spawn)
    state.usage.log_spend(SpendLogEntry {
        wallet_address,
        model: service_id.clone(),
        provider: "external-service".to_string(),
        input_tokens: 0,
        output_tokens: 0,
        cost_usdc: total_cost,
        tx_signature,
        request_id: request_id.clone(),
        session_id: None,
    });

    // Handle upstream response based on status
    if upstream_status.is_server_error() {
        warn!(
            service_id = %service_id,
            upstream_status = %upstream_status,
            "upstream service returned server error"
        );
        return Ok((
            StatusCode::BAD_GATEWAY,
            axum::Json(json!({
                "error": "upstream service error"
            })),
        )
            .into_response());
    }

    // For 2xx and 4xx, forward the response as-is
    let response_headers = upstream_response.headers().clone();
    let response_bytes = upstream_response
        .bytes()
        .await
        .map_err(|e| GatewayError::Internal(format!("failed to read upstream response: {e}")))?;

    let mut builder = Response::builder().status(upstream_status);

    // Forward Content-Type from upstream
    if let Some(ct) = response_headers.get("content-type") {
        builder = builder.header("content-type", ct);
    }

    builder
        .body(Body::from(response_bytes))
        .map_err(|e| GatewayError::Internal(format!("failed to build response: {e}")))
}
