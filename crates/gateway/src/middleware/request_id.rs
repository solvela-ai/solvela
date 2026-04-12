//! Request ID middleware — generates or validates a unique ID for every request.
//!
//! The `X-Solvela-Request-Id` (and legacy `X-RCR-Request-Id`) response header
//! is **always** present (not gated by the debug flag). Clients can provide
//! their own ID via the `X-Request-Id` request header; if absent or invalid,
//! the gateway generates a UUID v4.

use axum::http::{HeaderName, HeaderValue, Request, Response};
use futures::future::BoxFuture;
use std::task::{Context, Poll};
use tower::{Layer, Service};
use uuid::Uuid;

/// Maximum length for a client-provided request ID.
const MAX_REQUEST_ID_LEN: usize = 128;

/// Response header name for the request ID (new prefix).
pub static REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-solvela-request-id");
/// Legacy response header name for the request ID.
static REQUEST_ID_HEADER_LEGACY: HeaderName = HeaderName::from_static("x-rcr-request-id");

/// Request header name for client-provided request IDs.
static CLIENT_REQUEST_ID_HEADER: HeaderName = HeaderName::from_static("x-request-id");

/// Newtype wrapper stored in request extensions so handlers can access the ID.
#[derive(Debug, Clone)]
pub struct RequestId(pub String);

/// Validate a client-provided request ID.
///
/// Must be ≤128 chars and contain only `[a-zA-Z0-9\-_]`.
fn is_valid_request_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_REQUEST_ID_LEN
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Tower layer that wraps services with [`RequestIdMiddleware`].
#[derive(Debug, Clone)]
pub struct RequestIdLayer;

impl<S> Layer<S> for RequestIdLayer {
    type Service = RequestIdMiddleware<S>;

    fn layer(&self, inner: S) -> Self::Service {
        RequestIdMiddleware { inner }
    }
}

/// Middleware service that attaches a request ID to every request/response.
#[derive(Debug, Clone)]
pub struct RequestIdMiddleware<S> {
    inner: S,
}

impl<S, ReqBody, ResBody> Service<Request<ReqBody>> for RequestIdMiddleware<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    ReqBody: Send + 'static,
    ResBody: Send + 'static,
{
    type Response = Response<ResBody>;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<ReqBody>) -> Self::Future {
        // Extract or generate request ID
        let request_id = req
            .headers()
            .get(&CLIENT_REQUEST_ID_HEADER)
            .and_then(|v| v.to_str().ok())
            .filter(|id| is_valid_request_id(id))
            .map(String::from)
            .unwrap_or_else(|| Uuid::new_v4().to_string());

        // Store in extensions for handlers to access
        req.extensions_mut().insert(RequestId(request_id.clone()));

        let mut inner = self.inner.clone();
        Box::pin(async move {
            let mut response = inner.call(req).await?;

            // Always attach both X-Solvela-Request-Id and X-RCR-Request-Id to response
            if let Ok(hv) = HeaderValue::from_str(&request_id) {
                response
                    .headers_mut()
                    .insert(REQUEST_ID_HEADER.clone(), hv.clone());
                response
                    .headers_mut()
                    .insert(REQUEST_ID_HEADER_LEGACY.clone(), hv);
            }

            Ok(response)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_request_ids() {
        assert!(is_valid_request_id("abc-123"));
        assert!(is_valid_request_id("ABC_def_456"));
        assert!(is_valid_request_id("a"));
        assert!(is_valid_request_id(&"a".repeat(128)));
    }

    #[test]
    fn test_invalid_request_ids() {
        assert!(!is_valid_request_id(""));
        assert!(!is_valid_request_id(&"a".repeat(129)));
        assert!(!is_valid_request_id("abc def")); // spaces
        assert!(!is_valid_request_id("abc/def")); // slash
        assert!(!is_valid_request_id("abc@def")); // special char
        assert!(!is_valid_request_id("abc\ndef")); // newline
    }
}
