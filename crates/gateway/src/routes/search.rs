//! Web-search tool route: `POST /v1/search`.
//!
//! An x402-paid, per-call-priced web-search tool — the first internal tool that
//! lights up the dormant service marketplace (Agent Toolbelt PR #1). The gateway
//! holds the upstream search-API key and calls it server-side; the agent pays
//! USDC-SPL via x402, the standard 5% platform fee on top.
//!
//! ## Why a dedicated route (not `/v1/services/{id}/proxy`)
//!
//! The proxy handler forwards the request body to an EXTERNAL, third-party x402
//! endpoint and explicitly rejects `internal` services. Web search is an
//! INTERNAL, gateway-hosted tool: the gateway authenticates to Tavily with its
//! own key and normalizes the response. So this is its own route, mirroring
//! `/v1/images/generations`.
//!
//! ## Money path — reused, not reinvented
//!
//! The flat-price + 5%-fee + settlement path is the SAME one the proxy uses:
//! - pricing comes from the `web-search` service registry entry's
//!   `price_per_request_usdc` (single config source);
//! - the atomic-USDC breakdown is computed by the shared
//!   [`crate::routes::service_payment::compute_service_cost`] (fail-closed on
//!   NaN/Inf/negative/overflow — never serve for free);
//! - the agent pays `price × 1.05` to the gateway's global recipient wallet
//!   (gateway settlement target; no vendor split for an internal tool);
//! - verification + settlement go through the same facilitator, with the same
//!   replay protection and scheme/payload checks;
//! - spend is logged fire-and-forget at settlement time.
//!
//! No new pricing dimension. The 5% fee is applied EXACTLY ONCE, in
//! `compute_service_cost`.

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

use crate::error::GatewayError;
use crate::middleware::x402::decode_payment_header;
use crate::payment_util::extract_payer_wallet;
use crate::providers::search::{SearchProvider, SearchQuery};
use crate::receipts;
use crate::routes::service_payment::{compute_service_cost, ServiceCost};
use crate::usage::SpendLogEntry;
use crate::AppState;

/// Service-registry id of the internal web-search tool. Pricing for the route
/// is read from this entry, so `config/services.toml` is the single source of
/// the tool's price (and `GET /v1/services` lists it).
const SEARCH_SERVICE_ID: &str = "web-search";

/// The canonical resource path used in the 402 challenge and enforced against
/// the inbound payment payload's `resource.url`.
const SEARCH_RESOURCE_URL: &str = "/v1/search";

/// Maximum query length accepted (defense against abusive payloads). Tavily's
/// own cap is higher; this is a cheap upfront bound.
const MAX_QUERY_LEN: usize = 2_000;

/// Upper bound on `max_results` echoed to the upstream adapter.
const MAX_RESULTS_CAP: u8 = 20;

/// Maximum accepted request-body size (64 KiB). The search body is a tiny JSON
/// object (`query` + optional `max_results`); a small cap rejects abusive
/// payloads cheaply before any allocation of the full body.
const SEARCH_BODY_LIMIT: usize = 64 * 1024;

/// Request body for `POST /v1/search`.
#[derive(Debug, Deserialize)]
pub struct SearchRequest {
    /// The search query string.
    pub query: String,
    /// Optional maximum number of results. Clamped to [`MAX_RESULTS_CAP`].
    #[serde(default)]
    pub max_results: Option<u8>,
}

/// Resolve the configured per-request price for the web-search tool from the
/// service registry, failing closed if the entry is missing or unpriced.
///
/// Pricing lives in `config/services.toml` (the `web-search` entry) so the
/// money path has a single source of truth and the tool is discoverable via
/// `GET /v1/services`. An unconfigured/unpriced tool is a 503 — never a free
/// or stub-priced response.
async fn resolve_price_usdc(state: &AppState) -> Result<f64, GatewayError> {
    let registry = state.service_registry.read().await;
    let entry = registry.get(SEARCH_SERVICE_ID).ok_or_else(|| {
        GatewayError::ServiceUnavailable("web search is not configured on this gateway".to_string())
    })?;
    let price = entry.price_per_request_usdc.ok_or_else(|| {
        warn!(
            service_id = SEARCH_SERVICE_ID,
            "web-search service entry has no price_per_request_usdc configured"
        );
        GatewayError::ServiceUnavailable("web search is not configured on this gateway".to_string())
    })?;
    // Defense in depth (F3): a non-positive (or non-finite) configured price is
    // a misconfiguration that would otherwise quote `expected_atomic = 0` and
    // serve for free. `compute_service_cost` also fails closed on this, but
    // refuse here too so an unpriced/zero-priced tool never even reaches the 402
    // quote — a 503 "not configured", never a free response. The explicit
    // `is_finite` check also rejects NaN (which `<= 0.0` alone would miss).
    if !price.is_finite() || price <= 0.0 {
        warn!(
            service_id = SEARCH_SERVICE_ID,
            price, "web-search service entry has a non-positive price_per_request_usdc"
        );
        return Err(GatewayError::ServiceUnavailable(
            "web search is not configured on this gateway".to_string(),
        ));
    }
    Ok(price)
}

/// POST /v1/search — x402-paid web search.
///
/// Flow (mirrors the proxy money path; see module docs):
/// 1. Require a configured search provider (`TAVILY_API_KEY`) AND a priced
///    registry entry — else 503 (never serve free).
/// 2. Compute the atomic-USDC cost breakdown (flat price + 5% fee).
/// 3. Validate the request body BEFORE any payment work. The price is flat
///    (body-independent), so a missing/empty/malformed `query` is a 400 with NO
///    payment charged and NO settlement — a request that was never going to run
///    must not be billed.
/// 4. No PAYMENT-SIGNATURE header → 402 with the cost breakdown.
/// 5. Header present → decode, validate every field, replay-protect, verify +
///    settle via the facilitator (hard enforcement — unconfirmed = reject).
/// 6. Fire-and-forget spend log at settlement time.
/// 7. Call the search adapter with the already-validated query, return
///    normalized results.
pub async fn search(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: axum::body::Body,
) -> Result<Response, GatewayError> {
    // Step 1a: a search provider must be configured. With no upstream key the
    // tool cannot serve — return 503, never a free or stub-paid response.
    let provider = state.search_provider.clone().ok_or_else(|| {
        GatewayError::ServiceUnavailable("web search is not configured on this gateway".to_string())
    })?;

    // Step 1b: resolve pricing from the registry (single config source).
    let price_usdc = resolve_price_usdc(&state).await?;

    // Step 2: compute the cost breakdown once with integer arithmetic — used
    // for both the 402 quote and the inbound amount enforcement so they cannot
    // drift. Fails closed (500) on a corrupt price; never serves for free.
    let ServiceCost {
        provider_atomic,
        fee_atomic,
        total_atomic,
    } = compute_service_cost(price_usdc).map_err(|e| {
        warn!(error = %e, "invalid web-search pricing");
        GatewayError::Internal("web search has invalid pricing configured".to_string())
    })?;
    // Internal gateway-hosted tool: the agent pays `price × 1.05` to the
    // gateway's global recipient (no vendor split). `total_atomic` is the
    // amount the agent must pay; `fee_atomic` the 5% on top.
    let expected_atomic = total_atomic;
    let pay_to_wallet = state.config.solana.recipient_wallet.clone();

    // Step 3: validate the request body BEFORE any payment verification or
    // settlement. The flat price does not depend on the body, so a request that
    // cannot run (missing/empty/over-long `query`) must be rejected with 400
    // and NEVER charged — no 402, no verify, no settle. This is purely an
    // ordering guarantee; the price and settlement semantics are unchanged.
    let body_bytes = axum::body::to_bytes(body, SEARCH_BODY_LIMIT)
        .await
        .map_err(|e| GatewayError::BadRequest(format!("failed to read request body: {e}")))?;
    let req: SearchRequest = serde_json::from_slice(&body_bytes).map_err(|_| {
        GatewayError::BadRequest("request body must be JSON with a 'query' field".to_string())
    })?;

    let query = req.query.trim();
    if query.is_empty() {
        return Err(GatewayError::BadRequest(
            "'query' must not be empty".to_string(),
        ));
    }
    if query.len() > MAX_QUERY_LEN {
        return Err(GatewayError::BadRequest(format!(
            "'query' must be at most {MAX_QUERY_LEN} characters"
        )));
    }
    let search_query = SearchQuery {
        query: query.to_string(),
        max_results: req.max_results.map(|n| n.clamp(1, MAX_RESULTS_CAP)),
    };

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

    // No header → 402 challenge. Binding `raw_header` here (rather than an
    // `Option` + `expect` later) keeps the no-`expect` invariant on the money
    // path: the `else` arm returns, so `raw_header` is unconditionally `&str`
    // afterward.
    let Some(raw_header) = payment_header else {
        counter!("solvela_payments_total", "status" => "none").increment(1);
        info!("no payment signature on /v1/search, returning 402");

        let provider_usdc = provider_atomic as f64 / 1_000_000.0;
        let fee_usdc = fee_atomic as f64 / 1_000_000.0;
        let total_usdc = expected_atomic as f64 / 1_000_000.0;

        let payment_required = PaymentRequired {
            x402_version: X402_VERSION,
            resource: Resource {
                url: SEARCH_RESOURCE_URL.to_string(),
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
        warn!(error = %e, "PAYMENT-SIGNATURE header decode failed (search)");
        GatewayError::InvalidPayment(
            "PAYMENT-SIGNATURE header could not be decoded. \
             Encode a valid PaymentPayload as standard base64 JSON."
                .to_string(),
        )
    })?;

    // Validate resource URL matches this endpoint.
    if payload.resource.url != SEARCH_RESOURCE_URL {
        // resource.url is client-controlled and unbounded — never reflect it.
        let got: String = payload.resource.url.chars().take(256).collect();
        warn!(got = %got, "payment resource URL mismatch (search)");
        return Err(GatewayError::InvalidPayment(
            "Payment resource does not match this endpoint.".to_string(),
        ));
    }

    // Validate resource method is POST. `resource.method` is client-controlled
    // and unbounded — return a STATIC body and log only a truncated copy
    // (mirrors the chat route's reflected-injection posture).
    if !payload.resource.method.eq_ignore_ascii_case("POST") {
        let method: String = payload.resource.method.chars().take(16).collect();
        warn!(method = %method, "payment resource method mismatch (search)");
        return Err(GatewayError::BadRequest(
            "Payment resource method must be POST.".to_string(),
        ));
    }

    // Validate network is Solana. `accepted.network` is client-controlled —
    // STATIC body, truncated log.
    if !payload
        .accepted
        .network
        .eq_ignore_ascii_case(SOLANA_NETWORK)
    {
        let network: String = payload.accepted.network.chars().take(32).collect();
        warn!(network = %network, "payment network mismatch (search)");
        return Err(GatewayError::BadRequest(
            "Payment network is unsupported. Use the network advertised in the 402 response."
                .to_string(),
        ));
    }

    // Validate asset is the CONFIGURED USDC-SPL mint. `accepted.asset` is
    // client-controlled — STATIC body, truncated log (cap to base58 pubkey len).
    if payload.accepted.asset != state.config.solana.usdc_mint {
        let asset: String = payload.accepted.asset.chars().take(44).collect();
        warn!(asset = %asset, "payment asset mismatch (search)");
        return Err(GatewayError::BadRequest(
            "Payment asset is not the accepted USDC mint. Use the asset advertised in the 402 \
             response."
                .to_string(),
        ));
    }

    // Validate pay_to matches the gateway's global recipient. `accepted.pay_to`
    // is client-controlled — STATIC body, truncated log. Never reflect the
    // server-internal recipient wallet to the client either.
    if payload.accepted.pay_to != pay_to_wallet {
        let pay_to: String = payload.accepted.pay_to.chars().take(44).collect();
        warn!(pay_to = %pay_to, "payment pay_to mismatch (search)");
        return Err(GatewayError::BadRequest(
            "Payment recipient does not match this endpoint. Use the pay_to advertised in the 402 \
             response."
                .to_string(),
        ));
    }

    // --- v0 spend-down channel DRAW fork (Pass B) ------------------------
    //
    // `search.rs` is string-keyed on the scheme, so `PayloadData::Channel`
    // produces ZERO compile errors here — this fork is inserted BY HAND after
    // the resource/network/asset/pay_to validation above (which applies to a
    // channel voucher too) and BEFORE the exact-only machinery below
    // (client_amount parse, #499 reject, scheme catch-all, tx-replay,
    // verify_and_settle). It forks iff the scheme is "channel" AND the payload
    // is a channel voucher; ANY channel-ish mismatch is a fail-closed reject —
    // no silent fallback to an exact transfer. The fork bypasses
    // `verify_and_settle` and the tx-replay cache (both exact-only; a voucher
    // has no on-chain tx to replay/settle) and never fires an escrow claim.
    // See channel scope §4.4 / HALT 3.
    match (payload.accepted.scheme.as_str(), &payload.payload) {
        ("channel", solvela_x402::types::PayloadData::Channel(voucher_payload)) => {
            return channel_draw(
                &state,
                &headers,
                &body_bytes,
                provider.as_ref(),
                search_query,
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

    // Issue #499: reject `require_tenant = TRUE` wallets on this path.
    //
    // Like the service-marketplace proxy, `/v1/search` does NOT run
    // `check_budget`'s per-tenant budget matrix — its price is flat per request,
    // not a per-wallet budget. A wallet provisioned `require_tenant = TRUE`
    // ("may only spend under a tagged, provisioned tenant") would otherwise
    // bypass that fail-closed guarantee here. Reject BEFORE any scheme/variant
    // check, replay record, settlement, or upstream call, so a rejected request
    // takes no spend. The wallet must use `POST /v1/chat/completions`, which
    // enforces the tenant matrix.
    //
    // Degradation matches the chat/proxy paths exactly:
    // `require_tenant_for_wallet` returns `false` (do not reject) when Redis is
    // absent or on a transient Redis/DB read error (the wallet caps are the
    // backstop) — see its doc.
    let payer_wallet = extract_payer_wallet(&payload);
    if state.usage.require_tenant_for_wallet(&payer_wallet).await {
        warn!("search request rejected: payer wallet requires per-tenant budgeting (#499)");
        return Err(GatewayError::Forbidden(
            "this wallet requires per-tenant budgeting; use POST /v1/chat/completions".to_string(),
        ));
    }

    // Validate scheme matches payload variant — never default-route an unknown
    // scheme. This internal tool offers only `exact`; an escrow payload is a
    // mismatch (no silent fallback to a direct transfer).
    match (payload.accepted.scheme.as_str(), &payload.payload) {
        ("exact", solvela_x402::types::PayloadData::Escrow(_)) => {
            return Err(GatewayError::BadRequest(
                "scheme is 'exact' but payload contains escrow data".to_string(),
            ));
        }
        ("exact", solvela_x402::types::PayloadData::Direct(_)) => { /* ok */ }
        (other, _) => {
            // `/v1/search` only accepts the `exact` scheme (it is the only
            // scheme advertised in the 402). Reject anything else explicitly
            // rather than silently treating it as exact.
            let scheme: String = other.chars().take(16).collect();
            return Err(GatewayError::BadRequest(format!(
                "unsupported payment scheme '{scheme}' for /v1/search (only 'exact' is accepted)"
            )));
        }
    }

    // Replay protection. `/v1/search` has its OWN in-memory bucket
    // (`ReplayPath::Search`): under Redis-down + proxy load, sharing the proxy
    // LRU would let proxy entries evict search entries and shorten search's
    // replay window. Per-path buckets keep each route's replay protection
    // independent (chat/proxy/a2a buckets already isolated this way).
    let tx_raw = match &payload.payload {
        solvela_x402::types::PayloadData::Direct(p) => &p.transaction,
        // UNREACHABLE: an escrow payload is rejected by the scheme/variant check
        // above (only `exact` is advertised/accepted on /v1/search). Kept for
        // exhaustiveness; never executed on this path.
        solvela_x402::types::PayloadData::Escrow(p) => &p.deposit_tx,
        // UNREACHABLE on the exact path: a channel voucher forks (or is
        // rejected) above and never reaches here. Fail-closed defense-in-depth
        // (never `unreachable!`/panic on a payment path).
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
                "durable-nonce search payment rejected: Redis unavailable (GHSA-fq3f-c8p7-873f)"
            );
            return Err(GatewayError::InvalidPayment(
                "Payment service is temporarily degraded; please retry shortly.".to_string(),
            ));
        }
        let mut replay_set = state
            .replay_set
            .for_path(crate::ReplayPath::Search)
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
                "search payment accepted under degraded in-memory replay protection (no Redis)"
            );
            false
        }
    };

    if replay_detected {
        counter!("solvela_replay_rejections_total").increment(1);
        counter!("solvela_payments_total", "status" => "failed").increment(1);
        warn!(
            tx_prefix = &tx_raw[..tx_raw.len().min(88)],
            "replay attack detected on /v1/search — transaction already used"
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
                "search payment settlement failed: transaction not confirmed"
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
                "search payment verified and settled"
            );
        }
        Err(e) => {
            // GHSA-cgqx-mg48-949v: do not echo the verifier error to clients.
            counter!("solvela_payments_total", "status" => "failed").increment(1);
            warn!(error = %e, "search payment verification failed");
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
    // before it reaches the `spend_logs.request_id` TEXT column (chars-based so
    // a multibyte boundary can never split a codepoint).
    let request_id: Option<String> = headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.chars().take(128).collect());

    // Step 6: record spend + receipt at SETTLEMENT time — the money moved
    // on-chain, so neither row may be contingent on the upstream search call.
    // Both are fire-and-forget (`log_spend` and `record_receipt` spawn their own
    // writes); never awaited on the hot path (Architectural Rule #9). Mirrors
    // the proxy path: a paid request that writes a spend row also writes a
    // receipt row, at settlement, for both the success and upstream-failure
    // arms below (charge-without-delivery stays visible; #486).
    //
    // This is an internal gateway-hosted tool (no `vendor_wallet`), so the
    // receipt is the gateway-path shape: the agent-facing breakdown is the
    // canonical `compute_service_cost` triple (provider + 5% fee = total) and
    // there is no vendor settlement leg. The scheme is client-originated but
    // already passed the scheme/variant check and settled — cap it before
    // recording (matches the proxy's `receipt_record`).
    let receipt_record = receipts::ReceiptRecord {
        receipt_id: uuid::Uuid::new_v4(),
        model: SEARCH_SERVICE_ID.to_string(),
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
        model: SEARCH_SERVICE_ID.to_string(),
        provider: format!("search:{}", provider.name()),
        input_tokens: 0,
        output_tokens: 0,
        cost_usdc: expected_atomic as f64 / 1_000_000.0,
        tx_signature,
        request_id,
        session_id: None,
        tenant: None,
        tenant_enforced: false,
        estimated_cost_usdc: None,
        // Internal gateway-hosted tool pays the gateway recipient — no vendor
        // settlement leg (that path is for external `vendor_wallet` services).
        vendor: None,
    };
    state.usage.log_spend(spend_entry);
    // `receipt_header_path` is `Some` only when a retrievable receipt exists (DB
    // configured AND the row write was dispatched); attached to every response
    // built after this point — including the 502 upstream-failure arm — so a
    // failed-upstream response still carries a fetchable receipt. With no
    // DATABASE_URL it stays `None` and no header is ever emitted (never promise
    // an unfetchable receipt).
    let receipt_header_path = receipts::record_receipt(state.db_pool.as_ref(), receipt_record);

    // Step 7: payment settled — run the search with the body validated in
    // Step 3. Note: a search failure here is a charge-without-delivery case (the
    // agent already paid on-chain); the spend + receipt rows above record the
    // charge. This matches the proxy path's #486 posture. The body was validated
    // BEFORE any charge, so the only way to reach here is with a runnable query.
    match provider.search(search_query).await {
        Ok(results) => {
            let mut response = (StatusCode::OK, Json(json!(results))).into_response();
            receipts::insert_receipt_header(&mut response, &receipt_header_path);
            Ok(response)
        }
        Err(e) => {
            // Do not leak upstream internals (GHSA-cgqx-mg48-949v posture).
            warn!(error = %e, provider = %provider.name(), "web-search upstream failed");
            let mut response = (
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": "search provider error" })),
            )
                .into_response();
            receipts::insert_receipt_header(&mut response, &receipt_header_path);
            Ok(response)
        }
    }
}

// ---------------------------------------------------------------------------
// v0 spend-down channel DRAW path (Pass B) — `/v1/search` only
// ---------------------------------------------------------------------------

/// Handle a `/v1/search` request paid by a channel voucher (the hand-inserted
/// fork above). Gated on `channel.enabled` + a DB pool + Redis (else 404,
/// uniform with `open`/`close`); sources `current_slot` RPC-free; acquires the
/// per-channel lock and RELEASES it on every exit path (the TTL is only the
/// crash backstop). All money-path invariants are enforced inside
/// [`channel_draw_locked`]; this wrapper owns only the gate + lock lifecycle.
// ponytail: 8 args because this hot-path handler threads request context
// (state/headers/body/provider/query/cost/amount/voucher) into the lock scope;
// a params struct would be pure ceremony for one call site.
#[allow(clippy::too_many_arguments)]
async fn channel_draw(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    body_bytes: &[u8],
    provider: &dyn SearchProvider,
    search_query: SearchQuery,
    cost: ServiceCost,
    accepted_amount: &str,
    voucher_payload: &solvela_x402::types::ChannelVoucherPayload,
) -> Result<Response, GatewayError> {
    // Gate: channels ship DISABLED; the draw additionally requires a DB pool
    // (the durable ledger) AND Redis (the per-channel lock). Any missing → the
    // same 404 as open/close — never a fake in-memory draw. See §3.10.
    if !state.config.channel.enabled {
        return Ok(crate::routes::channel::channel_not_available());
    }
    let Some(pool) = state.db_pool.as_ref() else {
        return Ok(crate::routes::channel::channel_not_available());
    };
    let Some(cache) = state.cache.as_ref() else {
        return Ok(crate::routes::channel::channel_not_available());
    };

    // SDK-contract check: `accepted.amount` MUST equal the per-call price
    // (`compute_service_cost` total). Billing does NOT read this field — the
    // gateway quote is authoritative — but a mismatch signals an SDK↔gateway
    // price disagreement; reject rather than silently ignore it. Static message
    // (never reflect the client-controlled amount string).
    if accepted_amount != cost.total_atomic.to_string() {
        return Err(GatewayError::BadRequest(
            "channel voucher accepted.amount must equal the per-call price for this endpoint"
                .to_string(),
        ));
    }

    // Bind the voucher to THE served request: SHA-256 of the RAW body bytes.
    let request_digest = crate::routes::channel::request_digest(body_bytes);

    // `current_slot` for the voucher expiry check — RPC-free cached slot (5s
    // TTL). `None` (RPC degraded, no cached value) → fail closed with a 503,
    // never serve on a stale-unknown slot. Never a per-call `getSlot`. §0.1/§3.11.
    let Some(current_slot) = crate::routes::escrow::fetch_cached_slot(state).await else {
        return Err(GatewayError::ServiceUnavailable(
            "could not reach the Solana cluster to verify the voucher; please retry shortly"
                .to_string(),
        ));
    };

    // Decode the wire voucher into the frozen verifier `Voucher`, fail-closed on
    // any malformed field (static, safe-to-forward messages).
    let voucher = crate::routes::channel::voucher_from_payload(voucher_payload).map_err(|e| {
        warn!(error = %e, "channel draw: malformed voucher payload");
        GatewayError::BadRequest(e.to_string())
    })?;

    // The channel id (base58) is BOTH the lock key and the ledger load key.
    let channel_id = voucher_payload.channel_id.as_str();

    // Per-channel lock (Redis SET NX EX, channel-distinct prefix). Fail closed
    // if Redis errors (no in-memory fallback — it cannot serialise across
    // instances); reject if a concurrent draw holds it. §3.9 / HALT 4.
    match cache.acquire_channel_draw_lock(channel_id).await {
        Ok(true) => {}
        Ok(false) => {
            return Err(GatewayError::ServiceUnavailable(
                "a draw is already in progress on this channel; please retry shortly".to_string(),
            ));
        }
        Err(e) => {
            warn!(error = %e, "channel draw: lock acquisition failed (Redis)");
            return Err(GatewayError::ServiceUnavailable(
                "payment service is temporarily degraded; please retry shortly".to_string(),
            ));
        }
    }

    // Lock HELD. Run the draw and RELEASE on ALL paths (success AND every
    // failure) — the 120s TTL is only the crash backstop, never the steady-state
    // release (releasing at once lets the next sequential draw proceed
    // immediately). This is the OPPOSITE of the A2A hold-on-success. §8.5.
    let outcome = channel_draw_locked(
        state,
        headers,
        provider,
        search_query,
        cost,
        request_digest,
        current_slot,
        &voucher,
        channel_id,
        pool,
    )
    .await;
    // Release synchronously (NOT detached — a spawned release would race the
    // next sequential draw). But the Redis client has NO per-command timeout, so
    // a hung Redis must not stall the already-earned response: bound the release
    // and fall back to the 120s TTL crash-backstop on timeout.
    if tokio::time::timeout(
        std::time::Duration::from_secs(5),
        cache.release_channel_draw_lock(channel_id),
    )
    .await
    .is_err()
    {
        warn!(
            channel_id,
            "channel draw lock release timed out — lock will expire via TTL (120s)"
        );
    }
    outcome
}

/// The locked body of a channel draw: load state → #499 → verify voucher →
/// serve → POST-serve persist → charge-visible spend/receipt. The caller
/// ([`channel_draw`]) holds the per-channel lock across this whole function and
/// releases it unconditionally afterward.
// ponytail: 10 args threading the already-validated draw context into the
// locked scope; a struct would be ceremony for one call site.
#[allow(clippy::too_many_arguments)]
async fn channel_draw_locked(
    state: &Arc<AppState>,
    headers: &HeaderMap,
    provider: &dyn SearchProvider,
    search_query: SearchQuery,
    cost: ServiceCost,
    request_digest: [u8; 32],
    current_slot: u64,
    voucher: &solvela_x402::channel::Voucher,
    channel_id: &str,
    pool: &sqlx::PgPool,
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
                warn!(error = %e, "channel draw: failed to load channel state");
                return Err(GatewayError::Internal(
                    "could not load the channel".to_string(),
                ));
            }
        };
    let agent_wallet = drawable.agent_wallet;
    let state_view = drawable.state;

    // #499: reject a `require_tenant = TRUE` wallet — SEARCH-exact budget
    // posture (NO `check_budget`). The wallet is sourced from the DB
    // `agent_wallet`, NEVER `extract_payer_wallet`/the voucher (a voucher has no
    // `agent_pubkey` → "unknown"). §3.4 / HALT 7.
    if state.usage.require_tenant_for_wallet(&agent_wallet).await {
        warn!("channel draw rejected: agent wallet requires per-tenant budgeting (#499)");
        return Err(GatewayError::Forbidden(
            "this wallet requires per-tenant budgeting; use POST /v1/chat/completions".to_string(),
        ));
    }

    // billed = the flat quote (= actual = realized on search; no gap, no
    // discount). `verify_voucher` enforces `voucher.cumulative - last == billed`.
    let billed = cost.total_atomic;

    // Verify the voucher — EXACT, fail-closed on every rule. On an AUTHENTICATED
    // rejection (signature already verified inside `verify_voucher`) surface the
    // authoritative `last_cumulative` so a desynced SDK can resync (R9); on a
    // pre-auth rejection surface nothing (not the caller's own ledger). §3.12.
    if let Err(e) =
        solvela_x402::channel::verify_voucher(&state_view, voucher, billed, current_slot)
    {
        return Err(map_voucher_rejection(e, state_view.last_cumulative_atomic));
    }

    // SERVE. The channel fork NEVER touches `verify_and_settle`, the tx-replay
    // cache, or `fire_escrow_claim` (channels have no per-call on-chain tx or
    // claim — HALT 3).
    let results = match provider.search(search_query).await {
        Ok(r) => r,
        Err(e) => {
            // Provider failure: NO persist → `last` does NOT advance → the agent
            // keeps its draw and was NOT debited. LEDGER INVARIANT: a positive
            // spend/receipt exists IFF `last` advanced IFF the agent was debited,
            // so a non-charge writes NOTHING — no false spend row, no false
            // receipt on the 502. The `warn!` is the sole audit trail. (Do not
            // leak upstream internals — GHSA-cgqx-mg48-949v.)
            warn!(
                error = %e,
                provider = %provider.name(),
                channel_id,
                "web-search upstream failed (channel draw) — no charge; last unchanged"
            );
            return Ok((
                StatusCode::BAD_GATEWAY,
                Json(json!({ "error": "search provider error" })),
            )
                .into_response());
        }
    };

    // POST-serve persist (deliver-then-record, #486-safe): advancing `last` IS
    // the debit, so the spend/receipt below are contingent on it committing.
    if let Err(e) = crate::channels::persist_voucher_and_advance(
        pool,
        &crate::channels::VoucherRecord {
            channel_id,
            cumulative_atomic: voucher.cumulative_atomic,
            call_cost_atomic: billed,
            // Search is flat-priced: quote == actual, so the realized advance
            // IS the billed delta (the chat draw passes min(actual, billed)).
            realized_advance_atomic: billed,
            expiry_slot: voucher.expiry_slot,
            nonce: voucher.nonce,
            request_digest: &voucher.request_digest,
            signature: &voucher.signature,
        },
    )
    .await
    {
        // R8: a persist failure AFTER a successful serve is a BOUNDED one-call
        // gateway loss — the agent still receives its results, but `last` did
        // NOT advance (deposit intact), so NO debit occurred and, by the ledger
        // invariant, NO spend/receipt is written (same non-charge rule as the
        // provider-failure arm) and the delivered 200 carries no receipt header.
        // Logged WITH the cumulative so the bounded loss is reconcilable. Do NOT
        // fail the already-delivered response; the agent resyncs on its next
        // draw (R9).
        warn!(
            error = %e,
            channel_id,
            cumulative_atomic = voucher.cumulative_atomic,
            last_cumulative = state_view.last_cumulative_atomic,
            "channel draw: persist failed after a successful serve — bounded \
             one-call gateway loss, NO charge recorded (deposit intact; agent \
             resyncs on next draw)"
        );
        return Ok((StatusCode::OK, Json(json!(results))).into_response());
    }

    // `last` advanced ⇒ the agent WAS debited ⇒ record the positive spend +
    // receipt. This is the ONLY site a channel spend/receipt row is written, so
    // the invariant "positive receipt ⟺ last advanced ⟺ agent debited" holds by
    // construction. wallet = DB `agent_wallet`, `tx_signature = None`, scheme =
    // "channel", no vendor leg.
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
        model: SEARCH_SERVICE_ID.to_string(),
        payment_scheme: "channel".to_string(),
        // ponytail: no `channel_id` receipt column in Pass B; add one when
        // receipts need channel-scoped lookup (a migration, out of this slice).
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
        model: SEARCH_SERVICE_ID.to_string(),
        provider: format!("search:{}", provider.name()),
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
        vendor: None,
    };
    state.usage.log_spend(spend_entry);
    let receipt_header_path = receipts::record_receipt(state.db_pool.as_ref(), receipt_record);

    let mut response = (StatusCode::OK, Json(json!(results))).into_response();
    receipts::insert_receipt_header(&mut response, &receipt_header_path);
    Ok(response)
}

/// Map a `verify_voucher` rejection to a fail-closed `GatewayError`.
///
/// Surfaces the authoritative `last_cumulative` ONLY for AUTHENTICATED
/// rejections — those that occur AFTER `verify_voucher` has verified the
/// ed25519 signature — so a desynced SDK (whose vouchers now `DeltaMismatch` /
/// `NonMonotonicCumulative`) can recompute its next cumulative and resync (R9).
/// A pre-authentication rejection (`InvalidSignature` / `ChannelMismatch`, both
/// checked before/at the signature gate) surfaces nothing: an unauthenticated
/// caller must never learn a channel's balance. §3.12.
fn map_voucher_rejection(
    err: solvela_x402::channel::ChannelVoucherError,
    last_cumulative: u64,
) -> GatewayError {
    use solvela_x402::channel::ChannelVoucherError as E;
    match err {
        // Pre-auth (voucher rules 1 & 2): reject WITHOUT the ledger figure.
        E::ChannelMismatch | E::InvalidSignature => {
            GatewayError::InvalidPayment("channel voucher rejected".to_string())
        }
        // Authenticated, but a BODY mismatch — not a cumulative desync. Telling
        // the SDK to "resync last_cumulative" here would send it into a confused
        // retry loop; point it at the real cause instead, with NO ledger figure.
        E::RequestDigestMismatch => GatewayError::InvalidPayment(
            "voucher request_digest does not match this request; re-sign for the body you are \
             sending"
                .to_string(),
        ),
        // Post-auth CUMULATIVE rejections (rules 4–7): the caller proved control
        // of the session key AND the mismatch is about the cumulative, so
        // surfacing its OWN channel's authoritative last_cumulative is a resync
        // aid (R9), not a third-party leak. The figure rides BOTH the prose
        // message (unchanged) and the structured `last_cumulative` body field
        // (§4b) — SDK trackers consume ONLY the structured field.
        E::Expired { .. }
        | E::NonMonotonicCumulative { .. }
        | E::DeltaMismatch { .. }
        | E::BelowSettled { .. }
        | E::OverDraw { .. } => GatewayError::InvalidPaymentWithResync {
            message: format!(
                "channel voucher rejected; resync from authoritative last_cumulative={last_cumulative}"
            ),
            last_cumulative,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_request_parses_query_only() {
        let req: SearchRequest = serde_json::from_str(r#"{"query":"solana x402"}"#).unwrap();
        assert_eq!(req.query, "solana x402");
        assert_eq!(req.max_results, None);
    }

    #[test]
    fn search_request_parses_max_results() {
        let req: SearchRequest =
            serde_json::from_str(r#"{"query":"solana","max_results":5}"#).unwrap();
        assert_eq!(req.max_results, Some(5));
    }

    #[test]
    fn search_request_rejects_missing_query() {
        let err = serde_json::from_str::<SearchRequest>(r#"{"max_results":5}"#);
        assert!(err.is_err(), "missing query must fail to deserialize");
    }

    // -- pre-auth no-leak (round-1 review item 16) ---------------------------

    /// Pins `verify_voucher`'s check ORDER: a voucher that is BOTH
    /// wrong-signature AND expired must reject as `InvalidSignature` — the
    /// pre-auth arm that surfaces NO ledger figure. If expiry were checked
    /// first, an unauthenticated caller could receive a resync body carrying
    /// the channel's authoritative last_cumulative.
    #[test]
    fn verify_voucher_rejects_bad_signature_before_expiry() {
        use ed25519_dalek::SigningKey;
        let session = SigningKey::from_bytes(&[5u8; 32]);
        let state = solvela_x402::channel::ChannelState {
            channel_id: [1u8; 32],
            deposited_atomic: 50_000,
            settled_atomic: 0,
            last_cumulative_atomic: 0,
            session_key: session.verifying_key().to_bytes(),
            expected_request_digest: [2u8; 32],
        };
        let voucher = solvela_x402::channel::Voucher {
            channel_id: [1u8; 32],
            cumulative_atomic: 10_500,
            expiry_slot: 0, // long expired
            nonce: 1,
            request_digest: [2u8; 32],
            signature: [0x99u8; 64], // garbage — never signed
        };
        let err = solvela_x402::channel::verify_voucher(&state, &voucher, 10_500, 1_000_000)
            .expect_err("must reject");
        assert!(
            matches!(
                err,
                solvela_x402::channel::ChannelVoucherError::InvalidSignature
            ),
            "signature must be checked BEFORE expiry (got {err:?})"
        );
    }

    /// Pins `map_voucher_rejection`'s arm assignment: pre-auth rejections NEVER
    /// produce the structured resync body (no ledger figure for an
    /// unauthenticated caller); post-auth cumulative rejections ALWAYS do,
    /// carrying the authoritative figure.
    #[test]
    fn map_voucher_rejection_resync_only_on_post_auth_arms() {
        use solvela_x402::channel::ChannelVoucherError as E;
        let last = 12_600u64;

        let pre_auth = [
            E::ChannelMismatch,
            E::InvalidSignature,
            E::RequestDigestMismatch,
        ];
        for err in pre_auth {
            match map_voucher_rejection(err, last) {
                GatewayError::InvalidPayment(msg) => {
                    assert!(
                        !msg.contains(&last.to_string()),
                        "pre-auth message must not leak the ledger figure: {msg}"
                    );
                }
                other => panic!("pre-auth arm must map to plain InvalidPayment, got {other:?}"),
            }
        }

        let post_auth = [
            E::Expired {
                expiry_slot: 1,
                current_slot: 100,
                buffer: 0,
            },
            E::NonMonotonicCumulative {
                cumulative: 1,
                last_cumulative: last,
            },
            E::DeltaMismatch {
                expected_billed: 10_500,
                actual_delta: 1,
            },
            E::BelowSettled {
                cumulative: 1,
                settled: 2,
            },
            E::OverDraw {
                cumulative: 99_999,
                deposited: 50_000,
            },
        ];
        for err in post_auth {
            match map_voucher_rejection(err, last) {
                GatewayError::InvalidPaymentWithResync {
                    last_cumulative, ..
                } => assert_eq!(last_cumulative, last),
                other => panic!("post-auth arm must map to the resync body, got {other:?}"),
            }
        }
    }
}
