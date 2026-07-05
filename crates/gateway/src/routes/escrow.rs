//! GET /v1/escrow/config — public escrow configuration discovery endpoint.
//!
//! Returns the escrow program ID, Solana network, USDC mint, provider wallet,
//! and the current Solana slot. No authentication required. Clients use this
//! to discover escrow parameters without making a payment attempt.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::Json;
use serde::Serialize;
use serde_json::json;
use tokio::sync::Mutex;

use crate::middleware::rate_limit::{connect_info_client_id, rate_limited_response, PeerAddr};
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
        network: solvela_x402::types::SOLANA_NETWORK.to_string(),
        usdc_mint: state.config.solana.usdc_mint.clone(),
        provider_wallet: state.config.solana.recipient_wallet.clone(),
    };

    (StatusCode::OK, Json(json!(config))).into_response()
}

/// Fetch the current Solana slot, returning a cached value if still fresh.
///
/// On RPC failure, logs a warning and returns `None` — the endpoint still
/// returns the rest of the config without the slot.
///
/// Uses a check-then-act pattern so the `Mutex` is never held across the
/// async RPC call. Two concurrent callers may both fetch on a cache miss —
/// this is benign for a cache.
///
/// `pub(crate)` so the v0 spend-down channel draw (`routes/search.rs`) can
/// source `current_slot` for `verify_voucher`'s expiry check RPC-free on the
/// hot path (no per-call `getSlot`), fail-closed to a 503 on `None`.
pub(crate) async fn fetch_cached_slot(state: &AppState) -> Option<u64> {
    read_cached_slot_with_age(state).await.map(|(slot, _)| slot)
}

/// Like [`fetch_cached_slot`] but FAIL-CLOSED past a staleness bound.
///
/// On an RPC outage the shared cache returns the last value INDEFINITELY (only
/// `None` when nothing was ever cached). That is fine for the discovery
/// endpoint, but a money-path caller — the channel-draw voucher expiry check,
/// which measures `expiry_slot - current_slot` — must NOT accept an arbitrarily
/// old slot: a stale (lower) `current_slot` inflates the expiry buffer and can
/// let a genuinely-expired voucher pass (`solvela_x402::channel::verify_voucher`
/// rule 4). So reject (→ `None`, which the draw turns into a fail-closed 503)
/// once the cached value is older than `max_staleness`. A fresh RPC fetch or a
/// `< SLOT_CACHE_TTL` cache hit is always fresh enough.
pub(crate) async fn fetch_cached_slot_bounded(
    state: &AppState,
    max_staleness: Duration,
) -> Option<u64> {
    read_cached_slot_with_age(state)
        .await
        .filter(|(_, fetched_at)| fetched_at.elapsed() <= max_staleness)
        .map(|(slot, _)| slot)
}

/// Shared cache read backing [`fetch_cached_slot`] and
/// [`fetch_cached_slot_bounded`]: returns the current slot together with WHEN
/// it was fetched (`Instant`), so a caller can bound the staleness. On an RPC
/// failure the last cached `(slot, fetched_at)` is returned unchanged (age
/// preserved); `None` only when nothing has ever been cached.
async fn read_cached_slot_with_age(state: &AppState) -> Option<(u64, Instant)> {
    let cache = &state.slot_cache;

    // 1. Acquire lock, read cached value, release immediately
    let cached = {
        let guard = cache.lock().await;
        *guard
    };

    // 2. Return cached value if within TTL (its original fetch time carries the age)
    if let Some((slot, fetched_at)) = cached {
        if fetched_at.elapsed() < SLOT_CACHE_TTL {
            return Some((slot, fetched_at));
        }
    }

    // 3. Fetch fresh slot WITHOUT holding the lock
    match fetch_slot_from_rpc(&state.http_client, &state.config.solana.rpc_url).await {
        Ok(slot) => {
            // 4. Acquire lock again to write new value
            let now = Instant::now();
            let mut guard = cache.lock().await;
            *guard = Some((slot, now));
            Some((slot, now))
        }
        Err(e) => {
            tracing::warn!(error = %e, "failed to fetch Solana slot for escrow config");
            // Return the stale (slot, fetched_at) if available — the age is
            // preserved so `fetch_cached_slot_bounded` can fail closed on it.
            cached
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
/// - 200 with escrow health when escrow is configured and caller is authorized
/// - 401 when the `Authorization: Bearer <token>` header is missing or invalid
/// - 404 when escrow is not configured **or** no `SOLVELA_ADMIN_TOKEN` is set
///
/// Status is "ok" when everything is healthy, "degraded" when the claim
/// processor is not running or fee payer pool is missing, and "down" when
/// escrow is not operational.
pub async fn escrow_health(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // Gate behind admin token — if not configured, hide the endpoint entirely
    let admin_token = match &state.admin_token {
        Some(t) => t,
        None => {
            return (StatusCode::NOT_FOUND, Json(json!({ "error": "not found" }))).into_response();
        }
    };

    // Validate Bearer token via the secret-safe newtype's constant-time compare
    let authorized = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|token| admin_token.verify(token.as_bytes()));

    if !authorized {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "error": "unauthorized" })),
        )
            .into_response();
    }

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

// ---------------------------------------------------------------------------
// POST /v1/escrow/deposit-tx — unsigned escrow-deposit transaction builder
// ---------------------------------------------------------------------------

/// Default expiry buffer (slots ahead of the current slot) when the caller does
/// not supply `expiry_slot`. Equals the 300-second-timeout equivalent the SDK
/// produces (`300 s × 1000 / 400 ms = 750 slots`), and sits inside the
/// `[MIN_EXPIRY_SLOTS_AHEAD, MAX_EXPIRY_SLOTS_AHEAD]` window. Mirrors the Rust
/// SDK's `escrow_expiry_slot(_, 300)` (`sdks/rust/.../signer.rs`).
const DEFAULT_EXPIRY_SLOTS_AHEAD: u64 = 750;

/// Minimum slots ahead of the current slot an escrow expiry may be. Mirrors the
/// Rust SDK's `MIN_ESCROW_EXPIRY_SLOTS_AHEAD = 150`, which itself
/// mirrors-and-exceeds the gateway/on-chain `MIN_EXPIRY_BUFFER_SLOTS = 50`. A
/// too-near expiry would be bounced by the verifier, so we reject it here.
const MIN_EXPIRY_SLOTS_AHEAD: u64 = 150;

/// Maximum slots ahead of the current slot an escrow expiry may be. Mirrors the
/// Rust SDK's `MAX_ESCROW_EXPIRY_SLOTS_AHEAD = 10_000` (~66 min). An explicit
/// value beyond this is clamped down, never silently extended to "never".
const MAX_EXPIRY_SLOTS_AHEAD: u64 = 10_000;

/// Request body for `POST /v1/escrow/deposit-tx`.
///
/// `amount` is the deposit in **atomic USDC units** (6-decimal micro-USDC) as an
/// integer string (e.g. `"2625"`), NOT a decimal USDC string. It must parse as a
/// `u64 > 0`.
///
/// `#[serde(deny_unknown_fields)]`: a money-path request must reject an unknown
/// field rather than silently ignore it — a typo like `"ammount"` would
/// otherwise drop the caller's intended amount and fall through to the default
/// expiry / a missing required field, building a deposit the caller never asked
/// for. This is a gateway-local request type (not the x402 wire `EscrowPayload`),
/// so strict rejection is safe and does not break protocol forward-compat.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DepositTxRequest {
    /// Base58 agent wallet pubkey (the external signer / fee payer).
    pub agent_wallet: String,
    /// Base64 of the 32-byte `service_id` seeding the escrow PDA.
    pub service_id: String,
    /// Atomic USDC units, integer string, must be > 0.
    pub amount: String,
    /// Optional absolute expiry slot. Absent → `current_slot +
    /// DEFAULT_EXPIRY_SLOTS_AHEAD`. Present → clamped into the valid window;
    /// rejected if already below `current_slot + MIN_EXPIRY_SLOTS_AHEAD`.
    #[serde(default)]
    pub expiry_slot: Option<u64>,
}

/// The deterministic inputs a client re-derives the message from to verify what
/// it is about to sign ("verify what you sign").
#[derive(Debug, Clone, Serialize)]
pub struct DecodedIntent {
    pub program_id: String,
    pub usdc_mint: String,
    pub provider: String,
    pub escrow_pda: String,
    pub vault_ata: String,
    /// Atomic USDC units, integer string (echoes the validated request amount).
    pub amount: String,
    /// Base64 of the 32-byte service_id (echoes the validated request).
    pub service_id: String,
    pub expiry_slot: u64,
    /// Base58 recent blockhash embedded in the message.
    pub recent_blockhash: String,
}

/// Response body for `POST /v1/escrow/deposit-tx`.
#[derive(Debug, Clone, Serialize)]
pub struct DepositTxResponse {
    /// Base64 of the UNSIGNED legacy message bytes. The signer signs THESE bytes
    /// and assembles `compact-u16(1) || signature(64) || message`.
    pub message: String,
    pub decoded_intent: DecodedIntent,
    pub network: String,
}

/// POST /v1/escrow/deposit-tx
///
/// Returns an UNSIGNED escrow-deposit legacy message for an external signer
/// (browser wallet / KMS / hardware). The gateway NEVER holds a private key on
/// this path — it derives the agent's PDA/ATA from the supplied public key,
/// fetches a recent blockhash, builds the canonical message via the
/// golden-vector-pinned `build_deposit_message`, and returns the message plus a
/// `decoded_intent` so the client can re-derive and byte-compare before signing.
///
/// Status codes:
/// - 200 with `{ message, decoded_intent, network }` on success
/// - 400 on a malformed pubkey / service_id / amount or an out-of-window
///   explicit `expiry_slot`
/// - 404 `{"error":"escrow not configured"}` when `escrow_program_id` is unset
/// - 503 (fail-closed) when the recent-blockhash RPC is unavailable — never a
///   silently-built unsubmittable transaction
///
/// All amount handling is integer atomic-unit (no float). Validation happens
/// before the single RPC call so malformed input never triggers network I/O.
pub async fn deposit_tx(
    State(state): State<Arc<AppState>>,
    // Infallible peer-address extractor (same as the faucet/receipts routes):
    // `None` when `ConnectInfo` is absent, degrading to the stricter "unknown"
    // bucket rather than 500-ing. Extracted before the body so the rate limit can
    // gate on the real TCP peer IP.
    peer_addr: PeerAddr,
    Json(req): Json<DepositTxRequest>,
) -> axum::response::Response {
    use base64::Engine;
    use solvela_x402::escrow::deposit::{
        build_deposit_message, derive_deposit_addresses, UnsignedDepositParams,
    };

    // Anti-amplification: this route is public and unauthenticated and each call
    // can fan out to Solana RPC (slot + blockhash). Enforce a per-IP cap BEFORE
    // any RPC work (and before the escrow-configured / validation checks) so an
    // abusive caller is rejected at the cheapest point and cannot drive RPC
    // traffic. Keyed on the TCP peer IP, never a client-supplied header
    // (X-Forwarded-For et al. are forgeable — GHSA-6ggq-cvwx-4f67); absent
    // `ConnectInfo` falls back to the shared stricter "unknown" bucket. Mirrors
    // the faucet/receipts in-handler limiters and reuses the same 429 envelope.
    let client_id = connect_info_client_id(peer_addr.0);
    if state
        .deposit_tx_rate_limiter
        .check(&client_id)
        .await
        .is_err()
    {
        metrics::counter!("solvela_deposit_tx_rate_limited_total").increment(1);
        tracing::warn!(client_id = %client_id, "escrow deposit-tx rate limit exceeded");
        return rate_limited_response(state.deposit_tx_rate_limiter.config());
    }

    // Fail closed if escrow is not configured (mirror escrow_config's 404 body).
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

    // --- Validate inputs BEFORE any RPC call (no network I/O on bad input) ---

    // agent_wallet: base58 → 32-byte pubkey.
    let agent_pubkey = match solvela_x402::escrow::pda::decode_bs58_pubkey(&req.agent_wallet) {
        Ok(pk) => pk,
        Err(_) => {
            return bad_request("agent_wallet must be a base58-encoded 32-byte pubkey");
        }
    };

    // service_id: base64 → exactly 32 bytes.
    let service_id_bytes = match base64::engine::general_purpose::STANDARD.decode(&req.service_id) {
        Ok(b) => b,
        Err(_) => return bad_request("service_id must be valid base64"),
    };
    let service_id: [u8; 32] = match service_id_bytes.try_into() {
        Ok(arr) => arr,
        Err(v) => {
            let len = v.len();
            return bad_request(&format!(
                "service_id must decode to exactly 32 bytes, got {len}"
            ));
        }
    };

    // amount: atomic-unit integer string, must be > 0. Integer parse only — the
    // field is atomic micro-USDC, never a decimal string, so a `u64` parse is the
    // correct (and only float-free) reading. Rejects "", "-5", "0.5", overflow.
    let amount: u64 = match req.amount.parse::<u64>() {
        Ok(a) => a,
        Err(_) => {
            return bad_request(
                "amount must be a positive integer of atomic USDC units (no decimals, no overflow)",
            );
        }
    };
    if amount == 0 {
        return bad_request("amount must be greater than zero");
    }
    // Reject a NON-CANONICAL amount string (leading zeros, `+` sign, surrounding
    // whitespace): `"01".parse::<u64>()` is `1`, but `decoded_intent.amount`
    // echoes the canonical `"1"`. A strict "verify what you sign" client does a
    // string compare of the amount it sent against the echoed intent — so the
    // request string and the echoed intent must be byte-equal. Re-serializing the
    // parsed `u64` is the canonical form; anything that differs is rejected here
    // rather than silently canonicalized.
    if req.amount != amount.to_string() {
        return bad_request(
            "amount must be a canonical integer string (no leading zeros, sign, or whitespace)",
        );
    }

    // --- Current slot for expiry-window logic (CACHED) ---
    // Reuse the same 5s `SlotCache` the `escrow_config` handler uses rather than
    // an uncached `getSlot` per request: this removes one RPC round-trip per
    // request (RPC-amplification surface) and closes the slot/blockhash skew
    // window. `fetch_cached_slot` returns the cached value when fresh, otherwise
    // fetches once (and on failure returns a stale cached value if any). It
    // yields `None` only when there is NO usable slot at all (no fresh fetch AND
    // no cached value) — fail closed with the same static 503 as before, never a
    // silently-built unsubmittable transaction.
    let current_slot = match fetch_cached_slot(&state).await {
        Some(s) => s,
        None => {
            // Detail already logged at warn! inside `fetch_cached_slot`. Surface
            // only a clean, static message — never raw RPC internals
            // (GHSA-cgqx-mg48-949v).
            return service_unavailable("could not reach the Solana cluster to build the deposit");
        }
    };

    // Resolve the expiry slot from the current slot + the configured window.
    let expiry_slot = match resolve_expiry_slot(req.expiry_slot, current_slot) {
        Ok(slot) => slot,
        Err(msg) => return bad_request(msg),
    };

    // --- Single RPC call: fetch a recent blockhash for the message ---
    // `confirmed` commitment (not `finalized`): this endpoint's job is to hand
    // back a promptly-submittable transaction, so it wants the freshest,
    // longest-lived blockhash, and keeping it on `confirmed` matches the
    // `confirmed` slot reference above (a finalized blockhash would be ~32 slots
    // staler, shortening the effective expiry window).
    let recent_blockhash = match solvela_x402::solana_rpc::get_latest_blockhash(
        &state.http_client,
        &state.config.solana.rpc_url,
        "confirmed",
    )
    .await
    {
        Ok(bh) => bh,
        Err(e) => {
            tracing::warn!(error = %e, "deposit-tx: failed to fetch recent blockhash");
            return service_unavailable("could not reach the Solana cluster to build the deposit");
        }
    };

    let usdc_mint = state.config.solana.usdc_mint.clone();
    let provider = state.config.solana.recipient_wallet.clone();

    // Build the canonical UNSIGNED message (golden-vector-pinned). The agent
    // pubkey is client-supplied (already validated); the provider/mint/program
    // all come from gateway config, so any builder error here is a gateway-config
    // fault, handled as a 500 below (never a 400 that blames the caller).
    let unsigned = UnsignedDepositParams {
        agent_pubkey,
        provider_wallet_b58: provider.clone(),
        usdc_mint_b58: usdc_mint.clone(),
        escrow_program_id_b58: escrow_program_id.clone(),
        amount,
        service_id,
        expiry_slot,
        recent_blockhash,
    };
    // By this point the client-controlled inputs (agent_pubkey, service_id,
    // amount) are all validated, so any `build_deposit_message` failure is a
    // GATEWAY-CONFIG fault (a malformed provider/mint/program in server config),
    // NOT a client error — return 500, never a 400 that blames the caller. Fail
    // closed and log the detail (the message stays free of internal specifics).
    let message_bytes = match build_deposit_message(&unsigned) {
        Ok(m) => m,
        Err(e) => {
            tracing::error!(error = %e, "deposit-tx: failed to build unsigned message (gateway config?)");
            return internal_error("could not build the deposit transaction");
        }
    };

    // Derive escrow PDA + vault ATA for the decoded_intent (reuse canonical
    // derivation; do not reimplement). Same config-fault classification as above.
    let derived = match derive_deposit_addresses(
        &agent_pubkey,
        &service_id,
        &usdc_mint,
        &escrow_program_id,
    ) {
        Ok(d) => d,
        Err(e) => {
            tracing::error!(error = %e, "deposit-tx: failed to derive escrow addresses (gateway config?)");
            return internal_error("could not derive the escrow accounts");
        }
    };

    let response = DepositTxResponse {
        message: base64::engine::general_purpose::STANDARD.encode(&message_bytes),
        decoded_intent: DecodedIntent {
            program_id: escrow_program_id,
            usdc_mint,
            provider,
            escrow_pda: bs58::encode(derived.escrow_pda).into_string(),
            vault_ata: bs58::encode(derived.vault_ata).into_string(),
            // Echo the *validated* amount as an atomic-unit integer string.
            amount: amount.to_string(),
            // Echo the *validated* service_id (re-encode the bytes we used, so
            // the response is canonical even if the request used a non-canonical
            // base64 variant).
            service_id: base64::engine::general_purpose::STANDARD.encode(service_id),
            expiry_slot,
            recent_blockhash: bs58::encode(recent_blockhash).into_string(),
        },
        network: solvela_x402::types::SOLANA_NETWORK.to_string(),
    };

    (StatusCode::OK, Json(json!(response))).into_response()
}

/// Static, RELATIVE client-facing message for a too-near explicit `expiry_slot`.
///
/// Deliberately does NOT embed the live absolute slot or the caller's echoed
/// value: the absolute `min_slot` reveals the gateway's current chain view, and
/// echoing caller input back is needless reflection. The relative constant
/// (`MIN_EXPIRY_SLOTS_AHEAD`) is fixed and safe to publish; the dynamic detail is
/// logged at `debug!` in [`resolve_expiry_slot`] for operators only.
///
/// The literal `150` below is pinned to [`MIN_EXPIRY_SLOTS_AHEAD`] by the
/// compile-time guard immediately following, so a change to the constant fails
/// the build (forcing this string to be updated) rather than silently drifting.
const EXPIRY_BELOW_MIN_MESSAGE: &str =
    "expiry_slot must be at least 150 slots ahead of the current slot";
const _: () = assert!(MIN_EXPIRY_SLOTS_AHEAD == 150);

/// Resolve the absolute expiry slot from an optional explicit value and the
/// current slot, using the SDK-aligned `[MIN, MAX]` window.
///
/// - `None` → `current_slot + DEFAULT_EXPIRY_SLOTS_AHEAD`.
/// - `Some(slot)`:
///   - reject (`Err`) if `slot < current_slot + MIN_EXPIRY_SLOTS_AHEAD` (a
///     too-near expiry the verifier would bounce — fail closed, do not silently
///     bump it up),
///   - clamp DOWN to `current_slot + MAX_EXPIRY_SLOTS_AHEAD` if it exceeds the
///     cap (never silently extend to "never").
///
/// The `Err` carries the STATIC, RELATIVE [`EXPIRY_BELOW_MIN_MESSAGE`] — the live
/// absolute slot and the caller's echoed value are logged at `debug!` only, never
/// returned on the wire. Saturating arithmetic guards the `current_slot + buffer`
/// overflow case.
fn resolve_expiry_slot(explicit: Option<u64>, current_slot: u64) -> Result<u64, &'static str> {
    let min_slot = current_slot.saturating_add(MIN_EXPIRY_SLOTS_AHEAD);
    let max_slot = current_slot.saturating_add(MAX_EXPIRY_SLOTS_AHEAD);
    match explicit {
        None => Ok(current_slot.saturating_add(DEFAULT_EXPIRY_SLOTS_AHEAD)),
        Some(slot) => {
            if slot < min_slot {
                // Dynamic detail (live slot + caller value) is operator-only.
                tracing::debug!(
                    expiry_slot = slot,
                    current_slot,
                    min_slot,
                    "deposit-tx: explicit expiry_slot below the minimum buffer"
                );
                return Err(EXPIRY_BELOW_MIN_MESSAGE);
            }
            // Above the cap → clamp down (never extend to never).
            Ok(slot.min(max_slot))
        }
    }
}

/// Build a 400 Bad Request JSON response with a safe, static-ish message.
fn bad_request(message: &str) -> axum::response::Response {
    crate::error::GatewayError::BadRequest(message.to_string()).into_response()
}

/// Build a 503 Service Unavailable JSON response (RPC unreachable). The message
/// is static and free of RPC internals.
fn service_unavailable(message: &str) -> axum::response::Response {
    crate::error::GatewayError::ServiceUnavailable(message.to_string()).into_response()
}

/// Build a 500 Internal Server Error JSON response for a gateway-config fault
/// (the client-controlled inputs are already validated by the time this is
/// reachable). `GatewayError::Internal` discards the inner message client-side
/// and emits a static "Internal server error" body, so no internal detail leaks.
fn internal_error(message: &str) -> axum::response::Response {
    crate::error::GatewayError::Internal(message.to_string()).into_response()
}

/// Make a `getSlot` JSON-RPC call to the Solana cluster.
async fn fetch_slot_from_rpc(client: &reqwest::Client, rpc_url: &str) -> Result<u64, String> {
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
            escrow_program_id: "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU".to_string(),
            current_slot: Some(298_765_432),
            network: "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp".to_string(),
            usdc_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            provider_wallet: "RecipientWallet111111111111111111111111111111".to_string(),
        };

        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(
            json["escrow_program_id"],
            "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU"
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

    // -----------------------------------------------------------------------
    // resolve_expiry_slot — pure window logic (no RPC)
    // -----------------------------------------------------------------------

    #[test]
    fn resolve_expiry_default_uses_default_buffer() {
        // No explicit expiry → current_slot + DEFAULT_EXPIRY_SLOTS_AHEAD (750).
        assert_eq!(
            resolve_expiry_slot(None, 1_000_000).unwrap(),
            1_000_000 + DEFAULT_EXPIRY_SLOTS_AHEAD
        );
    }

    #[test]
    fn resolve_expiry_explicit_within_window_passes_through() {
        // A value comfortably inside [min, max] is returned unchanged.
        let current = 1_000_000;
        let slot = current + 800; // between 150 and 10_000 ahead
        assert_eq!(resolve_expiry_slot(Some(slot), current).unwrap(), slot);
    }

    #[test]
    fn resolve_expiry_below_min_buffer_is_rejected() {
        // Too-near expiry (verifier would bounce it) → Err, NOT a silent bump-up.
        let current = 1_000_000;
        // current + 149 is below the 150 floor.
        let err = resolve_expiry_slot(Some(current + 149), current).unwrap_err();
        // The error message is STATIC and RELATIVE — it must NOT echo the live
        // absolute slot or the caller's value (fix #6). It must mention the
        // relative buffer requirement.
        assert_eq!(err, EXPIRY_BELOW_MIN_MESSAGE);
        assert!(
            err.contains("at least 150 slots ahead"),
            "must state the relative buffer, got: {err}"
        );
        assert!(
            !err.contains("1000000") && !err.contains(&(current + 149).to_string()),
            "message must not leak the live slot or the caller's value: {err}"
        );
        // An absolute slot far in the past is likewise rejected.
        assert!(resolve_expiry_slot(Some(1), current).is_err());
        assert!(resolve_expiry_slot(Some(0), current).is_err());
    }

    #[test]
    fn resolve_expiry_at_exact_min_is_accepted() {
        let current = 1_000_000;
        let at_min = current + MIN_EXPIRY_SLOTS_AHEAD;
        assert_eq!(resolve_expiry_slot(Some(at_min), current).unwrap(), at_min);
    }

    #[test]
    fn resolve_expiry_above_max_is_clamped_down() {
        // Beyond the cap → clamped DOWN to current + MAX (never extended).
        let current = 1_000_000;
        let way_future = current + MAX_EXPIRY_SLOTS_AHEAD + 5_000;
        assert_eq!(
            resolve_expiry_slot(Some(way_future), current).unwrap(),
            current + MAX_EXPIRY_SLOTS_AHEAD
        );
    }

    #[test]
    fn resolve_expiry_saturates_on_current_slot_overflow() {
        // current_slot near u64::MAX must not panic; min/max saturate.
        let current = u64::MAX - 10;
        // Default buffer saturates to u64::MAX.
        assert_eq!(resolve_expiry_slot(None, current).unwrap(), u64::MAX);
        // An explicit u64::MAX is >= the saturated min and <= the saturated max,
        // so it passes through.
        assert_eq!(
            resolve_expiry_slot(Some(u64::MAX), current).unwrap(),
            u64::MAX
        );
    }

    #[test]
    fn expiry_window_constants_are_sdk_aligned() {
        // Guard against drift from the Rust SDK signer constants.
        assert_eq!(MIN_EXPIRY_SLOTS_AHEAD, 150);
        assert_eq!(MAX_EXPIRY_SLOTS_AHEAD, 10_000);
        assert_eq!(DEFAULT_EXPIRY_SLOTS_AHEAD, 750);
        // The floor must be >= the gateway/on-chain MIN_EXPIRY_BUFFER_SLOTS (50)
        // and the default must sit inside [MIN, MAX]. Compile-time guards.
        const _: () = assert!(MIN_EXPIRY_SLOTS_AHEAD >= 50);
        const _: () = assert!(DEFAULT_EXPIRY_SLOTS_AHEAD >= MIN_EXPIRY_SLOTS_AHEAD);
        const _: () = assert!(DEFAULT_EXPIRY_SLOTS_AHEAD <= MAX_EXPIRY_SLOTS_AHEAD);
    }
}
