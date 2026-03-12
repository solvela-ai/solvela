//! Prometheus metrics middleware.
//!
//! Records request duration, active request gauge, and request counter
//! for all routes except `/metrics` itself (to avoid feedback loops from
//! Prometheus scraping).

use std::time::Instant;

use axum::extract::{MatchedPath, Request};
use axum::middleware::Next;
use axum::response::Response;

/// Axum middleware function that records Prometheus request metrics.
///
/// On each request:
/// 1. Increments `rcr_active_requests` gauge
/// 2. Runs the inner handler
/// 3. Decrements `rcr_active_requests` gauge
/// 4. Records `rcr_request_duration_seconds` histogram (labels: method, path)
/// 5. Increments `rcr_requests_total` counter (labels: method, path, status)
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

    if should_record {
        metrics::gauge!("rcr_active_requests").increment(1.0);
    }

    let start = Instant::now();
    let response = next.run(request).await;
    let duration = start.elapsed().as_secs_f64();

    if should_record {
        let method_str = method.as_str().to_owned();
        let path_str = path.unwrap_or_else(|| "unknown".to_owned());
        let status_str = response.status().as_u16().to_string();

        metrics::gauge!("rcr_active_requests").decrement(1.0);
        metrics::histogram!("rcr_request_duration_seconds", "method" => method_str.clone(), "path" => path_str.clone())
            .record(duration);
        metrics::counter!("rcr_requests_total", "method" => method_str, "path" => path_str, "status" => status_str)
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
}
