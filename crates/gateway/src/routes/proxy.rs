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
use metrics::{counter, histogram};
use serde_json::json;
use tracing::{info, warn};

use solvela_x402::types::{
    CostBreakdown, PaymentAccept, PaymentRequired, Resource, PLATFORM_FEE_PERCENT, SOLANA_NETWORK,
    USDC_MINT, X402_VERSION,
};

use crate::error::GatewayError;
use crate::middleware::x402::decode_payment_header;
use crate::payment_util::extract_payer_wallet;
use crate::security;
use crate::usage::SpendLogEntry;
use crate::AppState;

/// Upstream request timeout for external service proxying.
const PROXY_TIMEOUT: Duration = Duration::from_secs(60);

/// Compute the total cost in atomic USDC units for a service request.
///
/// Uses integer arithmetic to avoid floating-point precision loss on financial
/// amounts. The price is converted to atomic units (6 decimals) first, then
/// the 5% platform fee is applied using integer math.
fn compute_service_cost_atomic(price_usdc: f64) -> u64 {
    // Convert to atomic units first (the only f64->int conversion)
    let provider_atomic = (price_usdc * 1_000_000.0).round() as u64;
    // 5% platform fee: total = provider * 105 / 100
    provider_atomic * 105 / 100
}

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

    // Compute cost once using integer arithmetic — used for both 402 response
    // and payment validation to guarantee identical results.
    let expected_atomic = compute_service_cost_atomic(price_usdc);
    let provider_atomic = (price_usdc * 1_000_000.0).round() as u64;
    let fee_atomic = expected_atomic - provider_atomic;

    // Step 3: Check for payment header.
    // Non-ASCII bytes in header value must produce 400, not a silent 402.
    let payment_header = match headers.get("payment-signature") {
        Some(val) => match val.to_str() {
            Ok(s) => Some(s),
            Err(_) => {
                return Err(GatewayError::BadRequest(
                    "Invalid PAYMENT-SIGNATURE header encoding".to_string(),
                ));
            }
        },
        None => None,
    };

    if payment_header.is_none() {
        counter!("solvela_payments_total", "status" => "none").increment(1);
        info!(service_id = %service_id, "no payment signature, returning 402");

        // Format cost breakdown from integer-derived values for display
        let provider_usdc = provider_atomic as f64 / 1_000_000.0;
        let fee_usdc = fee_atomic as f64 / 1_000_000.0;
        let total_usdc = expected_atomic as f64 / 1_000_000.0;

        let payment_required = PaymentRequired {
            x402_version: X402_VERSION,
            resource: Resource {
                url: format!("/v1/services/{service_id}/proxy"),
                method: "POST".to_string(),
            },
            accepts: vec![PaymentAccept {
                scheme: "exact".to_string(),
                network: SOLANA_NETWORK.to_string(),
                amount: expected_atomic.to_string(),
                asset: USDC_MINT.to_string(),
                pay_to: state.config.solana.recipient_wallet.clone(),
                max_timeout_seconds: solvela_x402::types::MAX_TIMEOUT_SECONDS,
                escrow_program_id: None,
            }],
            cost_breakdown: CostBreakdown {
                provider_cost: format!("{provider_usdc:.6}"),
                platform_fee: format!("{fee_usdc:.6}"),
                total: format!("{total_usdc:.6}"),
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

    // Validate payment amount covers the service cost + platform fee.
    // Invalid amount format must produce 400, not silently become zero.
    let client_amount: u64 = payload
        .accepted
        .amount
        .parse()
        .map_err(|_| GatewayError::BadRequest("Invalid payment amount format".to_string()))?;
    if client_amount < expected_atomic {
        return Err(GatewayError::BadRequest(format!(
            "payment amount insufficient: paid {client_amount} but cost is {expected_atomic} atomic USDC"
        )));
    }

    // Validate scheme matches payload variant
    match (payload.accepted.scheme.as_str(), &payload.payload) {
        ("exact", solvela_x402::types::PayloadData::Escrow(_)) => {
            return Err(GatewayError::BadRequest(
                "scheme is 'exact' but payload contains escrow data".to_string(),
            ));
        }
        ("escrow", solvela_x402::types::PayloadData::Direct(_)) => {
            return Err(GatewayError::BadRequest(
                "scheme is 'escrow' but payload contains direct transfer data".to_string(),
            ));
        }
        _ => {}
    }

    // Replay protection
    let tx_raw = match &payload.payload {
        solvela_x402::types::PayloadData::Direct(p) => &p.transaction,
        solvela_x402::types::PayloadData::Escrow(p) => &p.deposit_tx,
    };

    let is_durable_nonce = crate::routes::chat::uses_durable_nonce(tx_raw);

    let replay_detected = if let Some(cache) = &state.cache {
        cache
            .check_and_record_tx(tx_raw, is_durable_nonce)
            .await
            .is_err()
    } else {
        // GHSA-fq3f-c8p7-873f: durable-nonce transactions carry a 24-hour replay window.
        // The in-memory LRU cannot cover that window, so deny rather than accept with
        // degraded protection.
        if is_durable_nonce {
            warn!(
                tx = %tx_raw,
                "durable-nonce proxy payment rejected: Redis unavailable (GHSA-fq3f-c8p7-873f)"
            );
            return Err(GatewayError::InvalidPayment(
                "Payment service is temporarily degraded; please retry shortly.".to_string(),
            ));
        }
        let mut replay_set = state
            .replay_set
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if replay_set.get(tx_raw).is_some() {
            true
        } else {
            replay_set.put(tx_raw.to_string(), std::time::Instant::now());
            warn!(
                tx = %tx_raw,
                "payment accepted under degraded in-memory replay protection (no Redis)"
            );
            false
        }
    };

    if replay_detected {
        counter!("solvela_replay_rejections_total").increment(1);
        counter!("solvela_payments_total", "status" => "failed").increment(1);
        warn!(tx = %tx_raw, "replay attack detected — transaction already used");
        return Err(GatewayError::InvalidPayment(
            "transaction has already been used; each payment signature may only be submitted once"
                .to_string(),
        ));
    }

    // Verify and settle via Facilitator
    match state.facilitator.verify_and_settle(&payload).await {
        Ok(settlement) => {
            counter!("solvela_payments_total", "status" => "verified").increment(1);
            histogram!("solvela_payment_amount_usdc").record(client_amount as f64 / 1_000_000.0);
            info!(
                tx_signature = ?settlement.tx_signature,
                network = %settlement.network,
                service_id = %service_id,
                "proxy payment verified and settled"
            );
        }
        Err(e) => {
            // GHSA-cgqx-mg48-949v: do not echo the verifier error to clients.
            counter!("solvela_payments_total", "status" => "failed").increment(1);
            warn!(error = %e, service_id = %service_id, "proxy payment verification failed");
            return Err(GatewayError::InvalidPayment(
                "Payment verification failed. Check your transaction and retry.".to_string(),
            ));
        }
    }

    // Extract the actual PAYER wallet (not the recipient pay_to address)
    let wallet_address = extract_payer_wallet(&payload);
    let tx_signature = match &payload.payload {
        solvela_x402::types::PayloadData::Direct(p) => Some(p.transaction.clone()),
        solvela_x402::types::PayloadData::Escrow(p) => Some(p.deposit_tx.clone()),
    };

    // Extract request ID for traceability
    let request_id: Option<String> = headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    // Step 5: SSRF check — resolve DNS once, validate all addresses are public,
    // then pin the validated IP into a per-request reqwest client. This eliminates
    // the TOCTOU / DNS rebinding window where an attacker controlling DNS could
    // return a public IP for the check and a private IP for the actual connection.
    let (validated_host, validated_addr) =
        match security::resolve_and_validate_endpoint(&service.endpoint).await {
            Ok(result) => result,
            Err(e) => {
                warn!(
                    service_id = %service_id,
                    endpoint = %service.endpoint,
                    error = %e,
                    "SSRF blocked: service endpoint failed validation"
                );
                return Err(GatewayError::BadRequest(format!(
                    "service endpoint resolves to a private or internal network address: {e}"
                )));
            }
        };

    // Step 6: Forward request to upstream service
    info!(
        service_id = %service_id,
        endpoint = %service.endpoint,
        resolved_ip = %validated_addr,
        "forwarding request to external service (DNS pinned)"
    );

    // Build a per-request client that pins the validated DNS resolution,
    // preventing DNS rebinding between the SSRF check and the connection.
    let pinned_client = reqwest::Client::builder()
        .resolve(&validated_host, validated_addr)
        .build()
        .map_err(|e| GatewayError::Internal(format!("HTTP client error: {e}")))?;

    // Collect the body bytes to forward
    let body_bytes = axum::body::to_bytes(body, 10 * 1024 * 1024)
        .await
        .map_err(|e| GatewayError::BadRequest(format!("failed to read request body: {e}")))?;

    // Build upstream request using the DNS-pinned client
    let mut upstream_req = pinned_client
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
        upstream_req = upstream_req.header("x-solvela-request-id", rid.as_str());
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
    let total_cost_usdc = expected_atomic as f64 / 1_000_000.0;
    state.usage.log_spend(SpendLogEntry {
        wallet_address,
        model: service_id.clone(),
        provider: "external-service".to_string(),
        input_tokens: 0,
        output_tokens: 0,
        cost_usdc: total_cost_usdc,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payment_util::extract_signer_from_base64_tx;

    #[test]
    fn test_compute_service_cost_atomic_basic() {
        // 0.01 USDC = 10_000 atomic; with 5% fee = 10_500
        assert_eq!(compute_service_cost_atomic(0.01), 10_500);
    }

    #[test]
    fn test_compute_service_cost_atomic_small() {
        // 0.001 USDC = 1_000 atomic; with 5% fee = 1_050
        assert_eq!(compute_service_cost_atomic(0.001), 1_050);
    }

    #[test]
    fn test_compute_service_cost_atomic_zero() {
        assert_eq!(compute_service_cost_atomic(0.0), 0);
    }

    #[test]
    fn test_compute_service_cost_atomic_large() {
        // 1.0 USDC = 1_000_000 atomic; with 5% fee = 1_050_000
        assert_eq!(compute_service_cost_atomic(1.0), 1_050_000);
    }

    #[test]
    fn test_compute_service_cost_atomic_consistency() {
        let price = 0.002625;
        let result1 = compute_service_cost_atomic(price);
        let result2 = compute_service_cost_atomic(price);
        assert_eq!(result1, result2);
    }

    #[test]
    fn test_compute_service_cost_uses_round_not_truncate() {
        // 0.0000015 USDC = 1.5 atomic -> rounds to 2 -> 2 * 105/100 = 2
        let cost = compute_service_cost_atomic(0.0000015);
        assert_eq!(cost, 2);
    }

    #[test]
    fn test_extract_payer_wallet_escrow() {
        let payload = solvela_x402::types::PaymentPayload {
            x402_version: 1,
            resource: solvela_x402::types::Resource {
                url: "/test".to_string(),
                method: "POST".to_string(),
            },
            accepted: solvela_x402::types::PaymentAccept {
                scheme: "escrow".to_string(),
                network: SOLANA_NETWORK.to_string(),
                amount: "1000".to_string(),
                asset: USDC_MINT.to_string(),
                pay_to: "RecipientWallet".to_string(),
                max_timeout_seconds: 300,
                escrow_program_id: None,
            },
            payload: solvela_x402::types::PayloadData::Escrow(solvela_x402::types::EscrowPayload {
                deposit_tx: "dGVzdA==".to_string(),
                service_id: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_string(),
                agent_pubkey: "9noXzpXnkyEcKF3AeXqUHTdR59V5uvrRBUo9bwsHaByz".to_string(),
            }),
        };
        assert_eq!(
            extract_payer_wallet(&payload),
            "9noXzpXnkyEcKF3AeXqUHTdR59V5uvrRBUo9bwsHaByz"
        );
    }

    #[test]
    fn test_extract_payer_wallet_direct_invalid_tx() {
        let payload = solvela_x402::types::PaymentPayload {
            x402_version: 1,
            resource: solvela_x402::types::Resource {
                url: "/test".to_string(),
                method: "POST".to_string(),
            },
            accepted: solvela_x402::types::PaymentAccept {
                scheme: "exact".to_string(),
                network: SOLANA_NETWORK.to_string(),
                amount: "1000".to_string(),
                asset: USDC_MINT.to_string(),
                pay_to: "RecipientWallet".to_string(),
                max_timeout_seconds: 300,
                escrow_program_id: None,
            },
            payload: solvela_x402::types::PayloadData::Direct(solvela_x402::types::SolanaPayload {
                transaction: "not-valid-base64!!!".to_string(),
            }),
        };
        assert_eq!(extract_payer_wallet(&payload), "unknown");
    }

    #[test]
    fn test_extract_signer_from_base64_tx_invalid() {
        assert_eq!(extract_signer_from_base64_tx("not-base64!!!"), None);
        assert_eq!(extract_signer_from_base64_tx(""), None);
        assert_eq!(extract_signer_from_base64_tx("AAAA"), None);
    }

    // =========================================================================
    // GHSA-fq3f-c8p7-873f: durable-nonce replay guard
    // =========================================================================

    /// Build a minimal base64-encoded Solana transaction whose first instruction
    /// is AdvanceNonceAccount (discriminator = [4, 0, 0, 0]) so that
    /// `uses_durable_nonce` returns `true`.
    fn make_durable_nonce_tx_b64() -> String {
        use base64::Engine;
        // Legacy message layout:
        //   header [req_sigs=1, ro_signed=0, ro_unsigned=1]
        //   compact-u16 num_accounts = 3
        //   key[0] = system program ([0;32])
        //   key[1] = nonce account ([1;32])
        //   key[2] = system program ([0;32])   ← used as program_id
        //   recent_blockhash [0;32]
        //   compact-u16 num_instructions = 1
        //   ix: program_id_index=2, accounts=[1,0], data=[4,0,0,0]
        let mut msg: Vec<u8> = vec![1u8, 0u8, 1u8, 3u8];
        msg.extend_from_slice(&[0u8; 32]); // system program
        msg.extend_from_slice(&[1u8; 32]); // nonce account
        msg.extend_from_slice(&[0u8; 32]); // system program again (as program_id)
        msg.extend_from_slice(&[0u8; 32]); // recent blockhash
        msg.push(1u8); // 1 instruction
        msg.push(2u8); // program_id_index = 2
        msg.push(2u8); // 2 accounts
        msg.extend_from_slice(&[1u8, 0u8]); // account indices
        msg.push(4u8); // data len = 4
        msg.extend_from_slice(&[4u8, 0u8, 0u8, 0u8]); // AdvanceNonceAccount

        let mut tx_data = vec![0x01u8]; // 1 signature
        tx_data.extend_from_slice(&[0xAAu8; 64]); // placeholder signature
        tx_data.extend_from_slice(&msg);
        base64::engine::general_purpose::STANDARD.encode(&tx_data)
    }

    #[tokio::test]
    async fn test_durable_nonce_payment_denied_when_redis_down() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use base64::Engine;
        use std::sync::Arc;
        use tokio::sync::RwLock;
        use tower::ServiceExt;

        use crate::config::AppConfig;
        use crate::providers::health::{CircuitBreakerConfig, ProviderHealthTracker};
        use crate::providers::ProviderRegistry;
        use crate::routes::escrow::new_slot_cache;
        use crate::services::{ServiceEntry, ServiceRegistry};
        use crate::usage::UsageTracker;
        use solvela_router::models::ModelRegistry;
        use solvela_x402::facilitator::Facilitator;

        let durable_nonce_b64 = make_durable_nonce_tx_b64();
        assert!(
            crate::routes::chat::uses_durable_nonce(&durable_nonce_b64),
            "test fixture must be detected as a durable-nonce transaction"
        );

        // Build a PaymentPayload that passes all pre-replay validation.
        // price=0.01 USDC → expected_atomic = 10_000 * 105/100 = 10_500
        let pp = solvela_x402::types::PaymentPayload {
            x402_version: 2,
            resource: solvela_x402::types::Resource {
                url: "/v1/services/test-svc/proxy".to_string(),
                method: "POST".to_string(),
            },
            accepted: solvela_x402::types::PaymentAccept {
                scheme: "exact".to_string(),
                network: SOLANA_NETWORK.to_string(),
                amount: "10500".to_string(),
                asset: USDC_MINT.to_string(),
                // AppConfig::default().solana.recipient_wallet is an empty string
                pay_to: String::new(),
                max_timeout_seconds: 300,
                escrow_program_id: None,
            },
            payload: solvela_x402::types::PayloadData::Direct(solvela_x402::types::SolanaPayload {
                transaction: durable_nonce_b64,
            }),
        };
        let header_value = base64::engine::general_purpose::STANDARD
            .encode(serde_json::to_vec(&pp).expect("serialize")); // safe: known-good struct

        // Register a test service so the registry lookup succeeds.
        let mut reg = ServiceRegistry::empty();
        reg.register(ServiceEntry {
            id: "test-svc".to_string(),
            name: "Test Service".to_string(),
            category: "test".to_string(),
            endpoint: "https://test.example.com/api".to_string(),
            x402_enabled: true,
            internal: false,
            description: None,
            pricing_label: "per-request".to_string(),
            chains: vec!["solana".to_string()],
            source: "api".to_string(),
            healthy: None,
            price_per_request_usdc: Some(0.01),
        })
        .expect("valid test service entry"); // safe: known-good test data

        let state = Arc::new(crate::AppState {
            config: AppConfig::default(),
            model_registry: ModelRegistry::from_toml(
                "[models.placeholder]\nprovider=\"t\"\nmodel_id=\"t\"\ndisplay_name=\"T\"\ninput_cost_per_million=1.0\noutput_cost_per_million=1.0\ncontext_window=4096\nsupports_streaming=false\nsupports_tools=false\nsupports_vision=false",
            )
            .expect("valid placeholder model TOML"), // safe: known-good test data
            service_registry: RwLock::new(reg),
            providers: ProviderRegistry::from_env(reqwest::Client::new()),
            facilitator: Facilitator::new(vec![]),
            usage: UsageTracker::noop(),
            cache: None, // no Redis — triggers the LRU fallback path
            provider_health: ProviderHealthTracker::new(CircuitBreakerConfig::default()),
            escrow_claimer: None,
            fee_payer_pool: None,
            nonce_pool: None,
            db_pool: None,
            session_secret: b"test-secret".to_vec(),
            http_client: reqwest::Client::new(),
            replay_set: crate::AppState::new_replay_set(),
            slot_cache: new_slot_cache(),
            escrow_metrics: None,
            admin_token: None,
            prometheus_handle: None,
            dev_bypass_payment: false,
        });

        let app = axum::Router::new()
            .route(
                "/v1/services/{service_id}/proxy",
                axum::routing::post(proxy_service),
            )
            .with_state(state);

        let request = Request::builder()
            .method("POST")
            .uri("/v1/services/test-svc/proxy")
            .header("payment-signature", &header_value)
            .body(Body::empty())
            .expect("valid test request"); // safe: known-good test data

        let response = app.oneshot(request).await.expect("handler must not panic");

        assert_eq!(
            response.status(),
            StatusCode::PAYMENT_REQUIRED,
            "durable-nonce payment must be denied (402) when Redis is unavailable"
        );

        let body_bytes = axum::body::to_bytes(response.into_body(), 65_536)
            .await
            .expect("read response body"); // safe: small test response
        let body: serde_json::Value =
            serde_json::from_slice(&body_bytes).expect("valid JSON error body"); // safe: gateway always returns JSON
        let message = body["error"]["message"].as_str().unwrap_or("");
        assert!(
            message.contains("temporarily degraded"),
            "error message must indicate service degradation, got: {message}"
        );
    }
}
