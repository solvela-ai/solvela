use std::sync::Arc;

use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::Response;
use axum::Router;
use tower_http::limit::RequestBodyLimitLayer;
use tracing::{debug, error, info, warn};

/// Error types that represent client-side validation failures (not server errors).
const CLIENT_VALIDATION_ERRORS: &[&str] = &[
    "amount_exceeds_max",
    "recipient_mismatch",
    "no_compatible_scheme",
];

use solvela_client::SolvelaClient;
use solvela_protocol::PaymentRequired;

/// Shared state for the proxy handlers.
pub struct ProxyState {
    pub client: SolvelaClient,
    pub gateway_url: String,
    pub http: reqwest::Client,
}

/// Maximum request body size (10 MB — matches gateway limit).
const MAX_BODY_SIZE: usize = 10 * 1024 * 1024;

/// Build the Axum router for the proxy.
pub fn build_proxy_router(state: Arc<ProxyState>) -> Router {
    Router::new()
        .fallback(proxy_handler)
        .layer(RequestBodyLimitLayer::new(MAX_BODY_SIZE))
        .with_state(state)
}

/// Catch-all proxy handler.
///
/// Forwards any request to the gateway. If the gateway returns 402,
/// signs a USDC-SPL payment and retries with the PAYMENT-SIGNATURE header.
/// All other responses (including SSE streams) are piped through unmodified.
async fn proxy_handler(
    State(state): State<Arc<ProxyState>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let path = uri.path_and_query().map_or("/", |pq| pq.as_str());
    let gateway_url = format!("{}{}", state.gateway_url, path);

    debug!(method = %method, path = %path, "proxying request");

    // Build the forwarded request
    let mut req_builder = state.http.request(reqwest_method(&method), &gateway_url);
    req_builder = forward_headers(req_builder, &headers);

    // Attach body for methods that have one
    if method == Method::POST || method == Method::PUT || method == Method::PATCH {
        req_builder = req_builder.body(body.clone());
    }

    // Send to gateway
    let gateway_resp = match req_builder.send().await {
        Ok(r) => r,
        Err(e) => {
            if e.is_timeout() {
                return proxy_error_response(
                    StatusCode::GATEWAY_TIMEOUT,
                    "gateway_timeout",
                    &format!("gateway request timed out: {e}"),
                    None,
                );
            }
            return proxy_error_response(
                StatusCode::BAD_GATEWAY,
                "gateway_unreachable",
                &format!("failed to reach gateway: {e}"),
                None,
            );
        }
    };

    let resp_status = gateway_resp.status();

    // If 402, attempt payment signing and retry
    if resp_status == reqwest::StatusCode::PAYMENT_REQUIRED {
        return handle_402(&state, &gateway_url, &method, &headers, &body, gateway_resp).await;
    }

    // Non-402: pipe through response as-is
    build_passthrough_response(gateway_resp)
}

/// Handle a 402 response: parse `PaymentRequired`, sign payment, retry.
async fn handle_402(
    state: &ProxyState,
    gateway_url: &str,
    method: &Method,
    headers: &HeaderMap,
    body: &axum::body::Bytes,
    resp_402: reqwest::Response,
) -> Response {
    // Parse the 402 body as PaymentRequired
    let resp_body = match resp_402.text().await {
        Ok(b) => b,
        Err(e) => {
            return proxy_error_response(
                StatusCode::BAD_GATEWAY,
                "invalid_402_body",
                &format!("failed to read 402 response body: {e}"),
                None,
            );
        }
    };

    let payment_required: PaymentRequired = match serde_json::from_str(&resp_body) {
        Ok(pr) => pr,
        Err(e) => {
            return proxy_error_response(
                StatusCode::BAD_GATEWAY,
                "invalid_402_body",
                &format!("failed to parse 402 body as PaymentRequired: {e}"),
                None,
            );
        }
    };

    info!(
        total = %payment_required.cost_breakdown.total,
        currency = %payment_required.cost_breakdown.currency,
        "402 received — signing payment"
    );

    // Sign the payment
    let payment_header = match state.client.sign_payment_for_402(&payment_required).await {
        Ok(h) => h,
        Err(e) => {
            return client_error_to_response(&e);
        }
    };

    // Retry the request with the PAYMENT-SIGNATURE header
    let mut retry_builder = state.http.request(reqwest_method(method), gateway_url);
    retry_builder = forward_headers(retry_builder, headers);
    retry_builder = retry_builder.header("PAYMENT-SIGNATURE", &payment_header);

    // Attach body
    if *method == Method::POST || *method == Method::PUT || *method == Method::PATCH {
        retry_builder = retry_builder.body(body.clone());
    }

    match retry_builder.send().await {
        Ok(resp) => {
            let status = resp.status();
            if status == reqwest::StatusCode::PAYMENT_REQUIRED {
                // Payment was rejected
                let body = resp.text().await.unwrap_or_default();
                proxy_error_response(
                    StatusCode::BAD_GATEWAY,
                    "payment_rejected",
                    &format!("gateway rejected payment: {body}"),
                    None,
                )
            } else {
                info!(status = %status, "paid request completed");
                build_passthrough_response(resp)
            }
        }
        Err(e) => proxy_error_response(
            StatusCode::BAD_GATEWAY,
            "gateway_unreachable",
            &format!("failed to send paid request: {e}"),
            None,
        ),
    }
}

/// Forward caller headers, stripping hop-by-hop and security-sensitive headers.
fn forward_headers(
    mut builder: reqwest::RequestBuilder,
    headers: &HeaderMap,
) -> reqwest::RequestBuilder {
    for (name, value) in headers {
        let name_lower = name.as_str().to_lowercase();
        if matches!(
            name_lower.as_str(),
            "host" | "connection" | "transfer-encoding" | "payment-signature"
        ) {
            if name_lower == "payment-signature" {
                warn!("stripped PAYMENT-SIGNATURE header from caller (security)");
            }
            continue;
        }
        if let Ok(v) = value.to_str() {
            builder = builder.header(name.as_str(), v);
        }
    }
    builder
}

/// Convert a reqwest response into an Axum response, preserving headers and
/// streaming the body through without buffering.
fn build_passthrough_response(resp: reqwest::Response) -> Response {
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let resp_headers = resp.headers().clone();

    // Stream the body through without buffering
    let body = Body::from_stream(resp.bytes_stream());

    let mut response = Response::builder()
        .status(status)
        .body(body)
        .unwrap_or_else(|_| {
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::empty())
                .expect("fallback response must build")
        });

    // Copy response headers (skip hop-by-hop)
    for (name, value) in &resp_headers {
        let name_lower = name.as_str().to_lowercase();
        if matches!(name_lower.as_str(), "connection" | "transfer-encoding") {
            continue;
        }
        response.headers_mut().insert(name.clone(), value.clone());
    }

    response
}

/// Map a `ClientError` to an appropriate proxy error response.
fn client_error_to_response(err: &solvela_client::ClientError) -> Response {
    use solvela_client::ClientError;

    match err {
        ClientError::NoCompatibleScheme => proxy_error_response(
            StatusCode::BAD_GATEWAY,
            "no_compatible_scheme",
            &err.to_string(),
            None,
        ),
        ClientError::RecipientMismatch { .. } => proxy_error_response(
            StatusCode::BAD_GATEWAY,
            "recipient_mismatch",
            &err.to_string(),
            None,
        ),
        ClientError::AmountExceedsMax { .. } => proxy_error_response(
            StatusCode::BAD_GATEWAY,
            "amount_exceeds_max",
            &err.to_string(),
            None,
        ),
        ClientError::Signing(_) => proxy_error_response(
            StatusCode::BAD_GATEWAY,
            "signing_error",
            &err.to_string(),
            None,
        ),
        _ => proxy_error_response(
            StatusCode::BAD_GATEWAY,
            "proxy_error",
            &err.to_string(),
            None,
        ),
    }
}

/// Build a structured JSON error response.
fn proxy_error_response(
    status: StatusCode,
    error_type: &str,
    message: &str,
    details: Option<serde_json::Value>,
) -> Response {
    if CLIENT_VALIDATION_ERRORS.contains(&error_type) {
        warn!(error_type = %error_type, message = %message, "proxy validation error");
    } else {
        error!(error_type = %error_type, message = %message, "proxy error");
    }

    let mut body = serde_json::json!({
        "error": {
            "type": error_type,
            "message": message,
        }
    });

    if let Some(d) = details {
        body["error"]["details"] = d;
    }

    let json = serde_json::to_string(&body).unwrap_or_else(|_| {
        r#"{"error":{"type":"internal","message":"serialization failed"}}"#.to_string()
    });

    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Body::from(json))
        .unwrap_or_else(|_| {
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(Body::empty())
                .expect("fallback response must build")
        })
}

/// Convert an `axum::http::Method` to a `reqwest::Method`.
fn reqwest_method(method: &Method) -> reqwest::Method {
    match *method {
        Method::GET => reqwest::Method::GET,
        Method::POST => reqwest::Method::POST,
        Method::PUT => reqwest::Method::PUT,
        Method::DELETE => reqwest::Method::DELETE,
        Method::PATCH => reqwest::Method::PATCH,
        Method::HEAD => reqwest::Method::HEAD,
        Method::OPTIONS => reqwest::Method::OPTIONS,
        _ => {
            warn!(method = %method, "unsupported HTTP method, forwarding as-is");
            reqwest::Method::from_bytes(method.as_str().as_bytes()).unwrap_or(reqwest::Method::GET)
        }
    }
}
