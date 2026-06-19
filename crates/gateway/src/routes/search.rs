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
use crate::providers::search::SearchQuery;
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
}
