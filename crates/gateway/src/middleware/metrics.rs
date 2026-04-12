//! Prometheus metrics middleware.
//!
//! Records request duration, active request gauge, and request counter
//! for all routes except `/metrics` itself (to avoid feedback loops from
//! Prometheus scraping).

use std::time::Instant;

use axum::extract::{MatchedPath, Request};
use axum::middleware::Next;
use axum::response::Response;

/// Drop guard that decrements `solvela_active_requests` on drop, ensuring the
/// gauge is decremented even if the inner handler panics.
struct ActiveRequestGuard;

impl Drop for ActiveRequestGuard {
    fn drop(&mut self) {
        metrics::gauge!("solvela_active_requests").decrement(1.0);
    }
}

/// Normalize an HTTP method to a `&'static str` label.
///
/// Maps standard methods to their canonical string representation and
/// collapses uncommon methods into `"OTHER"` to prevent unbounded label
/// cardinality in Prometheus.
fn normalize_method(method: &axum::http::Method) -> &'static str {
    match *method {
        axum::http::Method::GET => "GET",
        axum::http::Method::POST => "POST",
        axum::http::Method::PUT => "PUT",
        axum::http::Method::DELETE => "DELETE",
        axum::http::Method::PATCH => "PATCH",
        axum::http::Method::HEAD => "HEAD",
        axum::http::Method::OPTIONS => "OPTIONS",
        _ => "OTHER",
    }
}

/// Axum middleware function that records Prometheus request metrics.
///
/// On each request:
/// 1. Increments `solvela_active_requests` gauge (with drop guard for safety)
/// 2. Runs the inner handler
/// 3. Decrements `solvela_active_requests` gauge (via drop guard)
/// 4. Records `solvela_request_duration_seconds` histogram (labels: method, path)
/// 5. Increments `solvela_requests_total` counter (labels: method, path, status)
///
/// Requests to `/metrics` are passed through without recording to prevent
/// the Prometheus scraper from inflating request counts.
pub async fn record_metrics(request: Request, next: Next) -> Response {
    let method = request.method().clone();
    let path = request
        .extensions()
        .get::<MatchedPath>()
        .map(|mp| mp.as_str().to_owned());

    // Skip recording for the /metrics endpoint itself
    let should_record = path.as_deref() != Some("/metrics");

    let _guard = if should_record {
        metrics::gauge!("solvela_active_requests").increment(1.0);
        Some(ActiveRequestGuard)
    } else {
        None
    };

    let start = Instant::now();
    let response = next.run(request).await;
    let duration = start.elapsed().as_secs_f64();

    if should_record {
        let method_str = normalize_method(&method);
        let path_str = path.unwrap_or_else(|| "unknown".to_owned());
        let status_str = response.status().as_u16().to_string();

        // Drop the guard before recording so the gauge is decremented first
        drop(_guard);

        metrics::histogram!("solvela_request_duration_seconds", "method" => method_str, "path" => path_str.clone())
            .record(duration);
        metrics::counter!("solvela_requests_total", "method" => method_str, "path" => path_str, "status" => status_str)
            .increment(1);
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify the middleware function signature is compatible with `axum::middleware::from_fn`.
    #[test]
    fn test_middleware_is_fn() {
        // This is a compile-time check — if record_metrics doesn't match the
        // expected signature, this test won't compile.
        let _: fn(Request, Next) -> _ = |req, next| record_metrics(req, next);
    }

    #[test]
    fn test_normalize_method_standard() {
        assert_eq!(normalize_method(&axum::http::Method::GET), "GET");
        assert_eq!(normalize_method(&axum::http::Method::POST), "POST");
        assert_eq!(normalize_method(&axum::http::Method::PUT), "PUT");
        assert_eq!(normalize_method(&axum::http::Method::DELETE), "DELETE");
        assert_eq!(normalize_method(&axum::http::Method::PATCH), "PATCH");
        assert_eq!(normalize_method(&axum::http::Method::HEAD), "HEAD");
        assert_eq!(normalize_method(&axum::http::Method::OPTIONS), "OPTIONS");
    }

    #[test]
    fn test_normalize_method_other() {
        assert_eq!(normalize_method(&axum::http::Method::TRACE), "OTHER");
        assert_eq!(normalize_method(&axum::http::Method::CONNECT), "OTHER");
    }
}
