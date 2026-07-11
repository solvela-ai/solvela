//! Solana price tool route: `POST /v1/solana/price`.
//!
//! An x402-paid, per-call-priced Solana token-price tool — Agent Toolbelt
//! pick #2 ("pay USDC on Solana to read Solana"). The gateway queries the
//! Jupiter Price API server-side; the agent pays USDC-SPL via x402, the
//! standard 5% platform fee on top.
//!
//! This route MIRRORS the web-search donor (`routes/search.rs`) end to end:
//! same dedicated-route rationale, same `compute_service_cost` money math,
//! same exact-scheme machinery, same channel-voucher draw fork, same
//! settle-then-serve posture. This is internal tool #2 — per the rule of
//! three, the shared `paid_internal_tool` helper is NOT extracted yet; when
//! tool #3 lands, factor the common route skeleton out of `search.rs` and
//! this file together.
//!
//! ## Divergences from the donor (both deliberate, both documented)
//!
//! 1. **Enablement**: the Jupiter lite host is KEYLESS, so there is no
//!    API-key env gate. The route is enabled whenever the `solana-price`
//!    entry exists (priced) in `config/services.toml`; a missing/unpriced
//!    entry is a clean 503. `state.price_provider` is always `Some` in
//!    production (`main.rs`) — the `None` arm is test/defensive only.
//! 2. **No response caching** — deliberately, and doubly so: prices are
//!    time-sensitive, and the response cache is wallet-agnostic (CLAUDE.md
//!    rule 16). The donor doesn't cache either; `state.cache` is used here
//!    ONLY for tx-replay protection and the channel draw lock, never to
//!    serve a cached price.
//!
//! ## Money path — reused, not reinvented
//!
//! - pricing comes from the `solana-price` service registry entry's
//!   `price_per_request_usdc` (single config source);
//! - the atomic-USDC breakdown is computed by the shared
//!   [`crate::routes::service_payment::compute_service_cost`] (fail-closed on
//!   NaN/Inf/negative/overflow — never serve for free);
//! - the agent pays `price × 1.05` to the gateway's global recipient wallet;
//! - verification + settlement go through the same facilitator, with the same
//!   replay protection and scheme/payload checks;
//! - spend is logged fire-and-forget at settlement time.
//!
//! No new pricing dimension. The 5% fee is applied EXACTLY ONCE, in
//! `compute_service_cost`. The upstream's market-data numbers are NEVER an
//! input to any billing computation.

use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use metrics::{counter, histogram};
use serde::Deserialize;
use serde_json::json;
use tracing::{info, warn};

use solvela_x402::types::{
    CostBreakdown, PaymentAccept, PaymentRequired, Resource, PLATFORM_FEE_PERCENT, SOLANA_NETWORK,
    X402_VERSION,
};

use crate::cache::ResponseCache;
use crate::error::GatewayError;
use crate::middleware::x402::decode_payment_header;
use crate::payment_util::extract_payer_wallet;
use crate::providers::price::PriceProvider;
use crate::receipts;
use crate::routes::channel::map_voucher_rejection;
use crate::routes::service_payment::{compute_service_cost, ServiceCost};
use crate::usage::SpendLogEntry;
use crate::AppState;

/// Service-registry id of the internal Solana-price tool. Pricing for the
/// route is read from this entry, so `config/services.toml` is the single
/// source of the tool's price (and `GET /v1/services` lists it).
const PRICE_SERVICE_ID: &str = "solana-price";

/// The canonical resource path used in the 402 challenge and enforced against
/// the inbound payment payload's `resource.url`.
const PRICE_RESOURCE_URL: &str = "/v1/solana/price";

/// Maximum mints per request — the Jupiter Price API v3's documented `ids`
/// cap ("You can query up to 50 ids at once", developers.jup.ag, 2026-07-08).
/// Enforced here BEFORE payment so an unservable batch is never charged.
const MAX_MINTS: usize = 50;

/// Maximum accepted request-body size (16 KiB). A maximal valid body is 50
/// base58 mints (≤44 chars each) — well under 4 KiB; a small cap rejects
/// abusive payloads cheaply before any allocation of the full body.
const PRICE_BODY_LIMIT: usize = 16 * 1024;

/// Request body for `POST /v1/solana/price`.
#[derive(Debug, Deserialize)]
pub struct PriceRequest {
    /// Base58 SPL mint addresses to price (1..=[`MAX_MINTS`]). Send each mint
    /// once; the response carries one entry per requested mint, in order.
    pub mints: Vec<String>,
}

/// Resolve the configured per-request price for the Solana-price tool from
/// the service registry, failing closed if the entry is missing or unpriced.
///
/// This IS the enablement gate (see the module docs): the upstream is
/// keyless, so — unlike the search donor's API-key gate — the registry entry
/// alone decides whether the route is live. An unconfigured/unpriced tool is
/// a 503 — never a free or stub-priced response.
async fn resolve_price_usdc(state: &AppState) -> Result<f64, GatewayError> {
    let registry = state.service_registry.read().await;
    let entry = registry.get(PRICE_SERVICE_ID).ok_or_else(|| {
        GatewayError::ServiceUnavailable(
            "solana price data is not configured on this gateway".to_string(),
        )
    })?;
    let price = entry.price_per_request_usdc.ok_or_else(|| {
        warn!(
            service_id = PRICE_SERVICE_ID,
            "solana-price service entry has no price_per_request_usdc configured"
        );
        GatewayError::ServiceUnavailable(
            "solana price data is not configured on this gateway".to_string(),
        )
    })?;
    // Defense in depth (mirrors the donor's F3): a non-positive or non-finite
    // configured price would otherwise quote `expected_atomic = 0` and serve
    // for free. `compute_service_cost` also fails closed on this, but refuse
    // here too so a misconfigured tool never even reaches the 402 quote.
    if !price.is_finite() || price <= 0.0 {
        warn!(
            service_id = PRICE_SERVICE_ID,
            price, "solana-price service entry has a non-positive price_per_request_usdc"
        );
        return Err(GatewayError::ServiceUnavailable(
            "solana price data is not configured on this gateway".to_string(),
        ));
    }
    Ok(price)
}

/// Validate one mint address: base58 that decodes to exactly 32 bytes.
///
/// Returns only a positional error (never reflects the client-controlled
/// string — same no-reflect posture as the donor's validation).
fn validate_mint(mint: &str, index: usize) -> Result<(), GatewayError> {
    // Cheap length bound first: a 32-byte value is 32–44 base58 chars.
    if mint.len() < 32 || mint.len() > 44 {
        return Err(GatewayError::BadRequest(format!(
            "'mints[{index}]' is not a valid base58 Solana mint address"
        )));
    }
    match bs58::decode(mint).into_vec() {
        Ok(bytes) if bytes.len() == 32 => Ok(()),
        _ => Err(GatewayError::BadRequest(format!(
            "'mints[{index}]' is not a valid base58 Solana mint address"
        ))),
    }
}

/// POST /v1/solana/price — x402-paid Solana token prices.
///
/// Flow (mirrors the web-search donor; see module docs):
/// 1. Require a wired price provider AND a priced registry entry — else 503
///    (never serve free). The registry entry is the enablement gate.
/// 2. Compute the atomic-USDC cost breakdown (flat price + 5% fee).
/// 3. Validate the request body BEFORE any payment work. The price is flat
///    (body-independent), so an empty/oversized/malformed `mints` list is a
///    400 with NO payment charged and NO settlement.
/// 4. No PAYMENT-SIGNATURE header → 402 with the cost breakdown.
/// 5. Header present → decode, validate every field, replay-protect, verify +
///    settle via the facilitator (hard enforcement — unconfirmed = reject).
/// 6. Fire-and-forget spend log at settlement time.
/// 7. Call the price adapter with the already-validated mints, return
///    normalized per-mint prices (unknown mint → explicit null entry).
pub async fn solana_price(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<Response, GatewayError> {
    // Step 1a: a price provider must be wired. Always `Some` in production
    // (keyless upstream — see module docs); `None` is defensive and returns
    // 503, never a free or stub-paid response.
    let provider = state.price_provider.clone().ok_or_else(|| {
        GatewayError::ServiceUnavailable(
            "solana price data is not configured on this gateway".to_string(),
        )
    })?;

    // Step 1b: resolve pricing from the registry (single config source AND
    // the enablement gate).
    let price_usdc = resolve_price_usdc(&state).await?;

    // Step 2: compute the cost breakdown once with integer arithmetic — used
    // for both the 402 quote and the inbound amount enforcement so they cannot
    // drift. Fails closed (500) on a corrupt price; never serves for free.
    let ServiceCost {
        provider_atomic,
        fee_atomic,
        total_atomic,
    } = compute_service_cost(price_usdc).map_err(|e| {
        warn!(error = %e, "invalid solana-price pricing");
        GatewayError::Internal("solana price data has invalid pricing configured".to_string())
    })?;
    // Internal gateway-hosted tool: the agent pays `price × 1.05` to the
    // gateway's global recipient (no vendor split).
    let expected_atomic = total_atomic;
    let pay_to_wallet = state.config.solana.recipient_wallet.clone();

    // Step 3: validate the request body BEFORE any payment verification or
    // settlement. The flat price does not depend on the body, so a request
    // that cannot run (empty / over-cap / non-base58 mints) must be rejected
    // with 400 and NEVER charged — no 402, no verify, no settle.
    let body_bytes = axum::body::to_bytes(body, PRICE_BODY_LIMIT)
        .await
        .map_err(|e| GatewayError::BadRequest(format!("failed to read request body: {e}")))?;
    let req: PriceRequest = serde_json::from_slice(&body_bytes).map_err(|_| {
        GatewayError::BadRequest(
            "request body must be JSON with a 'mints' array of base58 mint addresses".to_string(),
        )
    })?;

    if req.mints.is_empty() {
        return Err(GatewayError::BadRequest(
            "'mints' must not be empty".to_string(),
        ));
    }
    if req.mints.len() > MAX_MINTS {
        return Err(GatewayError::BadRequest(format!(
            "'mints' must contain at most {MAX_MINTS} entries (upstream batch cap)"
        )));
    }
    for (i, mint) in req.mints.iter().enumerate() {
        validate_mint(mint, i)?;
    }
    let mints = req.mints;

    // Step 4: payment header. Non-ASCII header value → 400, not a silent 402.
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

    // No header → 402 challenge. (Same `let-else` shape as the donor: the
    // `else` arm returns, keeping the no-`expect` invariant on the money path.)
    let Some(raw_header) = payment_header else {
        counter!("solvela_payments_total", "status" => "none").increment(1);
        info!("no payment signature on /v1/solana/price, returning 402");

        let provider_usdc = provider_atomic as f64 / 1_000_000.0;
        let fee_usdc = fee_atomic as f64 / 1_000_000.0;
        let total_usdc = expected_atomic as f64 / 1_000_000.0;

        let payment_required = PaymentRequired {
            x402_version: X402_VERSION,
            resource: Resource {
                url: PRICE_RESOURCE_URL.to_string(),
                method: "POST".to_string(),
            },
            accepts: vec![PaymentAccept {
                scheme: "exact".to_string(),
                network: SOLANA_NETWORK.to_string(),
                amount: expected_atomic.to_string(),
                // Quote the CONFIGURED mint (what the verifier enforces).
                asset: state.config.solana.usdc_mint.clone(),
                pay_to: pay_to_wallet.clone(),
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
            extensions: None,
        };
        return Err(GatewayError::PaymentChallenge(Box::new(payment_required)));
    };

    // Step 5: payment present — decode and verify.
    let payload = decode_payment_header(raw_header).map_err(|e| {
        warn!(error = %e, "PAYMENT-SIGNATURE header decode failed (price)");
        GatewayError::InvalidPayment(
            "PAYMENT-SIGNATURE header could not be decoded. \
             Encode a valid PaymentPayload as standard base64 JSON."
                .to_string(),
        )
    })?;

    // Validate resource URL matches this endpoint. `resource.url` is
    // client-controlled and unbounded — never reflect it.
    if payload.resource.url != PRICE_RESOURCE_URL {
        let got: String = payload.resource.url.chars().take(256).collect();
        warn!(got = %got, "payment resource URL mismatch (price)");
        return Err(GatewayError::InvalidPayment(
            "Payment resource does not match this endpoint.".to_string(),
        ));
    }

    // Validate resource method is POST — STATIC body, truncated log.
    if !payload.resource.method.eq_ignore_ascii_case("POST") {
        let method: String = payload.resource.method.chars().take(16).collect();
        warn!(method = %method, "payment resource method mismatch (price)");
        return Err(GatewayError::BadRequest(
            "Payment resource method must be POST.".to_string(),
        ));
    }

    // Validate network is Solana — STATIC body, truncated log.
    if !payload
        .accepted
        .network
        .eq_ignore_ascii_case(SOLANA_NETWORK)
    {
        let network: String = payload.accepted.network.chars().take(32).collect();
        warn!(network = %network, "payment network mismatch (price)");
        return Err(GatewayError::BadRequest(
            "Payment network is unsupported. Use the network advertised in the 402 response."
                .to_string(),
        ));
    }

    // Validate asset is the CONFIGURED USDC-SPL mint — STATIC body,
    // truncated log (cap to base58 pubkey len).
    if payload.accepted.asset != state.config.solana.usdc_mint {
        let asset: String = payload.accepted.asset.chars().take(44).collect();
        warn!(asset = %asset, "payment asset mismatch (price)");
        return Err(GatewayError::BadRequest(
            "Payment asset is not the accepted USDC mint. Use the asset advertised in the 402 \
             response."
                .to_string(),
        ));
    }

    // Validate pay_to matches the gateway's global recipient — STATIC body,
    // truncated log; never reflect the server-internal recipient either.
    if payload.accepted.pay_to != pay_to_wallet {
        let pay_to: String = payload.accepted.pay_to.chars().take(44).collect();
        warn!(pay_to = %pay_to, "payment pay_to mismatch (price)");
        return Err(GatewayError::BadRequest(
            "Payment recipient does not match this endpoint. Use the pay_to advertised in the 402 \
             response."
                .to_string(),
        ));
    }

    // --- v0 spend-down channel DRAW fork (mirrors the donor's hand-inserted
    // fork; see routes/search.rs for the full rationale). Forks iff the scheme
    // is "channel" AND the payload is a channel voucher; ANY channel-ish
    // mismatch is a fail-closed reject — no silent fallback to an exact
    // transfer. The fork bypasses `verify_and_settle` and the tx-replay cache
    // (both exact-only) and never fires an escrow claim.
    match (payload.accepted.scheme.as_str(), &payload.payload) {
        ("channel", solvela_x402::types::PayloadData::Channel(voucher_payload)) => {
            return channel_draw(
                &state,
                &headers,
                &body_bytes,
                provider.as_ref(),
                mints,
                ServiceCost {
                    provider_atomic,
                    fee_atomic,
                    total_atomic,
                },
                &payload.accepted.amount,
                voucher_payload,
            )
            .await;
        }
        ("channel", _) => {
            return Err(GatewayError::InvalidPayment(
                "scheme is 'channel' but the payload is not a channel voucher".to_string(),
            ));
        }
        (_, solvela_x402::types::PayloadData::Channel(_)) => {
            return Err(GatewayError::InvalidPayment(
                "payment payload is a channel voucher but the scheme is not 'channel'".to_string(),
            ));
        }
        // Not a channel request — fall through to the exact machinery below.
        _ => {}
    }

    // Validate payment amount covers cost + fee. Bad format → 400, never 0.
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

    // Issue #499: reject `require_tenant = TRUE` wallets on this path —
    // identical rationale and degradation to the donor (see routes/search.rs):
    // this flat-priced route does not run the per-tenant budget matrix, so a
    // wallet that REQUIRES it must be rejected BEFORE any scheme/variant
    // check, replay record, settlement, or upstream call.
    let payer_wallet = extract_payer_wallet(&payload);
    if state.usage.require_tenant_for_wallet(&payer_wallet).await {
        warn!("price request rejected: payer wallet requires per-tenant budgeting (#499)");
        return Err(GatewayError::Forbidden(
            "this wallet requires per-tenant budgeting; use POST /v1/chat/completions".to_string(),
        ));
    }

    // Validate scheme matches payload variant — never default-route an
    // unknown scheme. This internal tool offers only `exact`; an escrow
    // payload is a mismatch (no silent fallback to a direct transfer).
    match (payload.accepted.scheme.as_str(), &payload.payload) {
        ("exact", solvela_x402::types::PayloadData::Escrow(_)) => {
            return Err(GatewayError::BadRequest(
                "scheme is 'exact' but payload contains escrow data".to_string(),
            ));
        }
        ("exact", solvela_x402::types::PayloadData::Direct(_)) => { /* ok */ }
        (other, _) => {
            let scheme: String = other.chars().take(16).collect();
            return Err(GatewayError::BadRequest(format!(
                "unsupported payment scheme '{scheme}' for /v1/solana/price (only 'exact' is accepted)"
            )));
        }
    }

    // Replay protection. `/v1/solana/price` has its OWN in-memory bucket
    // (`ReplayPath::SolanaPrice`) so a burst on another route cannot evict
    // this route's replay entries (same isolation rationale as the donor).
    let tx_raw = match &payload.payload {
        solvela_x402::types::PayloadData::Direct(p) => &p.transaction,
        // UNREACHABLE: an escrow payload is rejected by the scheme/variant
        // check above. Kept for exhaustiveness; never executed on this path.
        solvela_x402::types::PayloadData::Escrow(p) => &p.deposit_tx,
        // UNREACHABLE on the exact path: a channel voucher forks (or is
        // rejected) above. Fail-closed defense-in-depth (never panic on a
        // payment path).
        solvela_x402::types::PayloadData::Channel(_) => {
            return Err(GatewayError::InvalidPayment(
                "channel voucher is not accepted on the exact path".to_string(),
            ));
        }
    };
    let is_durable_nonce = crate::routes::chat::uses_durable_nonce(tx_raw);

    let replay_detected = if let Some(cache) = &state.cache {
        cache
            .check_and_record_tx(tx_raw, is_durable_nonce)
            .await
            .is_err()
    } else {
        // GHSA-fq3f-c8p7-873f: durable-nonce txs have a 24h replay window the
        // in-memory LRU cannot cover — deny rather than accept degraded.
        if is_durable_nonce {
            warn!(
                tx_prefix = &tx_raw[..tx_raw.len().min(88)],
                "durable-nonce price payment rejected: Redis unavailable (GHSA-fq3f-c8p7-873f)"
            );
            return Err(GatewayError::InvalidPayment(
                "Payment service is temporarily degraded; please retry shortly.".to_string(),
            ));
        }
        let mut replay_set = state
            .replay_set
            .for_path(crate::ReplayPath::SolanaPrice)
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        let found = match replay_set.get(tx_raw) {
            Some(&inserted_at) if now.duration_since(inserted_at) < crate::AppState::REPLAY_TTL => {
                true
            }
            Some(_) => {
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
                "price payment accepted under degraded in-memory replay protection (no Redis)"
            );
            false
        }
    };

    if replay_detected {
        counter!("solvela_replay_rejections_total").increment(1);
        counter!("solvela_payments_total", "status" => "failed").increment(1);
        warn!(
            tx_prefix = &tx_raw[..tx_raw.len().min(88)],
            "replay attack detected on /v1/solana/price — transaction already used"
        );
        return Err(GatewayError::InvalidPayment(
            "transaction has already been used; each payment signature may only be submitted once"
                .to_string(),
        ));
    }

    // Verify + settle via the facilitator — hard enforcement. An unconfirmed
    // settlement must NOT be served as paid.
    match state.facilitator.verify_and_settle(&payload).await {
        Ok(settlement) if !settlement.success => {
            counter!("solvela_payments_total", "status" => "failed").increment(1);
            warn!(
                tx_signature = %settlement.tx_signature.as_deref().unwrap_or("unknown"),
                error = ?settlement.error,
                "price payment settlement failed: transaction not confirmed"
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
                "price payment verified and settled"
            );
        }
        Err(e) => {
            // GHSA-cgqx-mg48-949v: do not echo the verifier error to clients.
            counter!("solvela_payments_total", "status" => "failed").increment(1);
            warn!(error = %e, "price payment verification failed");
            return Err(GatewayError::InvalidPayment(
                "Payment verification failed. Check your transaction and retry.".to_string(),
            ));
        }
    }

    let tx_signature = match &payload.payload {
        solvela_x402::types::PayloadData::Direct(p) => Some(p.transaction.clone()),
        solvela_x402::types::PayloadData::Escrow(p) => Some(p.deposit_tx.clone()),
        // UNREACHABLE on the exact path (see the `tx_raw` arm above) —
        // fail-closed, never panic.
        solvela_x402::types::PayloadData::Channel(_) => {
            return Err(GatewayError::InvalidPayment(
                "channel voucher is not accepted on the exact path".to_string(),
            ));
        }
    };
    // `x-request-id` is client-controlled and unbounded; cap to 128 chars
    // (chars-based) before it reaches the `spend_logs.request_id` TEXT column.
    let request_id: Option<String> = headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.chars().take(128).collect());

    // Step 6: record spend + receipt at SETTLEMENT time — the money moved
    // on-chain, so neither row may be contingent on the upstream call. Both
    // are fire-and-forget; never awaited on the hot path (Rule #9). Mirrors
    // the donor: a paid request that writes a spend row also writes a receipt
    // row, at settlement, for both the success and upstream-failure arms
    // below (charge-without-delivery stays visible; #486).
    let receipt_record = receipts::ReceiptRecord {
        receipt_id: uuid::Uuid::new_v4(),
        model: PRICE_SERVICE_ID.to_string(),
        payment_scheme: payload.accepted.scheme.chars().take(16).collect(),
        tx_signature: tx_signature.clone(),
        payer_wallet: payer_wallet.clone(),
        amount_paid_atomic: expected_atomic,
        provider_cost_atomic: provider_atomic,
        platform_fee_atomic: fee_atomic,
        total_atomic,
        vendor: None,
    };

    let spend_entry = SpendLogEntry {
        wallet_address: payer_wallet,
        model: PRICE_SERVICE_ID.to_string(),
        provider: format!("price:{}", provider.name()),
        input_tokens: 0,
        output_tokens: 0,
        cost_usdc: expected_atomic as f64 / 1_000_000.0,
        tx_signature,
        request_id,
        session_id: None,
        tenant: None,
        tenant_enforced: false,
        estimated_cost_usdc: None,
        reserved: Default::default(),
        // Internal gateway-hosted tool pays the gateway recipient — no vendor
        // settlement leg.
        vendor: None,
        // The price tool never runs the smart router — no routing telemetry.
        routing_tier: None,
        routing_score: None,
    };
    state.usage.log_spend(spend_entry);
    let receipt_header_path = receipts::record_receipt(state.db_pool.as_ref(), receipt_record);

    // Step 7: payment settled — fetch prices with the mints validated in
    // Step 3. A failure here is a charge-without-delivery case (the agent
    // already paid on-chain); the spend + receipt rows above record the
    // charge. This matches the donor's #486 posture.
    match provider.prices(&mints).await {
        Ok(results) => {
            let mut response = (StatusCode::OK, Json(json!(results))).into_response();
            receipts::insert_receipt_header(&mut response, &receipt_header_path);
            Ok(response)
        }
        Err(e) => {
            // Do not leak upstream internals (GHSA-cgqx-mg48-949v posture).
            warn!(error = %e, provider = %provider.name(), "solana-price upstream failed");
            let mut response = (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": "price provider error" })),
            )
                .into_response();
            receipts::insert_receipt_header(&mut response, &receipt_header_path);
            Ok(response)
        }
    }
}

// ---------------------------------------------------------------------------
// v0 spend-down channel DRAW path — `/v1/solana/price`
//
// A line-for-line mirror of the donor's fork (`routes/search.rs`); see that
// file for the full design rationale (§ references). Tool #3 is the
// extraction point for the shared skeleton (rule of three).
// ---------------------------------------------------------------------------

/// Handle a `/v1/solana/price` request paid by a channel voucher. Gated on
/// `channel.enabled` + a DB pool + Redis (else 404, uniform with
/// `open`/`close`); sources `current_slot` RPC-free; acquires the per-channel
/// lock and RELEASES it on every exit path (the TTL is only the crash
/// backstop). All money-path invariants are enforced inside
/// [`channel_draw_locked`]; this wrapper owns only the gate + lock lifecycle.
// ponytail: 8 args threading request context into the lock scope — mirrors the
// donor; a params struct would be pure ceremony for one call site.
#[allow(clippy::too_many_arguments)]
async fn channel_draw(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    body_bytes: &[u8],
    provider: &dyn PriceProvider,
    mints: Vec<String>,
    cost: ServiceCost,
    accepted_amount: &str,
    voucher_payload: &solvela_x402::types::ChannelVoucherPayload,
) -> Result<Response, GatewayError> {
    // Gate: channels ship DISABLED; the draw additionally requires a DB pool
    // (the durable ledger) AND Redis (the per-channel lock). Any missing →
    // the same 404 as open/close — never a fake in-memory draw.
    if !state.config.channel.enabled {
        return Ok(crate::routes::channel::channel_not_available());
    }
    let Some(pool) = state.db_pool.as_ref() else {
        return Ok(crate::routes::channel::channel_not_available());
    };
    let Some(cache) = state.cache.as_ref() else {
        return Ok(crate::routes::channel::channel_not_available());
    };

    // SDK-contract check: `accepted.amount` MUST equal the per-call price.
    // Billing does NOT read this field — the gateway quote is authoritative —
    // but a mismatch signals an SDK↔gateway price disagreement; reject rather
    // than silently ignore it. Static message (never reflect the
    // client-controlled amount string).
    if accepted_amount != cost.total_atomic.to_string() {
        return Err(GatewayError::BadRequest(
            "channel voucher accepted.amount must equal the per-call price for this endpoint"
                .to_string(),
        ));
    }

    // Bind the voucher to THE served request: SHA-256 of the RAW body bytes.
    let request_digest = crate::routes::channel::request_digest(body_bytes);

    // RPC-free cached slot (5s TTL); `None` → fail closed with a 503, never
    // serve on a stale-unknown slot.
    let Some(current_slot) = crate::routes::escrow::fetch_cached_slot(state).await else {
        return Err(GatewayError::ServiceUnavailable(
            "could not reach the Solana cluster to verify the voucher; please retry shortly"
                .to_string(),
        ));
    };

    // Decode the wire voucher, fail-closed on any malformed field.
    let voucher = crate::routes::channel::voucher_from_payload(voucher_payload).map_err(|e| {
        warn!(error = %e, "channel draw (price): malformed voucher payload");
        GatewayError::BadRequest(e.to_string())
    })?;

    let channel_id = voucher_payload.channel_id.as_str();

    // Per-channel lock (Redis SET NX EX). Fail closed if Redis errors (no
    // in-memory fallback); reject if a concurrent draw holds it.
    let lock_token = match cache.acquire_channel_draw_lock(channel_id).await {
        Ok(Some(token)) => token,
        Ok(None) => {
            return Err(GatewayError::ServiceUnavailable(
                "a draw is already in progress on this channel; please retry shortly".to_string(),
            ));
        }
        Err(e) => {
            warn!(error = %e, "channel draw (price): lock acquisition failed (Redis)");
            return Err(GatewayError::ServiceUnavailable(
                "payment service is temporarily degraded; please retry shortly".to_string(),
            ));
        }
    };

    // Lock HELD. Run the draw and RELEASE on ALL paths — the TTL is only the
    // crash backstop, never the steady-state release.
    let outcome = channel_draw_locked(
        state,
        headers,
        provider,
        mints,
        cost,
        request_digest,
        current_slot,
        &voucher,
        channel_id,
        pool,
        cache,
        &lock_token,
    )
    .await;
    // Release synchronously (a spawned release would race the next sequential
    // draw), but bounded — a hung Redis must not stall the already-earned
    // response; the TTL is the crash backstop.
    if tokio::time::timeout(
        std::time::Duration::from_secs(5),
        cache.release_channel_draw_lock(channel_id, &lock_token),
    )
    .await
    .is_err()
    {
        warn!(
            channel_id,
            "channel draw (price): lock release timed out — lock will expire via TTL"
        );
    }
    outcome
}

/// The locked body of a channel draw: load state → #499 → verify voucher →
/// serve → POST-serve persist → charge-visible spend/receipt. The caller
/// ([`channel_draw`]) holds the per-channel lock across this whole function
/// and releases it unconditionally afterward.
// ponytail: 10 args threading the already-validated draw context into the
// locked scope — mirrors the donor; a struct would be ceremony for one call site.
#[allow(clippy::too_many_arguments)]
async fn channel_draw_locked(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    provider: &dyn PriceProvider,
    mints: Vec<String>,
    cost: ServiceCost,
    request_digest: [u8; 32],
    current_slot: u64,
    voucher: &solvela_x402::channel::Voucher,
    channel_id: &str,
    pool: &sqlx::PgPool,
    cache: &ResponseCache,
    lock_token: &str,
) -> Result<Response, GatewayError> {
    // Load the drawable open channel + its DB-sourced `agent_wallet`. A
    // closed/closing/unknown channel is not drawable → 404.
    let drawable =
        match crate::channels::load_open_channel_state(pool, channel_id, request_digest).await {
            Ok(Some(d)) => d,
            Ok(None) => {
                return Err(GatewayError::NotFound(
                    "channel not found or not open".to_string(),
                ));
            }
            Err(e) => {
                warn!(error = %e, "channel draw (price): failed to load channel state");
                return Err(GatewayError::Internal(
                    "could not load the channel".to_string(),
                ));
            }
        };
    let agent_wallet = drawable.agent_wallet;
    let state_view = drawable.state;

    // #499: reject a `require_tenant = TRUE` wallet — flat-priced-tool budget
    // posture (NO `check_budget`). The wallet is sourced from the DB
    // `agent_wallet`, NEVER the voucher.
    if state.usage.require_tenant_for_wallet(&agent_wallet).await {
        warn!("channel draw (price) rejected: agent wallet requires per-tenant budgeting (#499)");
        return Err(GatewayError::Forbidden(
            "this wallet requires per-tenant budgeting; use POST /v1/chat/completions".to_string(),
        ));
    }

    // billed = the flat quote (= actual = realized; no gap, no discount).
    // `verify_voucher` enforces `voucher.cumulative - last == billed`.
    let billed = cost.total_atomic;

    // Verify the voucher — EXACT, fail-closed on every rule. Authenticated
    // rejections surface the authoritative `last_cumulative` for SDK resync;
    // pre-auth rejections surface nothing.
    if let Err(e) =
        solvela_x402::channel::verify_voucher(&state_view, voucher, billed, current_slot)
    {
        return Err(map_voucher_rejection(e, state_view.last_cumulative_atomic));
    }

    // SERVE. The channel fork NEVER touches `verify_and_settle`, the
    // tx-replay cache, or `fire_escrow_claim`.
    let results = match provider.prices(&mints).await {
        Ok(r) => r,
        Err(e) => {
            // Provider failure: NO persist → `last` does NOT advance → the
            // agent keeps its draw and was NOT debited. LEDGER INVARIANT: a
            // positive spend/receipt exists IFF `last` advanced IFF the agent
            // was debited, so a non-charge writes NOTHING. (Do not leak
            // upstream internals — GHSA-cgqx-mg48-949v.)
            warn!(
                error = %e,
                provider = %provider.name(),
                channel_id,
                "solana-price upstream failed (channel draw) — no charge; last unchanged"
            );
            return Ok((
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": "price provider error" })),
            )
                .into_response());
        }
    };

    // Re-confirm we STILL hold the draw lock before persisting (donor FIX 3):
    // on a confirmed loss, abort the persist and deliver the earned results
    // recording NOTHING. A Redis error here is inconclusive → proceed (the DB
    // CAS remains the money backstop).
    match cache.channel_draw_lock_held(channel_id, lock_token).await {
        Ok(true) => {}
        Ok(false) => {
            counter!(
                "solvela_channel_draw_persist_failed_total",
                "reason" => "lock_lost"
            )
            .increment(1);
            warn!(
                channel_id,
                cumulative_atomic = voucher.cumulative_atomic,
                "channel draw (price): draw lock no longer ours before persist (TTL expired \
                 mid-serve) — aborting persist, bounded one-call non-charge (deposit intact)"
            );
            return Ok((StatusCode::OK, Json(json!(results))).into_response());
        }
        Err(e) => {
            warn!(
                error = %e,
                channel_id,
                "channel draw (price): could not re-check draw-lock ownership before persist; \
                 proceeding (DB CAS is the money backstop)"
            );
        }
    }

    // POST-serve persist (deliver-then-record, #486-safe): advancing `last`
    // IS the debit, so the spend/receipt below are contingent on it committing.
    if let Err(e) = crate::channels::persist_voucher_and_advance(
        pool,
        &crate::channels::VoucherRecord {
            channel_id,
            cumulative_atomic: voucher.cumulative_atomic,
            call_cost_atomic: billed,
            // Flat-priced: quote == actual, so the realized advance IS the
            // billed delta (the chat draw passes min(actual, billed)).
            realized_advance_atomic: billed,
            expiry_slot: voucher.expiry_slot,
            nonce: voucher.nonce,
            request_digest: &voucher.request_digest,
            signature: &voucher.signature,
        },
    )
    .await
    {
        // A persist failure AFTER a successful serve is a BOUNDED one-call
        // gateway loss — the agent still receives its results, but `last` did
        // NOT advance (deposit intact), so NO debit occurred and NO
        // spend/receipt is written. The shared classifier emits the
        // notice/page-tier counters (one code site with the other draws).
        let reason = crate::channels::emit_persist_failure_counters(&e);
        warn!(
            error = %e,
            channel_id,
            reason,
            cumulative_atomic = voucher.cumulative_atomic,
            last_cumulative = state_view.last_cumulative_atomic,
            "channel draw (price): persist failed after a successful serve — bounded \
             one-call gateway loss, NO charge recorded (deposit intact; agent \
             resyncs on next draw)"
        );
        return Ok((StatusCode::OK, Json(json!(results))).into_response());
    }

    // `last` advanced ⇒ the agent WAS debited ⇒ record the positive spend +
    // receipt. This is the ONLY site a price channel spend/receipt row is
    // written, so "positive receipt ⟺ last advanced ⟺ agent debited" holds by
    // construction. wallet = DB `agent_wallet`, `tx_signature = None`,
    // scheme = "channel", no vendor leg.
    let ServiceCost {
        provider_atomic,
        fee_atomic,
        total_atomic,
    } = cost;
    // `x-request-id` is client-controlled/unbounded; cap to 128 chars
    // (chars-based) before it reaches the `spend_logs.request_id` TEXT column.
    let request_id: Option<String> = headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.chars().take(128).collect());
    let receipt_record = receipts::ReceiptRecord {
        receipt_id: uuid::Uuid::new_v4(),
        model: PRICE_SERVICE_ID.to_string(),
        payment_scheme: "channel".to_string(),
        tx_signature: None,
        payer_wallet: agent_wallet.clone(),
        amount_paid_atomic: total_atomic,
        provider_cost_atomic: provider_atomic,
        platform_fee_atomic: fee_atomic,
        total_atomic,
        vendor: None,
    };
    let spend_entry = SpendLogEntry {
        wallet_address: agent_wallet,
        model: PRICE_SERVICE_ID.to_string(),
        provider: format!("price:{}", provider.name()),
        input_tokens: 0,
        output_tokens: 0,
        cost_usdc: total_atomic as f64 / 1_000_000.0,
        // A voucher draw has no on-chain settlement signature.
        tx_signature: None,
        request_id,
        session_id: None,
        tenant: None,
        tenant_enforced: false,
        estimated_cost_usdc: None,
        reserved: Default::default(),
        vendor: None,
        // The price tool never runs the smart router — no routing telemetry.
        routing_tier: None,
        routing_score: None,
    };
    state.usage.log_spend(spend_entry);
    let receipt_header_path = receipts::record_receipt(state.db_pool.as_ref(), receipt_record);

    let mut response = (StatusCode::OK, Json(json!(results))).into_response();
    receipts::insert_receipt_header(&mut response, &receipt_header_path);
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn price_request_parses_mints() {
        let req: PriceRequest =
            serde_json::from_str(r#"{"mints":["EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"]}"#)
                .unwrap();
        assert_eq!(req.mints.len(), 1);
        assert_eq!(req.mints[0], "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");
    }

    #[test]
    fn price_request_rejects_missing_mints() {
        let err = serde_json::from_str::<PriceRequest>(r#"{"tokens":["x"]}"#);
        assert!(err.is_err(), "missing mints must fail to deserialize");
    }

    #[test]
    fn validate_mint_accepts_real_mints() {
        assert!(validate_mint("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", 0).is_ok());
        assert!(validate_mint("So11111111111111111111111111111111111111112", 0).is_ok());
    }

    #[test]
    fn validate_mint_rejects_bad_inputs() {
        // Not base58.
        assert!(validate_mint("not-a-mint!!", 0).is_err());
        // Valid base58 but wrong decoded length.
        assert!(validate_mint("abc", 0).is_err());
        // Empty.
        assert!(validate_mint("", 0).is_err());
        // 44 chars of base58 that decode to more than 32 bytes are rejected
        // by the decoded-length check even when the string length passes.
        assert!(validate_mint("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz", 0).is_err());
    }

    #[test]
    fn validate_mint_error_never_reflects_the_input() {
        let err = validate_mint("EVIL$injection<script>attempt-string!!", 3).unwrap_err();
        let msg = format!("{err}");
        assert!(
            !msg.contains("EVIL"),
            "the client-controlled mint string must never be reflected: {msg}"
        );
        assert!(msg.contains("mints[3]"), "positional index expected: {msg}");
    }
}
