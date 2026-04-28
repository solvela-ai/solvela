use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use tokio::sync::Mutex;
use tracing::warn;

/// Configuration for the rate limiter.
#[derive(Debug, Clone)]
pub struct RateLimitConfig {
    /// Maximum requests per window.
    pub max_requests: u32,
    /// Window duration.
    pub window: Duration,
    /// Maximum requests per window for unidentified clients that fall through
    /// to the shared "unknown" bucket. Should be much lower than `max_requests`
    /// to limit abuse surface when ConnectInfo is not configured.
    pub unknown_max_requests: u32,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            max_requests: 60,
            window: Duration::from_secs(60),
            unknown_max_requests: 10,
        }
    }
}

impl RateLimitConfig {
    /// Create a config with a custom max_requests value.
    /// Used for env var override during load testing.
    pub fn with_max_requests(max: u32) -> Self {
        Self {
            max_requests: max,
            ..Self::default()
        }
    }
}

/// Per-client rate limit state.
#[derive(Debug)]
struct RateLimitEntry {
    count: u32,
    window_start: Instant,
}

/// In-memory rate limiter (Phase 1).
///
/// Uses a simple fixed-window counter per client identifier.
/// Will be replaced by Redis-based rate limiting in Phase 2.
#[derive(Debug, Clone)]
pub struct RateLimiter {
    config: RateLimitConfig,
    entries: Arc<Mutex<HashMap<String, RateLimitEntry>>>,
    last_emergency_cleanup: Arc<tokio::sync::Mutex<Option<Instant>>>,
}

impl RateLimiter {
    pub fn new(config: RateLimitConfig) -> Self {
        Self {
            config,
            entries: Arc::new(Mutex::new(HashMap::new())),
            last_emergency_cleanup: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    /// Check if a request from the given client should be allowed.
    /// Returns `Ok(remaining)` or `Err(())` if rate limited.
    ///
    /// When `client_id` is `"unknown"`, applies the stricter
    /// `unknown_max_requests` limit instead of the normal `max_requests`.
    pub async fn check(&self, client_id: &str) -> Result<u32, ()> {
        let mut entries = self.entries.lock().await;

        // Emergency cleanup: if the map has grown too large, evict expired
        // entries — but only if at least 60 seconds have passed since the last
        // emergency cleanup to prevent cleanup storms under sustained load.
        if entries.len() > 100_000 {
            let mut last = self.last_emergency_cleanup.lock().await;
            let should_cleanup = last.is_none_or(|t| t.elapsed() >= Duration::from_secs(60));
            if should_cleanup {
                *last = Some(Instant::now());
                drop(last);
                let now = Instant::now();
                entries.retain(|_, entry| {
                    now.duration_since(entry.window_start) < self.config.window * 2
                });
            }
        }

        let now = Instant::now();

        let entry = entries
            .entry(client_id.to_string())
            .or_insert(RateLimitEntry {
                count: 0,
                window_start: now,
            });

        // Reset window if expired
        if now.duration_since(entry.window_start) >= self.config.window {
            entry.count = 0;
            entry.window_start = now;
        }

        entry.count += 1;

        let effective_limit = if client_id == "unknown" {
            self.config.unknown_max_requests
        } else {
            self.config.max_requests
        };

        if entry.count > effective_limit {
            Err(())
        } else {
            Ok(effective_limit - entry.count)
        }
    }

    /// Periodically clean up expired entries to prevent memory leaks.
    pub async fn cleanup(&self) {
        let mut entries = self.entries.lock().await;
        let now = Instant::now();
        entries.retain(|_, entry| now.duration_since(entry.window_start) < self.config.window * 2);
    }
}

/// Paths that are exempt from rate limiting.
///
/// These are operational/monitoring endpoints that must remain accessible even
/// when a client has exceeded its rate limit — otherwise health checks fail and
/// load balancers mistakenly mark the service as down.
const RATE_LIMIT_SKIP_PATHS: &[&str] = &["/health", "/v1/models", "/metrics"];

/// Rate limiting middleware.
///
/// Identifies clients by wallet address (from PAYMENT-SIGNATURE header)
/// or by IP address as fallback.
pub async fn rate_limit(request: Request, next: Next) -> Response {
    // Skip rate limiting for operational/monitoring endpoints (checked early,
    // before any Redis/memory lookup).
    if RATE_LIMIT_SKIP_PATHS.contains(&request.uri().path()) {
        return next.run(request).await;
    }

    // Extract client identifier: wallet from payment header, or IP fallback
    let client_id = extract_client_id(&request);

    // Check if we have a rate limiter in extensions
    if let Some(limiter) = request.extensions().get::<RateLimiter>() {
        let limiter = limiter.clone();
        match limiter.check(&client_id).await {
            Ok(remaining) => {
                let mut response = next.run(request).await;
                // Add standard rate limit headers
                let headers = response.headers_mut();
                if let Ok(val) = limiter.config.max_requests.to_string().parse() {
                    headers.insert("x-ratelimit-limit", val);
                }
                if let Ok(val) = remaining.to_string().parse() {
                    headers.insert("x-ratelimit-remaining", val);
                }
                if let Ok(val) = limiter.config.window.as_secs().to_string().parse() {
                    headers.insert("x-ratelimit-reset", val);
                }
                response
            }
            Err(()) => {
                warn!(client_id, "rate limit exceeded");
                let reset_secs = limiter.config.window.as_secs();
                let mut response = (
                    StatusCode::TOO_MANY_REQUESTS,
                    axum::Json(serde_json::json!({
                        "error": {
                            "type": "rate_limit_exceeded",
                            "message": "Too many requests. Please slow down.",
                        }
                    })),
                )
                    .into_response();
                let headers = response.headers_mut();
                if let Ok(limit_val) = limiter.config.max_requests.to_string().parse() {
                    headers.insert("x-ratelimit-limit", limit_val);
                }
                if let Ok(remaining_val) = "0".parse() {
                    headers.insert("x-ratelimit-remaining", remaining_val);
                }
                if let Ok(reset_val) = reset_secs.to_string().parse::<axum::http::HeaderValue>() {
                    headers.insert("x-ratelimit-reset", reset_val.clone());
                    headers.insert("retry-after", reset_val);
                }
                response
            }
        }
    } else {
        // No rate limiter configured, pass through
        next.run(request).await
    }
}

/// Extract a client identifier from the request.
///
/// Priority order (most to least trustworthy):
/// 1. Wallet address from the decoded PaymentInfo extension — cryptographically
///    bound to the payment; cannot be spoofed without a valid signed transaction.
/// 2. Peer socket address injected by Tower — reflects the actual TCP connection,
///    not a header, so cannot be forged by the client.
/// 3. `"unknown"` fallback — never `X-Forwarded-For`, which is trivially spoofed
///    and must not be used for security-sensitive client identification.
///
/// Note: If this gateway is deployed behind a trusted reverse proxy and IP-based
/// rate limiting is required, configure the proxy to strip and re-inject
/// `X-Forwarded-For` at the proxy layer — do not rely on client-supplied headers.
fn extract_client_id(request: &Request) -> String {
    // 1. Wallet address from verified payment — strongest identity signal
    if let Some(payment_info) = request
        .extensions()
        .get::<super::solvela_x402::PaymentInfo>()
    {
        return payment_info.payload.accepted.pay_to.clone();
    }

    // 2. Actual TCP peer address from Tower's ConnectInfo extension
    if let Some(addr) = request
        .extensions()
        .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
    {
        return addr.0.ip().to_string();
    }

    // 3. Unknown — rate limit slot shared by all unknown clients (conservative).
    //    This fallback is hit when ConnectInfo is not configured. When deployed
    //    behind a reverse proxy, configure `into_make_service_with_connect_info::<SocketAddr>()`
    //    on the Axum server so that each TCP peer gets its own rate-limit bucket.
    warn!(
        "rate limiter falling back to shared 'unknown' bucket — ConnectInfo not configured; \
           all unidentified clients share a single stricter rate limit"
    );
    "unknown".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config(max_requests: u32, window_ms: u64) -> RateLimitConfig {
        RateLimitConfig {
            max_requests,
            window: Duration::from_millis(window_ms),
            unknown_max_requests: 10,
        }
    }

    #[tokio::test]
    async fn test_allows_requests_within_limit() {
        let limiter = RateLimiter::new(test_config(5, 60_000));

        for i in 0..5 {
            let result = limiter.check("wallet-a").await;
            assert!(result.is_ok(), "request {i} should be allowed");
            assert_eq!(result.unwrap(), 5 - (i + 1));
        }
    }

    #[tokio::test]
    async fn test_blocks_excess_requests() {
        let limiter = RateLimiter::new(test_config(3, 60_000));

        // Use up the limit
        for _ in 0..3 {
            assert!(limiter.check("wallet-b").await.is_ok());
        }

        // The next request should be rejected
        assert!(limiter.check("wallet-b").await.is_err());
        assert!(limiter.check("wallet-b").await.is_err());
    }

    #[tokio::test]
    async fn test_window_reset_after_expiry() {
        let limiter = RateLimiter::new(test_config(2, 50));

        // Use up the limit
        assert!(limiter.check("wallet-c").await.is_ok());
        assert!(limiter.check("wallet-c").await.is_ok());
        assert!(limiter.check("wallet-c").await.is_err());

        // Wait for the window to expire
        tokio::time::sleep(Duration::from_millis(60)).await;

        // Should be allowed again
        let result = limiter.check("wallet-c").await;
        assert!(
            result.is_ok(),
            "request should be allowed after window reset"
        );
        assert_eq!(result.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_cleanup_removes_old_entries() {
        let limiter = RateLimiter::new(test_config(10, 50));

        // Create some entries
        assert!(limiter.check("wallet-d").await.is_ok());
        assert!(limiter.check("wallet-e").await.is_ok());

        // Wait for entries to expire (cleanup threshold is 2x window)
        tokio::time::sleep(Duration::from_millis(110)).await;

        limiter.cleanup().await;

        // Verify entries were removed by checking the internal state
        let entries = limiter.entries.lock().await;
        assert!(entries.is_empty(), "expired entries should be cleaned up");
    }

    #[tokio::test]
    async fn test_independent_clients() {
        let limiter = RateLimiter::new(test_config(2, 60_000));

        // Wallet A uses its limit
        assert!(limiter.check("wallet-a").await.is_ok());
        assert!(limiter.check("wallet-a").await.is_ok());
        assert!(limiter.check("wallet-a").await.is_err());

        // Wallet B should still have its full allowance
        assert!(limiter.check("wallet-b").await.is_ok());
        assert!(limiter.check("wallet-b").await.is_ok());
        assert!(limiter.check("wallet-b").await.is_err());
    }

    #[test]
    fn test_rate_limit_config_with_max_requests() {
        let config = RateLimitConfig::with_max_requests(10000);
        assert_eq!(config.max_requests, 10000);
        assert_eq!(config.unknown_max_requests, 10); // unchanged from default
        assert_eq!(config.window, Duration::from_secs(60)); // unchanged
    }

    #[test]
    fn test_skip_paths_contains_operational_endpoints() {
        assert!(
            RATE_LIMIT_SKIP_PATHS.contains(&"/health"),
            "health endpoint must be exempt from rate limiting"
        );
        assert!(
            RATE_LIMIT_SKIP_PATHS.contains(&"/v1/models"),
            "models endpoint must be exempt from rate limiting"
        );
        assert!(
            RATE_LIMIT_SKIP_PATHS.contains(&"/metrics"),
            "metrics endpoint must be exempt from rate limiting"
        );
    }

    #[tokio::test]
    async fn test_unknown_bucket_uses_stricter_limit() {
        let config = RateLimitConfig {
            max_requests: 60,
            window: Duration::from_secs(60),
            unknown_max_requests: 3,
        };
        let limiter = RateLimiter::new(config);

        // "unknown" should only get 3 requests, not 60
        assert!(limiter.check("unknown").await.is_ok());
        assert!(limiter.check("unknown").await.is_ok());
        assert!(limiter.check("unknown").await.is_ok());
        assert!(
            limiter.check("unknown").await.is_err(),
            "unknown bucket should be limited to unknown_max_requests"
        );

        // Named clients should still get the full 60
        for _ in 0..60 {
            assert!(limiter.check("wallet-x").await.is_ok());
        }
        assert!(limiter.check("wallet-x").await.is_err());
    }
}
