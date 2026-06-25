//! POST /v1/messages — inbound Anthropic-Messages-compatible endpoint.
//!
//! Translates an Anthropic Messages request into Solvela's internal
//! OpenAI-shaped [`ChatRequest`], runs the EXISTING chat pipeline via
//! [`chat_completions_inner`] (the single home of the x402 money path), then
//! translates the resulting [`ChatResponse`] back into the Anthropic Messages
//! response shape. No new auth, no new payment logic — this is a thin
//! translate → call-core → translate wrapper (CLAUDE.md rule #14 posture: a
//! protocol adapter, not new payment logic).
//!
//! SCOPE (PR1): text-only, non-streaming. Streaming (SSE), tools, `count_tokens`,
//! and bearer/spend-down auth are LATER PRs. The translation layer rejects
//! `stream:true`, image content, and tool blocks with a clear error rather than
//! silently degrading.
//!
//! ## Error envelopes
//!
//! Two distinct shapes, by design:
//!
//! - **402 Payment Required** keeps the x402 [`PaymentRequired`] body verbatim
//!   (identical to `/v1/chat/completions`), so an x402 client / registry probe
//!   sees the canonical challenge regardless of which endpoint it called. This
//!   includes the unpaid-quote 402, the discovery 402, and an invalid-payment
//!   402.
//! - **Every other 4xx/5xx** is re-wrapped into the Anthropic error envelope
//!   `{"type":"error","error":{"type":…,"message":…}}` so a native Anthropic /
//!   Claude Code client parses gateway errors the way it expects. The status
//!   code is preserved from the underlying [`GatewayError`].

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderName, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;
use tracing::warn;

use solvela_protocol::ChatResponse;

use crate::error::GatewayError;
use crate::providers::anthropic_inbound::{
    anthropic_request_to_chat, anthropic_request_to_native_skeleton, chat_response_to_anthropic,
    AnthropicInboundError, AnthropicMessagesRequest,
};
use crate::routes::chat::{
    chat_completions_discovery, chat_completions_inner, resolve_model_provider, WireDialect,
};
use crate::AppState;

/// The resource URL this endpoint binds payments to. A payment signed for
/// `/v1/messages` can never be replayed against `/v1/chat/completions` (and
/// vice versa) — see the resource-URL check in `chat_completions_inner`.
const MESSAGES_RESOURCE_URL: &str = "/v1/messages";

/// Upper bound when buffering the inner-core RESPONSE body (the upstream LLM
/// completion) so it can be re-translated into the Anthropic shape. This is a
/// RESPONSE-buffer ceiling — distinct from `RequestBodyLimitLayer`, which gates
/// inbound REQUEST bodies. 10 MiB is well above any realistic completion;
/// exceeding it fails closed (a 502) rather than allocating unbounded memory.
const MAX_RESPONSE_BODY_BYTES: usize = 10 * 1024 * 1024;

/// Response headers worth carrying across from the inner OpenAI response onto
/// the translated Anthropic response. These are payment/audit-relevant
/// (receipt, session) and providers' debug/fallback markers — none are
/// body-shape headers (content-type is reset by the new `Json` body).
const FORWARDED_RESPONSE_HEADERS: &[&str] = &[
    "x-solvela-receipt",
    "x-solvela-session",
    "x-rcr-session",
    "x-session-id",
    "x-solvela-fallback",
    "x-rcr-fallback",
    "x-solvela-debug",
    "x-rcr-debug",
];

/// POST /v1/messages — Anthropic Messages API compatibility endpoint.
///
/// Flow:
/// 1. Parse the Anthropic Messages request (raw `Bytes` so an unpaid bad/empty
///    body returns the discovery 402, mirroring `/v1/chat/completions`).
/// 2. Translate to the internal [`ChatRequest`].
/// 3. Run the shared money-path core ([`chat_completions_inner`]) with this
///    endpoint's `resource_url` — unpaid → 402, paid → provider call.
/// 4. Translate the internal [`ChatResponse`] back into the Anthropic Messages
///    response shape (preserving payment/audit headers).
pub async fn create_message(
    State(state): State<Arc<AppState>>,
    peer_addr: crate::middleware::rate_limit::PeerAddr,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // Mirror the chat route's discovery posture: a PAYING client gets a real
    // validation error on a bad body; an UNPAID probe with a bad/empty body
    // gets the discovery 402 so x402 registry health-checkers see the challenge.
    let has_payment_header = headers
        .get("payment-signature")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| !v.is_empty());

    let anthropic_req: AnthropicMessagesRequest = match serde_json::from_slice(&body) {
        Ok(req) => req,
        Err(e) => {
            if has_payment_header {
                // Paying client, bad body → real validation error in the
                // Anthropic envelope (a native client expects this shape).
                return anthropic_error_response(
                    StatusCode::BAD_REQUEST,
                    "invalid_request_error",
                    format!("invalid Anthropic Messages request body: {e}"),
                );
            }
            // Unpaid probe with a bad/empty body → advertise the resource via
            // the x402 discovery 402 (kept as the x402 body, NOT the Anthropic
            // envelope — registry probes and x402 clients read this shape).
            return chat_completions_discovery(&state, MESSAGES_RESOURCE_URL).into_response();
        }
    };

    // Streaming is OUT OF SCOPE on both the native and reshape branches (native
    // SSE is a follow-up). Reject it LOUDLY here, before model resolution, so a
    // `stream:true` request never silently serves non-streaming on either path
    // (mirrors the strict translator's rejection; keeps the branch's current
    // behavior per the scope decision).
    if anthropic_req.stream {
        return anthropic_error_response(
            StatusCode::BAD_REQUEST,
            "invalid_request_error",
            "streaming responses are not yet supported on POST /v1/messages; \
             set \"stream\": false (SSE support is planned for a later release)"
                .to_string(),
        );
    }

    // Decide the NATIVE-vs-RESHAPE fork from the RESOLVED model's provider.
    // Build a lenient skeleton (model + flattened text) so the resolver can run
    // alias/profile resolution; the skeleton is also the ChatRequest the money
    // core uses for its structural checks on the native branch. The core
    // re-resolves and re-checks `provider == "anthropic"` itself, so this only
    // selects the inbound translation — it is not authoritative.
    let mut native_skeleton = anthropic_request_to_native_skeleton(&anthropic_req);

    // INBOUND MODEL-ID CONTRACT (`/v1/messages` only): Claude Code and every
    // native `api.anthropic.com` client address models by their BARE Anthropic id
    // (e.g. `claude-sonnet-4-6`, or `ANTHROPIC_SMALL_FAST_MODEL`'s haiku), NOT by
    // the gateway-canonical `anthropic/<id>` used on `/v1/models` and
    // `/v1/chat/completions`. A bare id is not a registry key, so without this it
    // resolves to `ModelNotFound` (the native passthrough never engages).
    //
    // If the inbound model does NOT already resolve (alias / profile / canonical
    // id all miss) but IS a known bare Anthropic id, rewrite the skeleton's model
    // to the canonical id so the existing resolver routes it native. This is
    // additive and `/v1/messages`-scoped — it never reinterprets a model that
    // already resolves, and it never default-routes an unknown id (an unresolved
    // non-Anthropic-bare id still falls through to `ModelNotFound` downstream).
    // The relay still rewrites the relayed `model` field to the bare upstream id
    // (see `run_native_relay`), so this canonicalization is purely for routing.
    if resolve_model_provider(&native_skeleton, &state).is_none() {
        if let Some(canonical) = state
            .model_registry
            .resolve_anthropic_model_id(&native_skeleton.model)
            .map(|m| m.id.clone())
        {
            native_skeleton.model = canonical;
        }
    }

    let is_native =
        resolve_model_provider(&native_skeleton, &state).as_deref() == Some("anthropic");

    if is_native {
        // NATIVE passthrough: the relay forwards the ORIGINAL bytes verbatim, so
        // we do NOT run the strict translator (which would reject tools/thinking
        // and defeat the entire point). Forward the inbound version/beta headers
        // verbatim; default `anthropic-version` to the GA value when absent. The
        // inbound Solvela bearer is NEVER forwarded (the relay sends the
        // gateway's own `x-api-key`).
        let anthropic_version = headers
            .get("anthropic-version")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let anthropic_beta = headers
            .get("anthropic-beta")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let dialect = WireDialect::AnthropicMessages {
            original_body: body,
            anthropic_version,
            anthropic_beta,
        };
        return match chat_completions_inner(
            state,
            peer_addr,
            headers,
            native_skeleton,
            MESSAGES_RESOURCE_URL,
            dialect,
        )
        .await
        {
            // The native relay's `Ok` response IS the raw Anthropic body —
            // forward it VERBATIM (do NOT re-translate; it is already the
            // Anthropic wire shape, and re-parsing it as a `ChatResponse` would
            // fail-close to a 502).
            Ok(response) => forward_native_success_response(response).await,
            Err(err) => translate_error(err).await,
        };
    }

    // RESHAPE path (cross-provider: an Anthropic-shaped request whose resolved
    // model is NOT an Anthropic model — reachable only via an eco/auto alias).
    // Run the STRICT translator UNCHANGED: a translation error (image/tool
    // block, or `stream:true`) is a client 4xx in the Anthropic envelope — it
    // never reaches the money path. This keeps the loud silent-degradation guard
    // fully intact for the lossy cross-provider exception.
    let chat_req = match anthropic_request_to_chat(anthropic_req) {
        Ok(req) => req,
        Err(e) => {
            let status = match &e {
                AnthropicInboundError::InvalidBody(_) => StatusCode::BAD_REQUEST,
                // Images are accepted on the wire but not yet processed — 415,
                // matching the chat route's `UnsupportedMediaType` posture.
                AnthropicInboundError::ImageUnsupported => StatusCode::UNSUPPORTED_MEDIA_TYPE,
                // Tools are an unsupported REQUEST feature, not an unsupported
                // media type — 400 (415 is media-type-specific and would be a
                // misleading status for `tools`/`tool_use`).
                AnthropicInboundError::ToolUseUnsupported => StatusCode::BAD_REQUEST,
            };
            let error_type = match &e {
                AnthropicInboundError::InvalidBody(_) => "invalid_request_error",
                AnthropicInboundError::ImageUnsupported => "invalid_request_error",
                AnthropicInboundError::ToolUseUnsupported => "invalid_request_error",
            };
            return anthropic_error_response(status, error_type, e.to_string());
        }
    };

    // Run the SHARED money-path core on the reshape branch. The core sees a
    // non-Anthropic resolved model, so `WireDialect::AnthropicMessages` does NOT
    // trigger the native relay — it takes the existing OpenAI provider pipeline,
    // and the OpenAI `ChatResponse` is re-translated to the Anthropic shape.
    // (The `original_body` is carried but unused on this branch.)
    let dialect = WireDialect::AnthropicMessages {
        original_body: body,
        anthropic_version: None,
        anthropic_beta: None,
    };
    match chat_completions_inner(
        state,
        peer_addr,
        headers,
        chat_req,
        MESSAGES_RESOURCE_URL,
        dialect,
    )
    .await
    {
        Ok(response) => translate_success_response(response).await,
        Err(err) => translate_error(err).await,
    }
}

/// GET /v1/messages — x402 discovery probe.
///
/// A GET carries no body and cannot be paid, so it always returns the discovery
/// 402 challenge bound to `/v1/messages`. Mirrors
/// `chat_completions_discovery_get` so registry health-checkers can confirm the
/// endpoint speaks x402.
pub async fn create_message_discovery_get(State(state): State<Arc<AppState>>) -> Response {
    chat_completions_discovery(&state, MESSAGES_RESOURCE_URL).into_response()
}

/// Forward a successful NATIVE-passthrough inner-core [`Response`] VERBATIM.
///
/// On the native branch the inner core's `Ok` response IS the upstream Anthropic
/// response, relayed byte-for-byte (preserving thinking `signature`,
/// `redacted_thinking`, native `tool_use` blocks, and cache-token usage). It is
/// already the Anthropic Messages wire shape, so — unlike the reshape path's
/// [`translate_success_response`] — we MUST NOT re-parse it as a `ChatResponse`
/// and re-emit (that would fail-close to a 502). We forward it unchanged; the
/// payment/audit headers (receipt, session) the money path attached are already
/// on it. A non-200 reaching here is unexpected (the core's served path returns
/// 200) — surface it as a redacted Anthropic error rather than forwarding an
/// unknown-shape body as a success.
async fn forward_native_success_response(response: Response) -> Response {
    let status = response.status();
    if status == StatusCode::OK {
        return response;
    }
    warn!(
        status = %status,
        "unexpected non-200 status on the native-relay Ok path for /v1/messages"
    );
    anthropic_error_response(
        StatusCode::BAD_GATEWAY,
        "api_error",
        "Upstream returned an unexpected response.".to_string(),
    )
}

/// Translate a successful inner-core [`Response`] (a serialized OpenAI
/// [`ChatResponse`] JSON body) into the Anthropic Messages response shape.
///
/// PR1 is non-streaming, so the inner response is always a JSON body; we
/// buffer it, deserialize into a [`ChatResponse`], and re-emit as the Anthropic
/// shape. Payment/audit headers (receipt, session) are carried across. If the
/// body cannot be deserialized into a [`ChatResponse`] (e.g. an unexpected stub
/// shape), we fail closed with a 502 in the Anthropic envelope rather than
/// emitting a malformed Anthropic body.
async fn translate_success_response(response: Response) -> Response {
    let status = response.status();
    let (parts, body) = response.into_parts();

    // The inner core returns 200 with a JSON ChatResponse on the served path.
    // Any non-success status reaching the Ok arm is unexpected; surface it as a
    // 502 in the Anthropic envelope rather than guessing at its body shape.
    if status != StatusCode::OK {
        warn!(
            status = %status,
            "unexpected non-200 status on the inner-core Ok path for /v1/messages"
        );
        return anthropic_error_response(
            StatusCode::BAD_GATEWAY,
            "api_error",
            "Upstream returned an unexpected response.".to_string(),
        );
    }

    let bytes = match axum::body::to_bytes(body, MAX_RESPONSE_BODY_BYTES).await {
        Ok(collected) => collected,
        Err(e) => {
            warn!(error = %e, "failed to buffer inner-core response body for /v1/messages");
            return anthropic_error_response(
                StatusCode::BAD_GATEWAY,
                "api_error",
                "Upstream response could not be read.".to_string(),
            );
        }
    };

    let chat_resp: ChatResponse = match serde_json::from_slice(&bytes) {
        Ok(resp) => resp,
        Err(e) => {
            // Fail closed: do NOT emit a partially-formed Anthropic body from a
            // response we could not parse.
            warn!(
                error = %e,
                "inner-core response was not a ChatResponse JSON body for /v1/messages"
            );
            return anthropic_error_response(
                StatusCode::BAD_GATEWAY,
                "api_error",
                "Upstream response had an unexpected shape.".to_string(),
            );
        }
    };

    let anthropic_resp = chat_response_to_anthropic(&chat_resp);
    let mut out = (StatusCode::OK, Json(anthropic_resp)).into_response();

    // Carry payment/audit headers from the inner response onto the Anthropic
    // response so a receipt / session token issued by the money path is not
    // dropped on this endpoint.
    for name in FORWARDED_RESPONSE_HEADERS {
        if let Some(value) = parts.headers.get(*name) {
            out.headers_mut()
                .insert(HeaderName::from_static(name), value.clone());
        }
    }

    out
}

/// Map a [`GatewayError`] from the shared core onto the right response shape.
///
/// - 402 challenges/invalid-payment keep the x402 body verbatim (so x402
///   clients and registry probes see the canonical challenge regardless of
///   endpoint).
/// - Every other error is re-wrapped into the Anthropic error envelope with the
///   same HTTP status the chat route would have returned.
async fn translate_error(err: GatewayError) -> Response {
    match err {
        // x402 challenge body — emit verbatim (identical to the chat route's
        // 402). This carries the canonical PAYMENT-REQUIRED header too.
        GatewayError::PaymentChallenge(_)
        | GatewayError::PaymentRequired
        | GatewayError::InvalidPayment(_) => err.into_response(),

        // Everything else → Anthropic error envelope. Reuse the chat route's
        // GatewayError::into_response to get the canonical status code, then
        // re-shape the body into the Anthropic envelope (preserving the safe,
        // already-sanitized message; GatewayError::into_response is the single
        // place that decides what is safe to surface to clients).
        other => {
            let canonical = other.into_response();
            reshape_to_anthropic_envelope(canonical).await
        }
    }
}

/// Re-shape an OpenAI-style `{"error":{"type","message"}}` error response into
/// the Anthropic envelope `{"type":"error","error":{"type","message"}}`,
/// preserving the HTTP status code.
///
/// Reusing `GatewayError::into_response` as the source keeps the
/// sanitize-before-surface rules (GHSA-cgqx-mg48-949v) in exactly one place —
/// this function only rewrites the JSON shape, never the message content or the
/// decision about what is safe to expose.
async fn reshape_to_anthropic_envelope(response: Response) -> Response {
    let status = response.status();
    // Buffer the OpenAI envelope body. This is the cold error path, so the
    // extra buffering is acceptable. On any failure we fall back to a generic
    // Anthropic error with the original status preserved.
    let bytes = match axum::body::to_bytes(response.into_body(), MAX_RESPONSE_BODY_BYTES).await {
        Ok(c) => c,
        Err(_) => {
            return anthropic_error_response(
                status,
                anthropic_error_type_for_status(status),
                "An error occurred.".to_string(),
            );
        }
    };
    // Pull `error.type` / `error.message` out of the OpenAI envelope when
    // present; otherwise use status-derived defaults.
    let (error_type, message) = match serde_json::from_slice::<serde_json::Value>(&bytes) {
        Ok(v) => {
            let t = v["error"]["type"]
                .as_str()
                .map(String::from)
                .unwrap_or_else(|| anthropic_error_type_for_status(status).to_string());
            let m = v["error"]["message"]
                .as_str()
                .map(String::from)
                .unwrap_or_else(|| "An error occurred.".to_string());
            (t, m)
        }
        Err(_) => (
            anthropic_error_type_for_status(status).to_string(),
            "An error occurred.".to_string(),
        ),
    };
    anthropic_error_response_owned(status, error_type, message)
}

/// Build an Anthropic error envelope response.
///
/// Wire shape: `{"type":"error","error":{"type":<error_type>,"message":<msg>}}`,
/// per the Anthropic Messages API error schema.
fn anthropic_error_response(status: StatusCode, error_type: &str, message: String) -> Response {
    anthropic_error_response_owned(status, error_type.to_string(), message)
}

fn anthropic_error_response_owned(
    status: StatusCode,
    error_type: String,
    message: String,
) -> Response {
    let body = json!({
        "type": "error",
        "error": {
            "type": error_type,
            "message": message,
        }
    });
    (status, Json(body)).into_response()
}

/// Map an HTTP status to a reasonable Anthropic error `type` string when the
/// underlying error does not carry one.
fn anthropic_error_type_for_status(status: StatusCode) -> &'static str {
    match status {
        StatusCode::BAD_REQUEST => "invalid_request_error",
        StatusCode::UNAUTHORIZED => "authentication_error",
        StatusCode::FORBIDDEN => "permission_error",
        StatusCode::NOT_FOUND => "not_found_error",
        StatusCode::UNSUPPORTED_MEDIA_TYPE => "invalid_request_error",
        StatusCode::TOO_MANY_REQUESTS => "rate_limit_error",
        StatusCode::SERVICE_UNAVAILABLE => "overloaded_error",
        _ => "api_error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use http_body_util::BodyExt;

    #[test]
    fn status_to_anthropic_error_type_maps_known_codes() {
        assert_eq!(
            anthropic_error_type_for_status(StatusCode::BAD_REQUEST),
            "invalid_request_error"
        );
        assert_eq!(
            anthropic_error_type_for_status(StatusCode::NOT_FOUND),
            "not_found_error"
        );
        assert_eq!(
            anthropic_error_type_for_status(StatusCode::TOO_MANY_REQUESTS),
            "rate_limit_error"
        );
        assert_eq!(
            anthropic_error_type_for_status(StatusCode::SERVICE_UNAVAILABLE),
            "overloaded_error"
        );
        assert_eq!(
            anthropic_error_type_for_status(StatusCode::INTERNAL_SERVER_ERROR),
            "api_error"
        );
    }

    #[tokio::test]
    async fn anthropic_error_response_has_expected_envelope() {
        let resp = anthropic_error_response(
            StatusCode::NOT_FOUND,
            "not_found_error",
            "model not found: foo".to_string(),
        );
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["type"], "error");
        assert_eq!(v["error"]["type"], "not_found_error");
        assert_eq!(v["error"]["message"], "model not found: foo");
    }

    /// A `ModelNotFound` GatewayError (a 404, NOT a 402) is re-shaped into the
    /// Anthropic envelope with the status preserved — proving non-402 errors do
    /// not leak the OpenAI `{"error":{...}}` shape.
    #[tokio::test]
    async fn non_402_gateway_error_becomes_anthropic_envelope() {
        let resp = translate_error(GatewayError::ModelNotFound("openai/nope".to_string())).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        // Anthropic envelope: top-level type:"error", nested error object.
        assert_eq!(v["type"], "error");
        assert_eq!(v["error"]["type"], "model_not_found");
        assert!(v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("openai/nope"));
    }

    /// An `InvalidPayment` GatewayError (a 402) keeps the x402/OpenAI shape
    /// verbatim — NOT the Anthropic envelope — so x402 clients see the
    /// canonical 402 regardless of endpoint.
    #[tokio::test]
    async fn invalid_payment_keeps_x402_body_shape() {
        let resp = translate_error(GatewayError::InvalidPayment("bad sig".to_string())).await;
        assert_eq!(resp.status(), StatusCode::PAYMENT_REQUIRED);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        // x402/OpenAI envelope, NOT the Anthropic top-level type:"error".
        assert!(v["type"].is_null(), "must not be the Anthropic envelope");
        assert_eq!(v["error"]["type"], "invalid_payment");
    }
}
