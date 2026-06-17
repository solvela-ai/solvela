//! OpenAPI + x402 discovery surfaces.
//!
//! Three additive, public, unauthenticated discovery endpoints that let
//! OpenAPI-first x402 crawlers (e.g. x402scan) index Solvela as *payable +
//! invocable* instead of "strict non-invocable / skipped":
//!
//! - `GET /openapi.json` — the canonical machine-readable contract, served
//!   **verbatim** from `docs/openapi.json` via `include_str!` (single source of
//!   truth, kept in lock-step with the `tests/openapi_contract.rs` drift guard
//!   and the pay.sh catalog sidecar). The paid operation carries non-empty
//!   `accepts[]` (on the live 402), an input request-body schema, and the
//!   `x-payment-info` extension — the trio x402scan's discovery spec requires.
//! - `GET /.well-known/x402` — the `/.well-known/x402` resolution surface, a
//!   tiny `{ "version": 1, "resources": [<resource_url>] }` document pointing at
//!   the payable chat endpoint.
//! - `GET /favicon.ico` — a minimal embedded icon so discovery audit tools stop
//!   flagging `FAVICON_MISSING`.
//!
//! None of these touch the 402 wire contract, signing, verification, or any
//! USDC math: the `x-payment-info` price `min`/`max` are static, **non-binding**
//! discovery hints (`mode: "dynamic"` is authoritative — the real per-request
//! price is the dynamic 402 challenge). Purely additive discovery metadata.

use std::sync::Arc;

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use crate::a2a::agent_card::public_base_url;
use crate::AppState;

/// The canonical spec, embedded verbatim at compile time. Same file the
/// `tests/openapi_contract.rs` drift guard embeds (`../../../docs/openapi.json`
/// from the tests dir; `../../../../docs/openapi.json` from here) — there is
/// exactly one source of truth, so the served bytes and the contract-checked
/// bytes cannot diverge.
const OPENAPI_JSON: &str = include_str!("../../../../docs/openapi.json");

/// A minimal valid 16x16 ICO, embedded so discovery audit tools that flag a
/// missing favicon are satisfied. Not a brand asset surface — purely to clear
/// the `FAVICON_MISSING` audit check.
const FAVICON_ICO: &[u8] = include_bytes!("../../static/favicon.ico");

/// `GET /openapi.json` — serve the canonical OpenAPI spec verbatim.
///
/// Public, no auth, no payment. Body is the embedded `docs/openapi.json` byte
/// for byte with an explicit `application/json` content type.
pub async fn openapi_spec() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        OPENAPI_JSON,
    )
        .into_response()
}

/// `GET /.well-known/x402` — the `/.well-known/x402` discovery resolution
/// surface.
///
/// Returns `{ "version": 1, "resources": ["<resource_url>"] }` where
/// `<resource_url>` is `{public_url}/v1/chat/completions`. The base URL is
/// resolved exactly the way the A2A AgentCard resolves it (configured
/// `server.public_url`, with the `http://{host}:{port}` dev fallback) — never
/// hardcoded — so the advertised resource is correct in any deployment.
pub async fn well_known_x402(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let base = public_base_url(&state);
    let resource_url = format!("{base}/v1/chat/completions");
    Json(json!({
        "version": 1,
        "resources": [resource_url],
    }))
}

/// `GET /favicon.ico` — serve the embedded favicon.
pub async fn favicon() -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "image/x-icon")],
        FAVICON_ICO,
    )
        .into_response()
}
