//! External service proxy handler.
//!
//! `POST /v1/services/{service_id}/proxy` — accepts an arbitrary JSON body,
//! verifies x402 payment, forwards the request to the external service's
//! endpoint, and returns the response.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use metrics::{counter, histogram};
use serde_json::json;
use tracing::{info, warn};
use uuid::Uuid;

use solvela_x402::types::{
    CostBreakdown, PaymentAccept, PaymentRequired, Resource, PLATFORM_FEE_PERCENT, SOLANA_NETWORK,
    X402_VERSION,
};

use crate::error::GatewayError;
use crate::middleware::x402::decode_payment_header;
use crate::payment_util::extract_payer_wallet;
use crate::receipts;
use crate::security;
use crate::usage::{SpendLogEntry, VendorSettlement};
use crate::AppState;

/// Upstream request timeout for external service proxying.
const PROXY_TIMEOUT: Duration = Duration::from_secs(60);

/// Upper bound for `price_per_request_usdc` that the integer cost path will accept.
///
/// Multiplying by 1_000_000 (USDC → atomic units) must not overflow `u64`.
/// We cap at `u64::MAX / 1_000_000` ≈ $18.4 trillion per request, which is far
/// beyond any realistic external-service price.
const PRICE_USDC_MAX: f64 = (u64::MAX / 1_000_000) as f64;

/// Validate a `price_per_request_usdc` value before it is cast to `u64`.
///
/// A naked `as u64` cast is fail-open for adversarial values:
/// - `NaN as u64` → 0 (service served for free)
/// - `f64::INFINITY as u64` → `u64::MAX` (panics on later arithmetic)
/// - negative `as u64` → giant positive number (also serves for free after `% 105 / 100`
///   because the price would be parsed but the comparison against `client_amount`
///   could overflow or under-charge).
fn validate_price_usdc(price_usdc: f64) -> Result<(), String> {
    if !price_usdc.is_finite() {
        return Err(format!(
            "price_per_request_usdc is non-finite ({price_usdc}); \
             refusing to cast NaN/∞ to u64"
        ));
    }
    if price_usdc < 0.0 {
        return Err(format!(
            "price_per_request_usdc is negative ({price_usdc}); \
             negative USDC amounts are invalid"
        ));
    }
    if price_usdc > PRICE_USDC_MAX {
        return Err(format!(
            "price_per_request_usdc ({price_usdc}) exceeds u64 range \
             after ×1_000_000 conversion"
        ));
    }
    Ok(())
}

/// All three atomic-USDC components of a paid service request, derived from a
/// single `price_per_request_usdc` value.
///
/// Centralising the breakdown in one struct (instead of recomputing each field
/// inline at the call site) prevents drift if the platform-fee percentage ever
/// changes — the call site only knows about `total_atomic`, `provider_atomic`,
/// and `fee_atomic` as derived values, never as independent expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ServiceCost {
    /// Amount the upstream provider receives, in atomic USDC (6 decimals).
    provider_atomic: u64,
    /// Platform fee on top of the provider amount, in atomic USDC.
    fee_atomic: u64,
    /// Total amount the client must pay, in atomic USDC. Always equals
    /// `provider_atomic + fee_atomic`.
    total_atomic: u64,
}

/// Compute the full atomic-USDC cost breakdown for a service request.
///
/// Uses integer arithmetic to avoid floating-point precision loss on financial
/// amounts: the `price_usdc` is converted to atomic units (6 decimals) once via
/// the only `f64 → u64` cast in the path, then the 5% platform fee is applied
/// in pure integer math.
///
/// Returns `Err` (not `Ok` with zeros) on NaN/Inf/negative/overflow input so the
/// caller fails-closed — a corrupt registry entry must reject the request, not
/// serve it for free. See `validate_price_usdc` for the rationale.
fn compute_service_cost(price_usdc: f64) -> Result<ServiceCost, String> {
    validate_price_usdc(price_usdc)?;
    let provider_atomic = (price_usdc * 1_000_000.0).round() as u64;
    // 5% platform fee: total = provider * 105 / 100. `saturating_mul` is
    // belt-and-braces — `validate_price_usdc` already capped the magnitude.
    let total_atomic = provider_atomic.saturating_mul(105) / 100;
    let fee_atomic = total_atomic.saturating_sub(provider_atomic);
    Ok(ServiceCost {
        provider_atomic,
        fee_atomic,
        total_atomic,
    })
}

/// Where a paid service request settles, with the amounts attached to each leg.
///
/// "Vendor-Settlement Fee Mechanics" RFC (2026-06-12, mechanism C / record +
/// invoice, VENDOR ABSORBS the fee). Modeling the two paths as an enum (rather
/// than a tuple of loosely-coupled values) makes it a compile error to mix the
/// gateway's fee-on-top amounts with the vendor's fee-absorbed ones.
#[derive(Debug, Clone)]
enum PaymentTarget {
    /// Today's default: the agent pays `price × 1.05` to the gateway's global
    /// recipient wallet; the 5% is collected on-chain in the same transfer.
    Gateway { total_atomic: u64, fee_atomic: u64 },
    /// Settlement-platform P1: the agent pays EXACTLY the listed price to the
    /// service's `vendor_wallet`; Solvela's 5% is never charged to the agent —
    /// it is recorded as an off-chain receivable against the vendor
    /// (`VendorSettlement`, spend_logs).
    Vendor {
        price_atomic: u64,
        settlement: VendorSettlement,
    },
}

impl PaymentTarget {
    /// Atomic USDC amount the AGENT must pay (quoted in the 402 and enforced
    /// against the payment header's `amount`).
    fn expected_atomic(&self) -> u64 {
        match self {
            Self::Gateway { total_atomic, .. } => *total_atomic,
            Self::Vendor { price_atomic, .. } => *price_atomic,
        }
    }

    /// Fee charged to the AGENT, in atomic USDC — zero on the vendor path
    /// (the vendor absorbs it; rule #5: the cost_breakdown stays truthful to
    /// what the agent pays).
    fn agent_fee_atomic(&self) -> u64 {
        match self {
            Self::Gateway { fee_atomic, .. } => *fee_atomic,
            Self::Vendor { .. } => 0,
        }
    }

    /// Agent-facing fee rate for the 402 `cost_breakdown`. Keeping `5` next
    /// to a `0.000000` fee would contradict the amounts on the vendor path.
    fn agent_fee_percent(&self) -> u8 {
        match self {
            Self::Gateway { .. } => PLATFORM_FEE_PERCENT,
            Self::Vendor { .. } => 0,
        }
    }

    /// The vendor-settlement record to attach to the spend log; `None` on the
    /// gateway path. Consumes the target — call once, when building the entry.
    fn into_vendor_settlement(self) -> Option<VendorSettlement> {
        match self {
            Self::Gateway { .. } => None,
            Self::Vendor { settlement, .. } => Some(settlement),
        }
    }

    /// Build the client-facing receipt record (settlement-platform P2) for
    /// this settlement target. The amounts are AGENT-facing (rule #5 — the
    /// breakdown stays truthful to what the agent pays): on the vendor path
    /// the fee shown is 0 (the vendor absorbs it; the 5% rides as the
    /// receivable inside `vendor`), on the gateway path the fee is on top.
    ///
    /// `scheme` passed the scheme/payload-variant check and was settled by
    /// the facilitator; it is still client-originated, so it is capped before
    /// recording.
    fn receipt_record(
        &self,
        service_id: &str,
        scheme: &str,
        tx_signature: Option<String>,
        payer_wallet: String,
    ) -> receipts::ReceiptRecord {
        // Definitionally consistent on both variants — the provider/vendor leg
        // is what the agent pays minus the agent-facing fee (Gateway: total −
        // 5% fee = provider cost; Vendor: listed price − 0). Computed here, not
        // caller-supplied, so a call site can never record an inconsistent
        // provider/total pair. `expected_atomic ≥ agent_fee_atomic` holds by
        // construction (Gateway totals come from `compute_service_cost`, where
        // total = provider + fee).
        let provider_cost_atomic = self.expected_atomic() - self.agent_fee_atomic();
        receipts::ReceiptRecord {
            receipt_id: Uuid::new_v4(),
            model: service_id.to_string(),
            payment_scheme: scheme.chars().take(16).collect(),
            tx_signature,
            payer_wallet,
            amount_paid_atomic: self.expected_atomic(),
            provider_cost_atomic,
            platform_fee_atomic: self.agent_fee_atomic(),
            total_atomic: self.expected_atomic(),
            vendor: match self {
                Self::Gateway { .. } => None,
                Self::Vendor { settlement, .. } => Some(settlement.clone()),
            },
        }
    }
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

    // Compute the full cost breakdown once using integer arithmetic — used for
    // both the 402 response and the payment validation, so they cannot drift.
    //
    // `compute_service_cost` rejects NaN/Inf/negative/overflow `price_usdc` so
    // a corrupt registry entry surfaces as a 500, not as a free request.
    let ServiceCost {
        provider_atomic,
        fee_atomic,
        total_atomic,
    } = compute_service_cost(price_usdc).map_err(|e| {
        warn!(service_id = %service_id, error = %e, "invalid service pricing");
        GatewayError::Internal(format!(
            "service '{service_id}' has invalid price_per_request_usdc"
        ))
    })?;

    // Per-service settlement recipient (settlement-platform P1).
    //
    // "Vendor-Settlement Fee Mechanics" RFC (2026-06-12, mechanism C / record
    // + invoice, VENDOR ABSORBS the fee): for a service with a
    // `vendor_wallet`, the agent pays EXACTLY the listed price
    // (`provider_atomic`, no 5% on top) in a single transfer addressed to the
    // vendor wallet, and Solvela's 5% (`fee_atomic`, the canonical
    // `floor(price × 105 / 100) − price` from `compute_service_cost`) is
    // recorded as an off-chain receivable against the vendor in spend_logs.
    // Services without a vendor wallet keep today's behavior unchanged:
    // `price × 1.05` paid to the gateway's global recipient.
    //
    // See `PaymentTarget`: the vendor path quotes the agent ZERO fee (the
    // vendor absorbs it) and the 402 `cost_breakdown` must show exactly what
    // the agent pays (rule #5).
    let (pay_to_wallet, payment_target) = match service.vendor_wallet.as_deref() {
        Some(vendor_wallet) => {
            // AUTHORITATIVE pre-quote guard (fail closed before quoting or
            // charging): the receivable ledger columns are BIGINT, so a
            // priced amount that cannot be represented as i64 must reject
            // the request up front — never quote a 402 the ledger cannot
            // record. The single u64 → i64 conversion at the sqlx bind site
            // (`usage.rs::log_spend`) is a belt-and-braces re-check that
            // logs a reconcilable loss event instead of panicking; this
            // guard is what makes that arm unreachable.
            if i64::try_from(provider_atomic).is_err() {
                return Err(GatewayError::Internal(format!(
                    "service '{service_id}' price exceeds the recordable range"
                )));
            }
            if i64::try_from(fee_atomic).is_err() {
                return Err(GatewayError::Internal(format!(
                    "service '{service_id}' fee exceeds the recordable range"
                )));
            }
            (
                vendor_wallet.to_string(),
                PaymentTarget::Vendor {
                    price_atomic: provider_atomic,
                    settlement: VendorSettlement {
                        vendor_wallet: vendor_wallet.to_string(),
                        settled_atomic: provider_atomic,
                        fee_receivable_atomic: fee_atomic,
                    },
                },
            )
        }
        None => (
            state.config.solana.recipient_wallet.clone(),
            PaymentTarget::Gateway {
                total_atomic,
                fee_atomic,
            },
        ),
    };
    let expected_atomic = payment_target.expected_atomic();

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

        // Format cost breakdown from integer-derived values for display.
        // On the vendor path the agent fee is 0 and the total equals the
        // provider price — the breakdown reflects what the AGENT pays, not
        // the off-chain receivable the vendor owes.
        let provider_usdc = provider_atomic as f64 / 1_000_000.0;
        let fee_usdc = payment_target.agent_fee_atomic() as f64 / 1_000_000.0;
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
                // Quote the CONFIGURED mint (what the verifier enforces),
                // never the compile-time constant.
                asset: state.config.solana.usdc_mint.clone(),
                // Per-service recipient: the vendor wallet when configured,
                // the gateway's global recipient otherwise.
                pay_to: pay_to_wallet.clone(),
                max_timeout_seconds: solvela_x402::types::MAX_TIMEOUT_SECONDS,
                escrow_program_id: None,
            }],
            cost_breakdown: CostBreakdown {
                provider_cost: format!("{provider_usdc:.6}"),
                platform_fee: format!("{fee_usdc:.6}"),
                total: format!("{total_usdc:.6}"),
                currency: "USDC".to_string(),
                // Agent-facing fee rate: 0 on the vendor path (the vendor
                // absorbs the 5%, invoiced off-chain), 5 otherwise — see
                // `PaymentTarget::agent_fee_percent`.
                fee_percent: payment_target.agent_fee_percent(),
            },
            error: "Payment required".to_string(),
            // No Bazaar discovery block on the marketplace-proxy resource: that
            // block is chat-completions-shaped and per-service proxy schemas
            // vary. Scoped to /v1/chat/completions for now.
            extensions: None,
        };

        // Emit the PaymentRequired body at the top level of the 402 response
        // per x402 spec — NOT wrapped in the OpenAI-style error envelope.
        // Mirrors `routes::chat::chat_completions`. See
        // `GatewayError::PaymentChallenge` doc and issue #217.
        return Err(GatewayError::PaymentChallenge(Box::new(payment_required)));
    }

    // Step 4: Payment present — decode and verify
    let raw_header = payment_header.expect("checked above");
    let payload = decode_payment_header(raw_header).map_err(|e| {
        // Don't echo the parser error — log it internally and return a
        // sanitized message. See `GatewayError::InvalidPayment` doc.
        warn!(error = %e, "PAYMENT-SIGNATURE header decode failed (proxy)");
        GatewayError::InvalidPayment(
            "PAYMENT-SIGNATURE header could not be decoded. \
             Encode a valid PaymentPayload as standard base64 JSON."
                .to_string(),
        )
    })?;

    // Validate resource URL matches this proxy endpoint
    let expected_url = format!("/v1/services/{service_id}/proxy");
    if payload.resource.url != expected_url {
        // GHSA-cgqx-mg48-949v posture: `resource.url` is client-controlled
        // and unbounded (the canonical envelope imposes no length cap on it),
        // so it is never reflected to the client. Log a truncated copy
        // server-side; the client gets a fixed string — the 402 challenge
        // already tells it the correct resource.
        let got: String = payload.resource.url.chars().take(256).collect();
        warn!(
            expected = %expected_url,
            got = %got,
            "payment resource URL mismatch (proxy)"
        );
        return Err(GatewayError::InvalidPayment(
            "Payment resource does not match this endpoint.".to_string(),
        ));
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

    // Validate asset is the CONFIGURED USDC-SPL mint — the same one the
    // verifier enforces — not the compile-time constant.
    if payload.accepted.asset != state.config.solana.usdc_mint {
        // GHSA-cgqx-mg48-949v posture: `asset` is client-controlled — cap to
        // the max base58 pubkey length (44 chars) before echoing/logging
        // (mirrors the tx_prefix truncation in a2a/handler.rs; chars-based so
        // a multibyte boundary can never panic).
        let asset_prefix: String = payload.accepted.asset.chars().take(44).collect();
        return Err(GatewayError::BadRequest(format!(
            "payment asset must be USDC mint '{}', got '{asset_prefix}'",
            state.config.solana.usdc_mint
        )));
    }

    // Validate pay_to matches this service's expected recipient — the vendor
    // wallet when configured, the gateway's global recipient otherwise. Hard
    // equality both ways: a vendor service paid to the global wallet is
    // rejected, and vice versa.
    if payload.accepted.pay_to != pay_to_wallet {
        // GHSA-cgqx-mg48-949v posture: `pay_to` is client-controlled — cap to
        // the max base58 pubkey length before echoing (mirrors the asset
        // truncation above).
        let pay_to_prefix: String = payload.accepted.pay_to.chars().take(44).collect();
        return Err(GatewayError::BadRequest(format!(
            "payment pay_to must be '{pay_to_wallet}', got '{pay_to_prefix}'"
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

    // Issue #499: reject `require_tenant = TRUE` wallets on this path.
    //
    // The service-marketplace proxy does NOT run `check_budget`'s per-tenant
    // budget matrix (see the spend-log note below), so a wallet provisioned as
    // `require_tenant = TRUE` — meaning "may only spend under a tagged,
    // provisioned tenant" — would otherwise bypass that fail-closed guarantee
    // here. Rather than add full per-tenant metering to the proxy (deliberately
    // out of scope), we REJECT before any replay record, settlement, or upstream
    // call, so a rejected request takes no spend. The wallet must use
    // `POST /v1/chat/completions`, which enforces the tenant matrix.
    //
    // Degradation matches the chat path exactly: `require_tenant_for_wallet`
    // returns `false` (do not reject) when Redis is absent or on a transient
    // Redis/DB read error (the wallet caps are the backstop) — see its doc.
    let payer_wallet = extract_payer_wallet(&payload);
    if state.usage.require_tenant_for_wallet(&payer_wallet).await {
        warn!(
            service_id = %service_id,
            "proxy request rejected: payer wallet requires per-tenant budgeting (#499)"
        );
        return Err(GatewayError::Forbidden(
            "this wallet requires per-tenant budgeting; use POST /v1/chat/completions".to_string(),
        ));
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
            // Log only the signature prefix, not the full base64 tx (attacker-
            // controlled, would pollute log pipelines).
            warn!(
                tx_prefix = &tx_raw[..tx_raw.len().min(88)],
                "durable-nonce proxy payment rejected: Redis unavailable (GHSA-fq3f-c8p7-873f)"
            );
            return Err(GatewayError::InvalidPayment(
                "Payment service is temporarily degraded; please retry shortly.".to_string(),
            ));
        }
        // GHSA-wc9q-wc6q-gwmq: recover from a poisoned lock so a single panic
        // in another payment request does not turn this Mutex into a tarpit
        // for every subsequent request.
        let mut replay_set = state
            .replay_set
            .for_path(crate::ReplayPath::Proxy)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        let found = match replay_set.get(tx_raw) {
            Some(&inserted_at) if now.duration_since(inserted_at) < crate::AppState::REPLAY_TTL => {
                true
            }
            Some(_) => {
                // Entry expired — evict so its LRU slot can be reclaimed,
                // then treat as not seen.
                replay_set.pop(tx_raw);
                false
            }
            None => false,
        };
        if found {
            true
        } else {
            replay_set.put(tx_raw.to_string(), now);
            warn!(
                tx_prefix = &tx_raw[..tx_raw.len().min(88)],
                "payment accepted under degraded in-memory replay protection (no Redis)"
            );
            false
        }
    };

    if replay_detected {
        counter!("solvela_replay_rejections_total").increment(1);
        counter!("solvela_payments_total", "status" => "failed").increment(1);
        warn!(
            tx_prefix = &tx_raw[..tx_raw.len().min(88)],
            "replay attack detected — transaction already used"
        );
        return Err(GatewayError::InvalidPayment(
            "transaction has already been used; each payment signature may only be submitted once"
                .to_string(),
        ));
    }

    // Verify and settle via Facilitator — hard enforcement.
    // The chat handler (`routes::chat`) explicitly rejects `Ok(settlement)` whose
    // `success` flag is `false` (e.g. RPC confirmation timed out). We mirror that
    // here so an unconfirmed transaction cannot be forwarded to the upstream
    // service as a paid request.
    //
    // Vendor-wallet services verify against the PER-SERVICE recipient via
    // `verify_and_settle_to` — the verifier re-derives the expected ATA from
    // the vendor wallet and enforces hard equality, and fails closed
    // (`RecipientOverrideUnsupported`) for any verifier that does not support
    // recipient override (e.g. the escrow scheme, whose vendor split is P4).
    // Non-vendor services keep today's `verify_and_settle` path unchanged.
    let settlement_outcome = match &payment_target {
        PaymentTarget::Vendor { .. } => {
            state
                .facilitator
                .verify_and_settle_to(&payload, &pay_to_wallet)
                .await
        }
        PaymentTarget::Gateway { .. } => state.facilitator.verify_and_settle(&payload).await,
    };
    match settlement_outcome {
        Ok(settlement) if !settlement.success => {
            counter!("solvela_payments_total", "status" => "failed").increment(1);
            warn!(
                tx_signature = %settlement.tx_signature.as_deref().unwrap_or("unknown"),
                error = ?settlement.error,
                service_id = %service_id,
                "proxy payment settlement failed: transaction not confirmed"
            );
            return Err(GatewayError::InvalidPayment(
                "Payment transaction could not be confirmed. Please retry.".to_string(),
            ));
        }
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

    // The actual PAYER wallet (not the recipient pay_to address) was already
    // extracted above for the #499 require_tenant gate; reuse it.
    let wallet_address = payer_wallet;
    let tx_signature = match &payload.payload {
        solvela_x402::types::PayloadData::Direct(p) => Some(p.transaction.clone()),
        solvela_x402::types::PayloadData::Escrow(p) => Some(p.deposit_tx.clone()),
    };

    // Extract request ID for traceability
    let request_id: Option<String> = headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    // Build the spend entry once (fire-and-forget: log_spend spawns its own
    // writes, never awaited on the hot path).
    //
    // WHEN it is logged differs by path:
    // - Vendor services log NOW, at settlement time: the 5% receivable is owed
    //   on SETTLED volume (the vendor was just paid on-chain), so deferring
    //   the record past the upstream call would silently drop receivables for
    //   settled requests whose upstream then times out or is unreachable.
    // - Non-vendor services keep the legacy post-upstream write below,
    //   byte-for-byte today's behavior.
    // P2 receipt: built from the same settlement facts as the spend entry and
    // written at the same point(s) — vendor services NOW (the vendor was just
    // paid on-chain), plain services post-upstream below — so every paid
    // request that writes a spend row also writes a receipt row.
    let receipt_record = payment_target.receipt_record(
        &service_id,
        &payload.accepted.scheme,
        tx_signature.clone(),
        wallet_address.clone(),
    );

    let spend_entry = SpendLogEntry {
        wallet_address,
        model: service_id.clone(),
        provider: "external-service".to_string(),
        input_tokens: 0,
        output_tokens: 0,
        cost_usdc: expected_atomic as f64 / 1_000_000.0,
        tx_signature,
        request_id: request_id.clone(),
        session_id: None,
        // PR1 keeps the service-marketplace proxy minimal: it does not read the
        // x-tenant header. Per-tenant attribution for the proxy path can be
        // added later; None leaves these rows untagged for now.
        tenant: None,
        // No tenant tag is read on the proxy path, so no per-tenant bucket is
        // enforced — never reconcile per-tenant counters here.
        tenant_enforced: false,
        // Service-marketplace proxy doesn't go through `check_budget`
        // (it has flat per-request pricing, not per-wallet budgets), so no
        // reservation is committed and log_spend increments by the full
        // cost. None preserves the legacy behavior here.
        estimated_cost_usdc: None,
        vendor: payment_target.into_vendor_settlement(),
    };
    // `receipt_header_path` is `Some` once a retrievable receipt exists (DB
    // configured AND the row write was dispatched); it is attached to every
    // response built after that point. With no DATABASE_URL it stays `None`
    // and no header is ever emitted (never promise an unfetchable receipt).
    let (deferred_spend_entry, mut receipt_header_path) = if spend_entry.vendor.is_some() {
        state.usage.log_spend(spend_entry);
        let path = receipts::record_receipt(state.db_pool.as_ref(), receipt_record);
        (None, path)
    } else {
        (Some((spend_entry, receipt_record)), None)
    };

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
    //
    // `redirect::Policy::none()` is critical: reqwest's default policy follows
    // up to 10 redirects, each with a fresh DNS lookup that bypasses the
    // `.resolve()` pin AND `resolve_and_validate_endpoint`. Without this,
    // a `302 Location: http://169.254.169.254/...` from a registered upstream
    // would land us inside cloud metadata or other internal infrastructure.
    let pinned_client = reqwest::Client::builder()
        .resolve(&validated_host, validated_addr)
        .redirect(reqwest::redirect::Policy::none())
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
            // Vendor services already settled AND recorded their receipt at
            // settlement time above, so the paid (504) response still carries
            // it. Plain services have not written spend or receipt yet —
            // `receipt_header_path` is `None` and no header is emitted
            // (mirrors the legacy spend-log behavior on this arm).
            let mut response = (
                StatusCode::GATEWAY_TIMEOUT,
                axum::Json(json!({
                    "error": "upstream service timed out"
                })),
            )
                .into_response();
            receipts::insert_receipt_header(&mut response, &receipt_header_path);
            return Ok(response);
        }
        Err(e) => {
            warn!(
                service_id = %service_id,
                error = %e,
                "failed to reach upstream service"
            );
            // Same receipt-header semantics as the timeout arm above.
            let mut response = (
                StatusCode::BAD_GATEWAY,
                axum::Json(json!({
                    "error": "service unreachable"
                })),
            )
                .into_response();
            receipts::insert_receipt_header(&mut response, &receipt_header_path);
            return Ok(response);
        }
    };

    let upstream_status = upstream_response.status();

    // Step 7: Fire-and-forget spend log + receipt (both internally use
    // tokio::spawn). Non-vendor services only — vendor services already
    // logged at settlement time above (one row per request either way).
    if let Some((entry, record)) = deferred_spend_entry {
        state.usage.log_spend(entry);
        receipt_header_path = receipts::record_receipt(state.db_pool.as_ref(), record);
    }

    // Handle upstream response based on status
    if upstream_status.is_server_error() {
        warn!(
            service_id = %service_id,
            upstream_status = %upstream_status,
            "upstream service returned server error"
        );
        let mut response = (
            StatusCode::BAD_GATEWAY,
            axum::Json(json!({
                "error": "upstream service error"
            })),
        )
            .into_response();
        receipts::insert_receipt_header(&mut response, &receipt_header_path);
        return Ok(response);
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

    let mut response = builder
        .body(Body::from(response_bytes))
        .map_err(|e| GatewayError::Internal(format!("failed to build response: {e}")))?;
    receipts::insert_receipt_header(&mut response, &receipt_header_path);
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payment_util::extract_signer_from_base64_tx;
    use solvela_x402::types::USDC_MINT;

    /// Convenience: total atomic cost only — most existing tests only care
    /// about the total, not the full breakdown.
    fn total_atomic(price_usdc: f64) -> Result<u64, String> {
        compute_service_cost(price_usdc).map(|c| c.total_atomic)
    }

    #[test]
    fn test_compute_service_cost_basic() {
        // 0.01 USDC = 10_000 atomic; with 5% fee = 10_500 total, fee = 500
        let cost = compute_service_cost(0.01).unwrap();
        assert_eq!(cost.provider_atomic, 10_000);
        assert_eq!(cost.fee_atomic, 500);
        assert_eq!(cost.total_atomic, 10_500);
    }

    #[test]
    fn test_compute_service_cost_breakdown_sums_to_total() {
        // The invariant the call site relies on for the 402 response: the
        // displayed provider + fee always reconstruct the total.
        for price in [0.0, 0.001, 0.01, 0.0042, 1.0, 12.345, 1_000.0] {
            let cost = compute_service_cost(price).unwrap();
            assert_eq!(
                cost.provider_atomic + cost.fee_atomic,
                cost.total_atomic,
                "breakdown invariant broken for price={price}"
            );
        }
    }

    #[test]
    fn test_compute_service_cost_small() {
        assert_eq!(total_atomic(0.001).unwrap(), 1_050);
    }

    #[test]
    fn test_compute_service_cost_zero() {
        assert_eq!(total_atomic(0.0).unwrap(), 0);
    }

    #[test]
    fn test_compute_service_cost_large() {
        assert_eq!(total_atomic(1.0).unwrap(), 1_050_000);
    }

    #[test]
    fn test_compute_service_cost_consistency() {
        let price = 0.002625;
        let result1 = compute_service_cost(price).unwrap();
        let result2 = compute_service_cost(price).unwrap();
        assert_eq!(result1, result2);
    }

    #[test]
    fn test_compute_service_cost_uses_round_not_truncate() {
        // 0.0000015 USDC = 1.5 atomic -> rounds to 2 -> 2 * 105/100 = 2
        assert_eq!(total_atomic(0.0000015).unwrap(), 2);
    }

    // =========================================================================
    // HIGH-2: NaN/Inf/negative/overflow guards on price_per_request_usdc
    //
    // Without these guards, a corrupt `services.toml` entry with `price = nan`
    // makes `(price * 1e6).round() as u64` return 0, and the proxy serves the
    // upstream request for free.
    // =========================================================================

    #[test]
    fn test_compute_service_cost_rejects_nan() {
        let result = compute_service_cost(f64::NAN);
        assert!(result.is_err(), "NaN price must be rejected");
        let err = result.unwrap_err();
        assert!(
            err.contains("non-finite"),
            "error must mention non-finite, got: {err}"
        );
    }

    #[test]
    fn test_compute_service_cost_rejects_positive_infinity() {
        let result = compute_service_cost(f64::INFINITY);
        assert!(result.is_err(), "+inf price must be rejected");
        let err = result.unwrap_err();
        assert!(
            err.contains("non-finite"),
            "error must mention non-finite, got: {err}"
        );
    }

    #[test]
    fn test_compute_service_cost_rejects_negative_infinity() {
        let result = compute_service_cost(f64::NEG_INFINITY);
        assert!(result.is_err(), "-inf price must be rejected");
    }

    #[test]
    fn test_compute_service_cost_rejects_negative() {
        let result = compute_service_cost(-0.001);
        assert!(result.is_err(), "negative price must be rejected");
        let err = result.unwrap_err();
        assert!(
            err.contains("negative"),
            "error must mention negative, got: {err}"
        );
    }

    #[test]
    fn test_compute_service_cost_rejects_overflow() {
        // PRICE_USDC_MAX ≈ 1.84e13. Anything above must reject so the
        // ×1_000_000 multiplication cannot overflow u64.
        let result = compute_service_cost(1.0e18_f64);
        assert!(result.is_err(), "overflowing price must be rejected");
        let err = result.unwrap_err();
        assert!(
            err.contains("exceeds u64 range") || err.contains("overflow"),
            "error must mention overflow/range, got: {err}"
        );
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
            vendor_wallet: None,
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
            semantic_cache: None,
            provider_health: ProviderHealthTracker::new(CircuitBreakerConfig::default()),
            escrow_claimer: None,
            fee_payer_pool: None,
            nonce_pool: None,
            db_pool: None,
            faucet: None,
            session_secret: b"test-secret".to_vec(),
            http_client: reqwest::Client::new(),
            replay_set: crate::AppState::new_replay_set(),
            slot_cache: new_slot_cache(),
            escrow_metrics: None,
            admin_token: None,
            api_key_hmac_secret: None,
            auth_provider: None,
            prometheus_handle: None,
            dev_bypass_payment: false,
            free_rate_limiter: crate::middleware::rate_limit::RateLimiter::new(crate::middleware::rate_limit::RateLimitConfig::free_default()),
            receipts_rate_limiter: crate::middleware::rate_limit::RateLimiter::new(crate::middleware::rate_limit::RateLimitConfig::receipts_default()),
            faucet_rate_limiter: crate::middleware::rate_limit::RateLimiter::new(crate::middleware::rate_limit::RateLimitConfig::faucet_default()),
            deposit_tx_rate_limiter: crate::middleware::rate_limit::RateLimiter::new(crate::middleware::rate_limit::RateLimitConfig::deposit_tx_default()),
            free_global_cap: crate::middleware::rate_limit::FreeTierGlobalCap::new(crate::middleware::rate_limit::FREE_TIER_GLOBAL_RPM_DEFAULT),
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

    // ---------------------------------------------------------------------
    // Validation / lookup branch coverage
    //
    // Each test calls `proxy_service` directly (no Router) and asserts on
    // the GatewayError variant. Tests cover:
    //   - service registry lookup misses + service-config rejections
    //   - 402 with PaymentRequired body when no header is present
    //   - PAYMENT-SIGNATURE encoding / decode failures
    //   - every payment-payload field validation gate
    //   - scheme/payload mismatch detection
    //   - facilitator failure (empty verifier list)
    //   - in-memory replay detection (non-durable-nonce path)
    // ---------------------------------------------------------------------

    use crate::services::{ServiceEntry, ServiceRegistry};
    use crate::AppState;
    use axum::body::Body;
    use axum::extract::{Path, State};
    use axum::http::HeaderMap;
    use base64::Engine;
    use std::sync::Arc;
    use tokio::sync::RwLock;

    /// Construct an AppState with a single configured service.
    /// Service id is always "test-svc"; tunable fields:
    /// - `price`: `price_per_request_usdc` (None = no pricing)
    /// - `internal`: rejects with "use /v1/chat/completions"
    /// - `x402_enabled`: rejects with "service does not support x402 payments"
    /// - `healthy`: `Some(false)` returns 503; `Some(true)` and `None` proceed
    fn make_state_with_service(
        price: Option<f64>,
        internal: bool,
        x402_enabled: bool,
        healthy: Option<bool>,
    ) -> Arc<AppState> {
        // Endpoint shape must match the validator's rule: internal services
        // use a relative path; external services use an absolute https URL.
        let endpoint = if internal {
            "/v1/test".to_string()
        } else {
            "https://test.example.com/api".to_string()
        };
        make_state_with_entry(
            ServiceEntry {
                id: "test-svc".to_string(),
                name: "Test Service".to_string(),
                category: "test".to_string(),
                endpoint,
                x402_enabled,
                internal,
                description: None,
                pricing_label: "per-request".to_string(),
                chains: vec!["solana".to_string()],
                source: "api".to_string(),
                healthy,
                price_per_request_usdc: price,
                vendor_wallet: None,
            },
            // AppConfig::default() leaves recipient_wallet empty — the legacy
            // tests above build payloads with `pay_to: String::new()`.
            "",
        )
    }

    /// Construct an AppState from a fully-specified service entry and a
    /// configured global recipient wallet.
    fn make_state_with_entry(entry: ServiceEntry, recipient_wallet: &str) -> Arc<AppState> {
        use crate::config::AppConfig;
        use crate::providers::health::{CircuitBreakerConfig, ProviderHealthTracker};
        use crate::providers::ProviderRegistry;
        use crate::routes::escrow::new_slot_cache;
        use crate::usage::UsageTracker;
        use solvela_router::models::ModelRegistry;
        use solvela_x402::facilitator::Facilitator;

        let mut reg = ServiceRegistry::empty();
        reg.register(entry).expect("valid test service entry");

        let mut config = AppConfig::default();
        config.solana.recipient_wallet = recipient_wallet.to_string();

        Arc::new(AppState {
            config,
            model_registry: ModelRegistry::from_toml(
                "[models.placeholder]\nprovider=\"t\"\nmodel_id=\"t\"\ndisplay_name=\"T\"\n\
                 input_cost_per_million=1.0\noutput_cost_per_million=1.0\ncontext_window=4096\n\
                 supports_streaming=false\nsupports_tools=false\nsupports_vision=false",
            )
            .expect("valid placeholder model TOML"),
            service_registry: RwLock::new(reg),
            providers: ProviderRegistry::from_env(reqwest::Client::new()),
            facilitator: Facilitator::new(vec![]),
            usage: UsageTracker::noop(),
            cache: None,
            semantic_cache: None,
            provider_health: ProviderHealthTracker::new(CircuitBreakerConfig::default()),
            escrow_claimer: None,
            fee_payer_pool: None,
            nonce_pool: None,
            db_pool: None,
            faucet: None,
            session_secret: b"test-secret".to_vec(),
            http_client: reqwest::Client::new(),
            replay_set: AppState::new_replay_set(),
            slot_cache: new_slot_cache(),
            escrow_metrics: None,
            admin_token: None,
            api_key_hmac_secret: None,
            auth_provider: None,
            prometheus_handle: None,
            dev_bypass_payment: false,
            free_rate_limiter: crate::middleware::rate_limit::RateLimiter::new(
                crate::middleware::rate_limit::RateLimitConfig::free_default(),
            ),
            receipts_rate_limiter: crate::middleware::rate_limit::RateLimiter::new(
                crate::middleware::rate_limit::RateLimitConfig::receipts_default(),
            ),
            faucet_rate_limiter: crate::middleware::rate_limit::RateLimiter::new(
                crate::middleware::rate_limit::RateLimitConfig::faucet_default(),
            ),
            deposit_tx_rate_limiter: crate::middleware::rate_limit::RateLimiter::new(
                crate::middleware::rate_limit::RateLimitConfig::deposit_tx_default(),
            ),
            free_global_cap: crate::middleware::rate_limit::FreeTierGlobalCap::new(
                crate::middleware::rate_limit::FREE_TIER_GLOBAL_RPM_DEFAULT,
            ),
        })
    }

    /// Build a structurally-valid PaymentPayload for `service_id` at $0.01
    /// (10_500 atomic with 5% fee). Caller can mutate the returned value
    /// to corrupt one field per test.
    fn valid_payload(service_id: &str) -> solvela_x402::types::PaymentPayload {
        solvela_x402::types::PaymentPayload {
            x402_version: 1,
            resource: solvela_x402::types::Resource {
                url: format!("/v1/services/{service_id}/proxy"),
                method: "POST".to_string(),
            },
            accepted: solvela_x402::types::PaymentAccept {
                scheme: "exact".to_string(),
                network: SOLANA_NETWORK.to_string(),
                amount: "10500".to_string(),
                asset: USDC_MINT.to_string(),
                // AppConfig::default().solana.recipient_wallet is empty by default.
                pay_to: String::new(),
                max_timeout_seconds: 300,
                escrow_program_id: None,
            },
            payload: solvela_x402::types::PayloadData::Direct(solvela_x402::types::SolanaPayload {
                transaction: format!("test_tx_{}", uuid::Uuid::new_v4().simple()),
            }),
        }
    }

    fn payload_to_header(p: &solvela_x402::types::PaymentPayload) -> String {
        base64::engine::general_purpose::STANDARD.encode(serde_json::to_vec(p).expect("ser"))
    }

    fn make_headers_with_payment(p: &solvela_x402::types::PaymentPayload) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            "payment-signature",
            payload_to_header(p).parse().expect("ascii"),
        );
        h
    }

    /// Convenience: drive `proxy_service` directly and unwrap the error variant.
    async fn call_expect_err(
        state: Arc<AppState>,
        service_id: &str,
        headers: HeaderMap,
    ) -> GatewayError {
        proxy_service(
            State(state),
            Path(service_id.to_string()),
            headers,
            Body::empty(),
        )
        .await
        .expect_err("expected GatewayError")
    }

    // ── Lookup / config rejections ────────────────────────────────────────

    #[tokio::test]
    async fn proxy_returns_404_when_service_not_found() {
        let state = make_state_with_service(Some(0.01), false, true, None);
        let err = call_expect_err(state, "missing-svc", HeaderMap::new()).await;
        assert!(
            matches!(err, GatewayError::ModelNotFound(ref m) if m.contains("missing-svc")),
            "expected ModelNotFound, got {err:?}"
        );
    }

    #[tokio::test]
    async fn proxy_rejects_internal_service() {
        let state = make_state_with_service(Some(0.01), /*internal=*/ true, true, None);
        let err = call_expect_err(state, "test-svc", HeaderMap::new()).await;
        assert!(
            matches!(err, GatewayError::BadRequest(ref m) if m.contains("internal")),
            "expected BadRequest about internal services, got {err:?}"
        );
    }

    #[tokio::test]
    async fn proxy_rejects_service_without_x402() {
        let state = make_state_with_service(Some(0.01), false, /*x402_enabled=*/ false, None);
        let err = call_expect_err(state, "test-svc", HeaderMap::new()).await;
        assert!(
            matches!(err, GatewayError::BadRequest(ref m) if m.contains("x402")),
            "expected BadRequest about x402 support, got {err:?}"
        );
    }

    #[tokio::test]
    async fn proxy_returns_503_for_unhealthy_service() {
        let state = make_state_with_service(Some(0.01), false, true, Some(false));
        let response = proxy_service(
            State(state),
            Path("test-svc".to_string()),
            HeaderMap::new(),
            Body::empty(),
        )
        .await
        .expect("unhealthy must return Ok(503), not GatewayError");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    // Note: the "no price configured" branch in proxy_service is defensive
    // against corrupt TOML state. ServiceRegistry::register rejects
    // price=None with InvalidPrice, so it can't be exercised without
    // bypassing the registry's validated entry path.

    // ── Payment header gating ────────────────────────────────────────────

    #[tokio::test]
    async fn proxy_returns_402_with_payment_required_when_no_header() {
        let state = make_state_with_service(Some(0.01), false, true, None);
        let err = call_expect_err(state, "test-svc", HeaderMap::new()).await;
        match err {
            // Issue #217: 402 now carries a top-level `PaymentRequired`
            // via the `PaymentChallenge` variant, not a stringified-JSON
            // body inside `InvalidPayment`. Assertions read the same
            // fields off the struct directly.
            GatewayError::PaymentChallenge(pr) => {
                // pr: Box<PaymentRequired> — auto-derefs for field access.
                assert_eq!(pr.accepts[0].amount, "10500");
                assert_eq!(pr.accepts[0].asset, USDC_MINT);
                assert_eq!(pr.cost_breakdown.currency, "USDC");
            }
            other => panic!("expected PaymentChallenge with PaymentRequired body, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn proxy_rejects_non_ascii_payment_header() {
        let state = make_state_with_service(Some(0.01), false, true, None);
        let mut headers = HeaderMap::new();
        // \xff is not valid UTF-8 / ASCII — to_str() fails on the value.
        headers.insert(
            "payment-signature",
            axum::http::HeaderValue::from_bytes(b"\xff").expect("raw bytes"),
        );
        let err = call_expect_err(state, "test-svc", headers).await;
        assert!(
            matches!(err, GatewayError::BadRequest(ref m) if m.contains("encoding")),
            "expected BadRequest about header encoding, got {err:?}"
        );
    }

    #[tokio::test]
    async fn proxy_rejects_undecodable_payment_header() {
        let state = make_state_with_service(Some(0.01), false, true, None);
        let mut headers = HeaderMap::new();
        headers.insert("payment-signature", "not-a-payment".parse().expect("ascii"));
        let err = call_expect_err(state, "test-svc", headers).await;
        assert!(
            matches!(err, GatewayError::InvalidPayment(_)),
            "expected InvalidPayment from decode failure, got {err:?}"
        );
    }

    // ── Per-field payload validation ──────────────────────────────────────

    #[tokio::test]
    async fn proxy_rejects_payment_with_wrong_resource_url() {
        let state = make_state_with_service(Some(0.01), false, true, None);
        let mut p = valid_payload("test-svc");
        p.resource.url = "/v1/services/other-svc/proxy".to_string();
        let err = call_expect_err(state, "test-svc", make_headers_with_payment(&p)).await;
        assert!(
            matches!(err, GatewayError::InvalidPayment(ref m) if m.contains("does not match")),
            "expected InvalidPayment about resource mismatch, got {err:?}"
        );
    }

    #[tokio::test]
    async fn proxy_rejects_payment_with_wrong_method() {
        let state = make_state_with_service(Some(0.01), false, true, None);
        let mut p = valid_payload("test-svc");
        p.resource.method = "GET".to_string();
        let err = call_expect_err(state, "test-svc", make_headers_with_payment(&p)).await;
        assert!(
            matches!(err, GatewayError::BadRequest(ref m) if m.contains("POST")),
            "expected BadRequest about method, got {err:?}"
        );
    }

    #[tokio::test]
    async fn proxy_rejects_payment_with_wrong_network() {
        let state = make_state_with_service(Some(0.01), false, true, None);
        let mut p = valid_payload("test-svc");
        p.accepted.network = "ethereum:mainnet".to_string();
        let err = call_expect_err(state, "test-svc", make_headers_with_payment(&p)).await;
        assert!(
            matches!(err, GatewayError::BadRequest(ref m) if m.contains("network")),
            "expected BadRequest about network, got {err:?}"
        );
    }

    #[tokio::test]
    async fn proxy_rejects_payment_with_wrong_asset() {
        let state = make_state_with_service(Some(0.01), false, true, None);
        let mut p = valid_payload("test-svc");
        p.accepted.asset = "FakeMint11111111111111111111111111111111111".to_string();
        let err = call_expect_err(state, "test-svc", make_headers_with_payment(&p)).await;
        assert!(
            matches!(err, GatewayError::BadRequest(ref m) if m.contains("USDC mint")),
            "expected BadRequest about asset/USDC mint, got {err:?}"
        );
    }

    #[tokio::test]
    async fn proxy_rejects_payment_with_wrong_pay_to() {
        let state = make_state_with_service(Some(0.01), false, true, None);
        let mut p = valid_payload("test-svc");
        p.accepted.pay_to = "WrongRecipientWallet1111111111111111111111".to_string();
        let err = call_expect_err(state, "test-svc", make_headers_with_payment(&p)).await;
        assert!(
            matches!(err, GatewayError::BadRequest(ref m) if m.contains("pay_to")),
            "expected BadRequest about pay_to, got {err:?}"
        );
    }

    #[tokio::test]
    async fn proxy_rejects_payment_with_unparseable_amount() {
        let state = make_state_with_service(Some(0.01), false, true, None);
        let mut p = valid_payload("test-svc");
        p.accepted.amount = "not-a-number".to_string();
        let err = call_expect_err(state, "test-svc", make_headers_with_payment(&p)).await;
        assert!(
            matches!(err, GatewayError::BadRequest(ref m) if m.contains("amount format")),
            "expected BadRequest about amount format, got {err:?}"
        );
    }

    #[tokio::test]
    async fn proxy_rejects_payment_with_insufficient_amount() {
        let state = make_state_with_service(Some(0.01), false, true, None);
        let mut p = valid_payload("test-svc");
        p.accepted.amount = "10000".to_string(); // expected is 10_500
        let err = call_expect_err(state, "test-svc", make_headers_with_payment(&p)).await;
        assert!(
            matches!(err, GatewayError::BadRequest(ref m) if m.contains("insufficient")),
            "expected BadRequest about insufficient amount, got {err:?}"
        );
    }

    #[tokio::test]
    async fn proxy_rejects_exact_scheme_with_escrow_payload() {
        let state = make_state_with_service(Some(0.01), false, true, None);
        let mut p = valid_payload("test-svc");
        // scheme="exact" but payload is escrow-shaped — must reject.
        p.payload = solvela_x402::types::PayloadData::Escrow(solvela_x402::types::EscrowPayload {
            deposit_tx: "dGVzdA==".to_string(),
            service_id: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_string(),
            agent_pubkey: "9noXzpXnkyEcKF3AeXqUHTdR59V5uvrRBUo9bwsHaByz".to_string(),
        });
        let err = call_expect_err(state, "test-svc", make_headers_with_payment(&p)).await;
        assert!(
            matches!(err, GatewayError::BadRequest(ref m) if m.contains("exact") && m.contains("escrow")),
            "expected BadRequest about scheme/payload mismatch, got {err:?}"
        );
    }

    #[tokio::test]
    async fn proxy_rejects_escrow_scheme_with_direct_payload() {
        let state = make_state_with_service(Some(0.01), false, true, None);
        let mut p = valid_payload("test-svc");
        p.accepted.scheme = "escrow".to_string();
        // payload is still Direct — must reject for the inverse mismatch.
        let err = call_expect_err(state, "test-svc", make_headers_with_payment(&p)).await;
        assert!(
            matches!(err, GatewayError::BadRequest(ref m) if m.contains("escrow") && m.contains("direct")),
            "expected BadRequest about escrow/direct mismatch, got {err:?}"
        );
    }

    // ── Replay + facilitator path ────────────────────────────────────────

    #[tokio::test]
    async fn proxy_facilitator_failure_returns_invalid_payment() {
        // With Facilitator::new(vec![]) the verify call fails → InvalidPayment
        // with the GHSA-cgqx-mg48-949v sanitized error message.
        let state = make_state_with_service(Some(0.01), false, true, None);
        let p = valid_payload("test-svc");
        let err = call_expect_err(state, "test-svc", make_headers_with_payment(&p)).await;
        match err {
            GatewayError::InvalidPayment(msg) => {
                assert!(msg.contains("verification failed"), "msg: {msg}");
                assert!(
                    !msg.contains("verifier"),
                    "must not leak verifier internals: {msg}"
                );
            }
            other => panic!("expected InvalidPayment from empty facilitator, got {other:?}"),
        }
    }

    // ── Per-service vendor_wallet recipient (settlement-platform P1) ──────

    /// Vendor wallet used by the vendor-recipient tests (valid base58 pubkey).
    const TEST_VENDOR_WALLET: &str = "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM";
    /// Global recipient configured on the gateway in vendor tests (valid
    /// base58 pubkey, distinct from the vendor wallet).
    const TEST_GLOBAL_RECIPIENT: &str = "So11111111111111111111111111111111111111112";

    /// State with one external service ("test-svc", $0.01) whose
    /// `vendor_wallet` is the caller's choice, and a configured global
    /// recipient of `TEST_GLOBAL_RECIPIENT`.
    fn make_state_with_vendor_service(vendor_wallet: Option<&str>) -> Arc<AppState> {
        make_state_with_vendor_service_priced(vendor_wallet, 0.01)
    }

    /// Like [`make_state_with_vendor_service`] but with a caller-supplied
    /// `price_per_request_usdc`, for boundary pricing (fee floor, i64 range).
    fn make_state_with_vendor_service_priced(
        vendor_wallet: Option<&str>,
        price_usdc: f64,
    ) -> Arc<AppState> {
        make_state_with_entry(
            ServiceEntry {
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
                price_per_request_usdc: Some(price_usdc),
                vendor_wallet: vendor_wallet.map(String::from),
            },
            TEST_GLOBAL_RECIPIENT,
        )
    }

    /// (c) 402 challenge: a vendor_wallet service must advertise the vendor
    /// wallet as `pay_to` and the LISTED PRICE as the amount (vendor absorbs
    /// the fee — the agent is never charged the 5% on this path), with a
    /// cost_breakdown that stays truthful to what the agent pays (rule #5):
    /// platform_fee = 0, total = provider price.
    #[tokio::test]
    async fn proxy_402_advertises_vendor_wallet_as_pay_to() {
        let state = make_state_with_vendor_service(Some(TEST_VENDOR_WALLET));
        let err = call_expect_err(state, "test-svc", HeaderMap::new()).await;
        match err {
            GatewayError::PaymentChallenge(pr) => {
                assert_eq!(pr.accepts[0].pay_to, TEST_VENDOR_WALLET);
                // $0.01 listed price = 10_000 atomic — NOT 10_500 (no 5% on top).
                assert_eq!(pr.accepts[0].amount, "10000");
                assert_eq!(pr.cost_breakdown.provider_cost, "0.010000");
                assert_eq!(pr.cost_breakdown.platform_fee, "0.000000");
                assert_eq!(pr.cost_breakdown.total, "0.010000");
                assert_eq!(pr.cost_breakdown.fee_percent, 0);
            }
            other => panic!("expected PaymentChallenge, got {other:?}"),
        }
    }

    /// (c) fallback: without a vendor_wallet the 402 advertises the global
    /// recipient and `price × 1.05` — byte-for-byte today's behavior.
    #[tokio::test]
    async fn proxy_402_advertises_global_recipient_without_vendor_wallet() {
        let state = make_state_with_vendor_service(None);
        let err = call_expect_err(state, "test-svc", HeaderMap::new()).await;
        match err {
            GatewayError::PaymentChallenge(pr) => {
                assert_eq!(pr.accepts[0].pay_to, TEST_GLOBAL_RECIPIENT);
                assert_eq!(pr.accepts[0].amount, "10500");
                assert_eq!(pr.cost_breakdown.provider_cost, "0.010000");
                assert_eq!(pr.cost_breakdown.platform_fee, "0.000500");
                assert_eq!(pr.cost_breakdown.total, "0.010500");
                assert_eq!(pr.cost_breakdown.fee_percent, PLATFORM_FEE_PERCENT);
            }
            other => panic!("expected PaymentChallenge, got {other:?}"),
        }
    }

    /// (d) wrong amount on the vendor path: anything below the listed price
    /// is insufficient (the vendor threshold is `price`, not `price × 1.05`).
    #[tokio::test]
    async fn proxy_vendor_service_rejects_amount_below_price() {
        let state = make_state_with_vendor_service(Some(TEST_VENDOR_WALLET));
        let mut p = valid_payload("test-svc");
        p.accepted.pay_to = TEST_VENDOR_WALLET.to_string();
        p.accepted.amount = "9999".to_string(); // expected is 10_000
        let err = call_expect_err(state, "test-svc", make_headers_with_payment(&p)).await;
        assert!(
            matches!(err, GatewayError::BadRequest(ref m)
                if m.contains("insufficient") && m.contains("10000")),
            "expected insufficient-amount rejection at the listed price, got {err:?}"
        );
    }

    /// (d) a payment addressed to the GLOBAL recipient must be rejected for a
    /// vendor_wallet service — hard equality against the per-service recipient.
    #[tokio::test]
    async fn proxy_vendor_service_rejects_pay_to_global_recipient() {
        let state = make_state_with_vendor_service(Some(TEST_VENDOR_WALLET));
        let mut p = valid_payload("test-svc");
        p.accepted.pay_to = TEST_GLOBAL_RECIPIENT.to_string();
        let err = call_expect_err(state, "test-svc", make_headers_with_payment(&p)).await;
        assert!(
            matches!(err, GatewayError::BadRequest(ref m)
                if m.contains("pay_to") && m.contains(TEST_VENDOR_WALLET)),
            "expected pay_to mismatch naming the vendor wallet, got {err:?}"
        );
    }

    /// (d) vice versa: a payment addressed to the VENDOR wallet must be
    /// rejected for a service without a vendor_wallet.
    #[tokio::test]
    async fn proxy_global_service_rejects_pay_to_vendor_wallet() {
        let state = make_state_with_vendor_service(None);
        let mut p = valid_payload("test-svc");
        p.accepted.pay_to = TEST_VENDOR_WALLET.to_string();
        let err = call_expect_err(state, "test-svc", make_headers_with_payment(&p)).await;
        assert!(
            matches!(err, GatewayError::BadRequest(ref m)
                if m.contains("pay_to") && m.contains(TEST_GLOBAL_RECIPIENT)),
            "expected pay_to mismatch naming the global recipient, got {err:?}"
        );
    }

    /// A payment addressed to the vendor wallet must clear the recipient
    /// equality gate and reach verification (which fails here only because
    /// the test facilitator has no verifiers).
    #[tokio::test]
    async fn proxy_vendor_service_pay_to_vendor_reaches_verification() {
        let state = make_state_with_vendor_service(Some(TEST_VENDOR_WALLET));
        let mut p = valid_payload("test-svc");
        p.accepted.pay_to = TEST_VENDOR_WALLET.to_string();
        let err = call_expect_err(state, "test-svc", make_headers_with_payment(&p)).await;
        assert!(
            matches!(err, GatewayError::InvalidPayment(ref m) if m.contains("verification failed")),
            "expected to reach (and fail at) verification, got {err:?}"
        );
    }

    /// Fee floor (round-2 item 9, unit half): a $0.000019 service prices to
    /// 19 atomic; `19 × 105 / 100 = 19` (integer floor), so the receivable is
    /// legitimately ZERO. Pins that sub-$0.00002 services round the 5% down
    /// to nothing rather than up to 1 atomic.
    #[test]
    fn test_compute_service_cost_fee_floor_sub_cent_price_has_zero_fee() {
        let cost = compute_service_cost(0.000019).unwrap();
        assert_eq!(cost.provider_atomic, 19);
        assert_eq!(cost.fee_atomic, 0, "floor(19 × 5 / 100) must be 0");
        assert_eq!(cost.total_atomic, 19);
    }

    /// Fee floor through the handler: the vendor 402 for a $0.000019 service
    /// advertises amount "19", platform_fee "0.000000", and fee_percent 0 —
    /// the agent-facing quote stays truthful at the rounding floor.
    #[tokio::test]
    async fn proxy_vendor_402_fee_floor_advertises_zero_fee() {
        let state = make_state_with_vendor_service_priced(Some(TEST_VENDOR_WALLET), 0.000019);
        let err = call_expect_err(state, "test-svc", HeaderMap::new()).await;
        match err {
            GatewayError::PaymentChallenge(pr) => {
                assert_eq!(pr.accepts[0].pay_to, TEST_VENDOR_WALLET);
                assert_eq!(pr.accepts[0].amount, "19");
                assert_eq!(pr.cost_breakdown.provider_cost, "0.000019");
                assert_eq!(pr.cost_breakdown.platform_fee, "0.000000");
                assert_eq!(pr.cost_breakdown.total, "0.000019");
                assert_eq!(pr.cost_breakdown.fee_percent, 0);
            }
            other => panic!("expected PaymentChallenge, got {other:?}"),
        }
    }

    /// Round-2 item 7: a vendor service priced so `provider_atomic` exceeds
    /// `i64::MAX` (registration only requires price > 0, and
    /// `validate_price_usdc` allows up to ~u64::MAX/1e6) must fail CLOSED
    /// with an Internal error BEFORE any 402 is quoted — never quote an
    /// amount the BIGINT receivable ledger cannot record.
    #[tokio::test]
    async fn proxy_vendor_service_unrecordable_price_fails_closed_before_402() {
        // 1.0e13 USDC → 1.0e19 atomic: within the u64 pricing cap
        // (~1.8446e13 USDC) but beyond i64::MAX (~9.22e18 atomic).
        let state = make_state_with_vendor_service_priced(Some(TEST_VENDOR_WALLET), 1.0e13);
        let err = call_expect_err(state, "test-svc", HeaderMap::new()).await;
        match err {
            GatewayError::Internal(msg) => {
                assert!(
                    msg.contains("recordable range"),
                    "error must name the recordable-range guard, got: {msg}"
                );
            }
            GatewayError::PaymentChallenge(pr) => panic!(
                "must fail closed BEFORE quoting a 402, but quoted amount {}",
                pr.accepts[0].amount
            ),
            other => panic!("expected Internal from the pre-quote guard, got {other:?}"),
        }
    }

    /// Same price on a NON-vendor service stays on the legacy gateway path
    /// (no receivable ledger involved) — pins that the new guard is scoped
    /// to the vendor path and does not change plain-service behavior.
    #[tokio::test]
    async fn proxy_plain_service_huge_price_still_quotes_402() {
        let state = make_state_with_vendor_service_priced(None, 1.0e13);
        let err = call_expect_err(state, "test-svc", HeaderMap::new()).await;
        assert!(
            matches!(err, GatewayError::PaymentChallenge(_)),
            "non-vendor path must keep today's behavior, got {err:?}"
        );
    }

    // ── Receipt records (settlement-platform P2) ──────────────────────────
    //
    // The plain-service receipt write fires post-upstream (step 7), which the
    // SSRF guard makes unreachable in tests (no public upstream); the record
    // CONTENT is pinned here at the unit level, and the vendor-path wiring is
    // pinned end-to-end in `tests/integration.rs`
    // (`vendor_paid_proxy_request_records_receipt_with_fee_receivable`).

    /// A vendor-path receipt must carry the vendor settlement + receivable,
    /// bill the agent exactly the listed price, and show a ZERO agent fee
    /// (vendor absorbs — rule #5 truthful breakdown).
    #[test]
    fn receipt_record_vendor_carries_receivable_and_zero_agent_fee() {
        let target = PaymentTarget::Vendor {
            price_atomic: 20_000,
            settlement: VendorSettlement {
                vendor_wallet: TEST_VENDOR_WALLET.to_string(),
                settled_atomic: 20_000,
                fee_receivable_atomic: 1_000,
            },
        };
        let record = target.receipt_record(
            "vendor-svc",
            "exact",
            Some("tx".to_string()),
            "payer".to_string(),
        );
        assert_eq!(record.model, "vendor-svc");
        assert_eq!(record.payment_scheme, "exact");
        assert_eq!(record.amount_paid_atomic, 20_000);
        assert_eq!(record.provider_cost_atomic, 20_000);
        assert_eq!(record.platform_fee_atomic, 0, "vendor absorbs the 5%");
        assert_eq!(record.total_atomic, 20_000);
        let vendor = record.vendor.expect("vendor receipt carries settlement");
        assert_eq!(vendor.vendor_wallet, TEST_VENDOR_WALLET);
        assert_eq!(vendor.settled_atomic, 20_000);
        assert_eq!(vendor.fee_receivable_atomic, 1_000);
    }

    /// A plain (gateway-recipient) receipt bills `price × 1.05` with the 5%
    /// fee on top and carries NO vendor fields.
    #[test]
    fn receipt_record_plain_service_has_fee_on_top_and_no_vendor_fields() {
        let target = PaymentTarget::Gateway {
            total_atomic: 10_500,
            fee_atomic: 500,
        };
        let record = target.receipt_record(
            "plain-svc",
            "exact",
            Some("tx".to_string()),
            "payer".to_string(),
        );
        assert_eq!(record.amount_paid_atomic, 10_500);
        assert_eq!(record.provider_cost_atomic, 10_000);
        assert_eq!(record.platform_fee_atomic, 500);
        assert_eq!(record.total_atomic, 10_500);
        assert!(
            record.vendor.is_none(),
            "plain services must not carry vendor settlement fields"
        );
    }

    /// A client-originated scheme string is capped before recording.
    #[test]
    fn receipt_record_caps_scheme_length() {
        let target = PaymentTarget::Gateway {
            total_atomic: 10_500,
            fee_atomic: 500,
        };
        let record = target.receipt_record(
            "svc",
            "a-very-long-unknown-scheme-string",
            None,
            "payer".to_string(),
        );
        assert_eq!(record.payment_scheme.chars().count(), 16);
    }

    #[tokio::test]
    async fn proxy_in_memory_replay_rejects_second_use_of_same_tx() {
        // Standard (non-durable-nonce) tx_raw goes through the in-memory LRU
        // when cache is None. First call fails verification but records the tx;
        // second call short-circuits at the replay-detected branch.
        let state = make_state_with_service(Some(0.01), false, true, None);
        let p = valid_payload("test-svc");

        let first = call_expect_err(
            Arc::clone(&state),
            "test-svc",
            make_headers_with_payment(&p),
        )
        .await;
        assert!(
            matches!(first, GatewayError::InvalidPayment(_)),
            "first call must reach verification (which fails), got {first:?}"
        );

        let second = call_expect_err(state, "test-svc", make_headers_with_payment(&p)).await;
        match second {
            GatewayError::InvalidPayment(msg) => {
                assert!(
                    msg.contains("already been used"),
                    "second call must hit replay-detected branch, got: {msg}"
                );
            }
            other => panic!("expected InvalidPayment from replay, got {other:?}"),
        }
    }
}
