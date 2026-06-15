use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    extract::{ConnectInfo, FromRequestParts, Request},
    http::request::Parts,
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use tokio::sync::Mutex;
use tracing::{debug, warn};

/// Infallible extractor for the TCP peer address, for use in route handlers that
/// need the actual connection IP (e.g. free-tier rate limiting) WITHOUT 500-ing
/// when `ConnectInfo` is absent.
///
/// axum 0.8's `ConnectInfo<T>` only implements `FromRequestParts` (not
/// `OptionalFromRequestParts`), so `Option<ConnectInfo<_>>` is not a valid
/// extractor and a bare `ConnectInfo` would reject every request that lacks the
/// extension — exactly the `oneshot` test path and any proxy-misconfig. This
/// wrapper reads the same extension the middleware reads
/// (`request.extensions().get::<ConnectInfo<SocketAddr>>()`) and yields `None`
/// instead of failing. Pair with [`connect_info_client_id`] for keying.
#[derive(Debug, Clone, Copy)]
pub struct PeerAddr(pub Option<SocketAddr>);

impl<S: Send + Sync> FromRequestParts<S> for PeerAddr {
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(PeerAddr(
            parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|ci| ci.0),
        ))
    }
}

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

    /// Default rate-limit budget for the anonymous **free-tier** bypass path.
    ///
    /// The zero-cost bypass serves requests with NO payer wallet, so abuse can
    /// only be bounded by client IP. This default is intentionally STRICTER than
    /// the paid `max_requests` (60/min): free requests get 5 per 60s window. The
    /// `unknown` bucket (no `ConnectInfo`) is stricter still, matching the
    /// default policy.
    ///
    /// Overridable via `SOLVELA_FREE_TIER_RATE_LIMIT` — see
    /// [`with_max_requests`](Self::with_max_requests).
    pub fn free_default() -> Self {
        Self {
            max_requests: 5,
            window: Duration::from_secs(60),
            unknown_max_requests: 2,
        }
    }

    /// Default rate-limit budget for the public, unauthenticated
    /// `GET /v1/receipts/{id}` route.
    ///
    /// Every receipts GET is a database query keyed by an unguessable UUID
    /// capability, so the threat is enumeration/scanning, not legitimate use:
    /// an agent fetches its own receipt once (maybe a handful of times across
    /// retries). 20 per 60s window per client IP is generous for real agents
    /// and hostile to scanners; it is STRICTER than the generic outer limiter
    /// (60/min) which still applies first. The `unknown` bucket (no
    /// `ConnectInfo`) is stricter still, matching the free-tier policy.
    pub fn receipts_default() -> Self {
        Self {
            max_requests: 20,
            window: Duration::from_secs(60),
            unknown_max_requests: 5,
        }
    }

    /// Default per-IP rate-limit budget for the public, unauthenticated
    /// `POST /v1/faucet/gas` gas-drip route (security review finding F6).
    ///
    /// The faucet is unauthenticated and the per-wallet DB primary key only
    /// stops repeat drips to ONE wallet — it does NOT stop mass enumeration: an
    /// attacker can mint unlimited fresh wallets, pre-fund each past the cheap
    /// USDC floor, and drain the entire daily cap in a single-IP burst. The
    /// daily cap is the only throttle without this per-IP cap. The legitimate
    /// caller (the MCP wallet auto-create flow) hits the faucet at most once per
    /// wallet per device lifetime, so a TIGHT cap is safe: 3 drips per IP per
    /// 24h is generous for a real device (initial wallet + a couple of retries)
    /// and hostile to a single-IP enumeration burst. The `unknown` bucket (no
    /// `ConnectInfo`) is stricter still (1), matching the receipts/free policy.
    ///
    /// Like [`receipts_default`](Self::receipts_default) this is deliberately
    /// NOT env-tunable — the cap only needs to bound enumeration throughput;
    /// tune it here in code if the legitimate-caller pattern ever changes.
    pub fn faucet_default() -> Self {
        Self {
            max_requests: 3,
            window: Duration::from_secs(24 * 60 * 60),
            unknown_max_requests: 1,
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

    /// Read-only access to this limiter's config.
    ///
    /// Used by the free-tier in-handler path to build a matching 429 response
    /// (limit/window headers) via [`rate_limited_response`].
    pub fn config(&self) -> &RateLimitConfig {
        &self.config
    }

    /// Check if a request from the given client should be allowed.
    /// Returns `Ok(remaining)` or `Err(())` if rate limited.
    ///
    /// When `client_id` is `"unknown"`, applies the stricter
    /// `unknown_max_requests` limit instead of the normal `max_requests`.
    pub async fn check(&self, client_id: &str) -> Result<u32, ()> {
        // Decide whether emergency cleanup is needed under a SHORT lock
        // on `entries`. Previously this function held `entries` for its
        // entire body — including across the nested
        // `last_emergency_cleanup.lock().await` and the O(N) `retain` —
        // so under DDoS (the exact moment `len > 100k` triggers) every
        // other request blocked on `entries` for the full duration. The
        // cleanup is now out-of-band and takes locks one at a time.
        let needs_cleanup = {
            let entries = self.entries.lock().await;
            entries.len() > 100_000
        };
        if needs_cleanup {
            self.maybe_emergency_cleanup().await;
        }

        let mut entries = self.entries.lock().await;
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

    /// Emergency cleanup path used when `entries.len() > 100_000`.
    ///
    /// Acquires `last_emergency_cleanup` first (cheap; deflects redundant
    /// callers when the gate fired in the last 60s), drops it, then
    /// re-acquires `entries` only for the retain. Each lock is held for
    /// a single short critical section — never across an `.await` for
    /// another lock — so the hot path's brief `entries` read at the top
    /// of `check()` doesn't queue behind the cleanup.
    async fn maybe_emergency_cleanup(&self) {
        // Cleanup-gate check + claim under its own short lock.
        {
            let mut last = self.last_emergency_cleanup.lock().await;
            if last.is_some_and(|t| t.elapsed() < Duration::from_secs(60)) {
                return; // Recent cleanup; skip.
            }
            *last = Some(Instant::now());
        }

        // Now perform the retain under a fresh `entries` lock.
        let mut entries = self.entries.lock().await;
        let now = Instant::now();
        entries.retain(|_, entry| now.duration_since(entry.window_start) < self.config.window * 2);
    }
}

/// Default aggregate (global, all-clients-combined) free-tier requests-per-minute
/// cap.
///
/// This MUST stay BELOW the upstream provider's SHARED free-tier ceiling. The
/// free model is served on Google's free Gemini API tier, whose hard limit is
/// ~15 requests/min SHARED across the whole gateway API key. The per-IP free
/// limiter ([`RateLimitConfig::free_default`]) cannot protect that shared
/// ceiling — many distinct IPs each under their per-IP cap can still collectively
/// exceed 15 RPM, making Google 429 the gateway and surfacing as errors for ALL
/// free users. 12 leaves headroom below 15 so the gateway returns its OWN clean
/// 429 before Google's leaks through. Override via `SOLVELA_FREE_TIER_GLOBAL_RPM`.
pub const FREE_TIER_GLOBAL_RPM_DEFAULT: u32 = 12;

/// In-memory fixed-window state for the aggregate free-tier cap.
///
/// A single shared counter (NOT per-client): `(epoch_minute, count)`. Used as
/// the backing store when Redis is absent, AND as the degradation fallback when
/// a configured Redis errors at runtime. Per-instance only — acceptable for
/// single-instance/dev; multi-instance deployments rely on the Redis path so the
/// counter is shared across instances (see [`FreeTierGlobalCap::check`]).
#[derive(Debug, Default)]
struct GlobalWindow {
    epoch_minute: u64,
    count: u64,
}

/// Aggregate (global) free-tier rate cap — total free-model throughput across
/// ALL clients per 1-minute fixed window, kept below the upstream provider's
/// shared ceiling.
///
/// Distinct from [`RateLimiter`], which is per-client. This caps the COMBINED
/// free traffic so the gateway 429s before the upstream provider does. Backing
/// store is Redis when configured (cross-instance: every instance increments one
/// shared key), degrading to an in-memory per-instance counter when Redis is
/// absent or errors at runtime.
#[derive(Debug, Clone)]
pub struct FreeTierGlobalCap {
    /// Max free requests allowed across all clients per window.
    cap: u32,
    /// In-memory fallback counter (Redis-absent and Redis-error paths).
    window: Arc<std::sync::Mutex<GlobalWindow>>,
}

impl FreeTierGlobalCap {
    /// Window length in seconds. Fixed 1-minute window to match the provider's
    /// per-minute (RPM) ceiling and the `retry-after` advertised on rejection.
    pub const WINDOW_SECS: u64 = 60;

    /// Create a cap with the given per-minute limit.
    pub fn new(cap: u32) -> Self {
        Self {
            cap,
            window: Arc::new(std::sync::Mutex::new(GlobalWindow::default())),
        }
    }

    /// The configured per-minute cap.
    pub fn cap(&self) -> u32 {
        self.cap
    }

    /// Current wall-clock epoch minute (UTC). The fixed-window bucket id shared
    /// by Redis and the in-memory fallback so both agree on window boundaries.
    fn epoch_minute() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() / Self::WINDOW_SECS)
            .unwrap_or(0)
    }

    /// Increment the in-memory fixed-window counter for `minute` and return the
    /// post-increment count. Resets the window when the minute rolls over.
    ///
    /// GHSA-wc9q-wc6q-gwmq pattern: recover from a poisoned lock rather than
    /// panicking, so one panicking task can't wedge the free path for everyone.
    fn incr_in_memory(&self, minute: u64) -> u64 {
        let mut w = self
            .window
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if w.epoch_minute != minute {
            w.epoch_minute = minute;
            w.count = 0;
        }
        w.count += 1;
        w.count
    }

    /// Check (and consume one slot of) the aggregate free-tier budget.
    ///
    /// Returns `Ok(())` when the request is under the cap, `Err(())` when the
    /// aggregate window is exhausted (caller emits the shared 429).
    ///
    /// Store selection:
    /// - `cache = Some` (Redis configured): increment the shared Redis window
    ///   key so the counter is cross-instance. On a Redis ERROR, DEGRADE to the
    ///   in-memory per-instance counter and `warn!` — an infra blip must not
    ///   hard-fail free traffic (fail-open to in-memory, NOT fail-open to
    ///   unbounded), and the provider's own 429 is the ultimate backstop.
    /// - `cache = None` (no Redis): use the in-memory per-instance counter.
    pub async fn check(&self, cache: Option<&crate::cache::ResponseCache>) -> Result<(), ()> {
        let minute = Self::epoch_minute();

        let count = match cache {
            Some(c) => match c.incr_global_free_window(minute).await {
                Ok(n) => n,
                Err(e) => {
                    // Degrade to in-memory rather than failing the request. We do
                    // NOT go unbounded: the in-memory counter still bounds this
                    // instance for the current window.
                    warn!(
                        error = %e,
                        "free-tier aggregate cap: Redis INCR failed — degrading to in-memory counter for this request"
                    );
                    self.incr_in_memory(minute)
                }
            },
            None => self.incr_in_memory(minute),
        };

        if count > self.cap as u64 {
            Err(())
        } else {
            Ok(())
        }
    }

    /// Build a [`RateLimitConfig`] describing this cap, so the shared
    /// [`rate_limited_response`] emits a correct `x-ratelimit-limit` /
    /// `retry-after` for the 1-minute aggregate window.
    pub fn as_rate_limit_config(&self) -> RateLimitConfig {
        RateLimitConfig {
            max_requests: self.cap,
            window: Duration::from_secs(Self::WINDOW_SECS),
            unknown_max_requests: self.cap,
        }
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
                let response = next.run(request).await;
                // Do NOT overwrite rate-limit headers when the downstream handler
                // already returned a 429. An in-handler free-tier limiter
                // (built by `rate_limited_response`) produces its own
                // authoritative `x-ratelimit-*` headers reflecting the FREE limit
                // (e.g. 5/min). The outer limiter's success arm fired only
                // because this request was under the (looser, e.g. 60/min) OUTER
                // cap; re-inserting those values here would tell the client it
                // has "40 remaining" alongside a 429, which would make it ignore
                // `retry-after` and hammer. Preserve the inner 429's headers.
                if response.status() == StatusCode::TOO_MANY_REQUESTS {
                    return response;
                }
                // Add standard rate limit headers (non-429 responses only).
                let mut response = response;
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
                rate_limited_response(&limiter.config)
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
/// 1. **Payer wallet** extracted from the signed transaction inside `PaymentInfo` —
///    derived from the on-chain signer (escrow `agent_pubkey` or the first
///    account key of a direct-transfer transaction). Cannot be spoofed without
///    a valid signed transaction.
/// 2. Peer socket address injected by Tower — reflects the actual TCP connection,
///    not a header, so cannot be forged by the client.
/// 3. `"unknown"` fallback — never `X-Forwarded-For`, which is trivially spoofed
///    and must not be used for security-sensitive client identification.
///
/// **GHSA-6ggq-cvwx-4f67 fix**: This used to key on `payment_info.payload.accepted.pay_to`,
/// which is a *client-supplied* field set to the gateway's own recipient wallet. An
/// attacker could vary `pay_to` per request to escape into a fresh per-client bucket
/// and bypass rate limiting from a single IP. The payer wallet is derived from the
/// signed transaction itself and is therefore bound to the cryptographic identity
/// of the request author.
///
/// Note: If this gateway is deployed behind a trusted reverse proxy and IP-based
/// rate limiting is required, configure the proxy to strip and re-inject
/// `X-Forwarded-For` at the proxy layer — do not rely on client-supplied headers.
fn extract_client_id(request: &Request) -> String {
    // 1. Payer wallet derived from the signed transaction — strongest identity signal
    if let Some(payment_info) = request.extensions().get::<super::x402::PaymentInfo>() {
        let payer = crate::payment_util::extract_payer_wallet(&payment_info.payload);
        // `extract_payer_wallet` returns "unknown" when the transaction can't be
        // decoded; treat that the same as any other unidentified client by falling
        // through to ConnectInfo / unknown bucket. Otherwise the literal "unknown"
        // string would itself become a shared bucket.
        if payer != "unknown" {
            return payer;
        }
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

/// Derive a rate-limit client id for the **anonymous free-tier path** from the
/// Tower-injected peer socket address.
///
/// The free bypass has no payer wallet, so the only honest identity is the
/// actual TCP peer IP (from `ConnectInfo`). When `ConnectInfo` is absent
/// (`addr == None`, e.g. behind a misconfigured proxy or in `oneshot` tests),
/// fall back to the shared stricter `"unknown"` bucket.
///
/// **GHSA-6ggq-cvwx-4f67 invariant:** this NEVER keys on a client-supplied
/// header (`X-Forwarded-For` et al.) — only the cryptographically-/transport-
/// grounded peer address — so a free client cannot spoof its way into a fresh
/// bucket. Mirrors steps 2–3 of [`extract_client_id`].
pub fn connect_info_client_id(addr: Option<std::net::SocketAddr>) -> String {
    match addr {
        Some(a) => a.ip().to_string(),
        None => {
            // FINDING 5: this used to `warn!` on EVERY ConnectInfo-absent request,
            // spamming logs in a misconfigured deploy. Downgraded to `debug!`; the
            // operator-facing signal is the `solvela_free_tier_unknown_bucket_total`
            // metric emitted by the free path (Finding 2) and the one-time startup
            // assertion in main.rs — both far lower noise than a per-request warn.
            debug!(
                "free-tier rate limiter falling back to shared 'unknown' bucket — ConnectInfo \
                 not configured; all unidentified free clients share a single stricter limit"
            );
            "unknown".to_string()
        }
    }
}

/// Build the canonical 429 Too Many Requests response.
///
/// Single source of truth for the rate-limit-exceeded body + `x-ratelimit-*` /
/// `retry-after` headers, shared by the global rate-limit middleware and the
/// in-handler free-tier limiter so both emit byte-identical 429s. `remaining`
/// is always `0` here (the request was rejected).
///
/// These headers are **authoritative** on the wire: the outer `rate_limit`
/// middleware now preserves any 429 response's headers unchanged (it returns
/// early on `StatusCode::TOO_MANY_REQUESTS` instead of re-inserting its own
/// looser limit/remaining values), so a free-tier 429 built here reaches the
/// client carrying the correct FREE limit and `remaining == 0`.
pub fn rate_limited_response(config: &RateLimitConfig) -> Response {
    let reset_secs = config.window.as_secs();
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
    if let Ok(limit_val) = config.max_requests.to_string().parse() {
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
    fn test_free_default_is_stricter_than_paid() {
        let free = RateLimitConfig::free_default();
        let paid = RateLimitConfig::default();
        assert!(
            free.max_requests < paid.max_requests,
            "free-tier limit ({}) must be stricter than the paid limit ({})",
            free.max_requests,
            paid.max_requests
        );
        assert_eq!(free.max_requests, 5);
        assert_eq!(free.unknown_max_requests, 2);
        assert_eq!(free.window, Duration::from_secs(60));
    }

    #[test]
    fn test_receipts_default_is_stricter_than_paid() {
        let receipts = RateLimitConfig::receipts_default();
        let paid = RateLimitConfig::default();
        assert!(
            receipts.max_requests < paid.max_requests,
            "receipts per-IP limit ({}) must be stricter than the generic outer limit ({})",
            receipts.max_requests,
            paid.max_requests
        );
        assert_eq!(receipts.max_requests, 20);
        assert_eq!(receipts.unknown_max_requests, 5);
        assert_eq!(receipts.window, Duration::from_secs(60));
    }

    #[test]
    fn test_faucet_default_is_stricter_than_paid() {
        let faucet = RateLimitConfig::faucet_default();
        let paid = RateLimitConfig::default();
        assert!(
            faucet.max_requests < paid.max_requests,
            "faucet per-IP limit ({}) must be stricter than the generic outer limit ({})",
            faucet.max_requests,
            paid.max_requests
        );
        assert_eq!(faucet.max_requests, 3);
        assert_eq!(faucet.unknown_max_requests, 1);
        // 24h window: the legitimate caller hits the faucet once per wallet per
        // device lifetime, so the throttle is over a day, not a minute.
        assert_eq!(faucet.window, Duration::from_secs(24 * 60 * 60));
    }

    #[test]
    fn test_connect_info_client_id_uses_peer_ip() {
        let addr: std::net::SocketAddr = "203.0.113.7:54321".parse().unwrap();
        // The PORT must be dropped — keying must be per-IP, not per-connection,
        // or each new TCP connection would escape into a fresh bucket.
        assert_eq!(connect_info_client_id(Some(addr)), "203.0.113.7");
    }

    #[test]
    fn test_connect_info_client_id_unknown_when_absent() {
        // No ConnectInfo (e.g. oneshot tests / misconfigured proxy) → the shared
        // stricter "unknown" bucket, never a per-request-spoofable value.
        assert_eq!(connect_info_client_id(None), "unknown");
    }

    #[tokio::test]
    async fn test_rate_limited_response_shape_and_headers() {
        let config = RateLimitConfig::free_default();
        let response = rate_limited_response(&config);
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        let headers = response.headers();
        assert_eq!(headers.get("x-ratelimit-limit").unwrap(), "5");
        assert_eq!(headers.get("x-ratelimit-remaining").unwrap(), "0");
        assert_eq!(headers.get("x-ratelimit-reset").unwrap(), "60");
        assert_eq!(headers.get("retry-after").unwrap(), "60");

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["error"]["type"], "rate_limit_exceeded");
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
    async fn test_free_global_cap_blocks_after_cap_in_memory() {
        // No Redis → in-memory counter. Cap of 3: first 3 OK, 4th rejected.
        let cap = FreeTierGlobalCap::new(3);
        assert!(cap.check(None).await.is_ok(), "1st under cap");
        assert!(cap.check(None).await.is_ok(), "2nd under cap");
        assert!(cap.check(None).await.is_ok(), "3rd at cap");
        assert!(
            cap.check(None).await.is_err(),
            "4th must be rejected — aggregate cap exceeded"
        );
        // Still rejected on subsequent calls within the same window.
        assert!(cap.check(None).await.is_err());
    }

    #[test]
    fn test_free_global_cap_default_below_google_ceiling() {
        // The aggregate default MUST stay strictly below Google's shared 15 RPM
        // free-tier ceiling, or the gateway leaks Google's 429 instead of its own.
        assert_eq!(FREE_TIER_GLOBAL_RPM_DEFAULT, 12);
        // Compile-time guard: a future bump to >= 15 fails the build, not just a
        // test run. (clippy::assertions_on_constants wants the const block form.)
        const _: () = assert!(
            FREE_TIER_GLOBAL_RPM_DEFAULT < 15,
            "aggregate default must stay below Google's 15 RPM shared ceiling"
        );
    }

    #[tokio::test]
    async fn test_free_global_cap_window_resets() {
        // Drive the in-memory counter directly across a window rollover: an
        // exhausted minute must not bleed into the next minute's budget.
        let cap = FreeTierGlobalCap::new(2);
        assert_eq!(cap.incr_in_memory(100), 1);
        assert_eq!(cap.incr_in_memory(100), 2);
        assert_eq!(cap.incr_in_memory(100), 3); // over cap (count > 2)
                                                // New minute → counter resets to 1.
        assert_eq!(
            cap.incr_in_memory(101),
            1,
            "window rollover must reset the aggregate counter"
        );
    }

    #[tokio::test]
    async fn test_free_global_cap_degrades_to_in_memory_on_redis_error() {
        // A configured-but-unreachable Redis must NOT 500 the request: the cap
        // degrades to its in-memory counter and still bounds traffic. Point at a
        // closed port so every INCR errors, then assert the in-memory counter is
        // what enforces the cap (cap=2 → 3rd rejected).
        let cache = crate::cache::ResponseCache::new(
            "redis://127.0.0.1:1",
            crate::cache::CacheConfig::default(),
        )
        .expect("client open does not connect");
        let cap = FreeTierGlobalCap::new(2);

        assert!(
            cap.check(Some(&cache)).await.is_ok(),
            "1st must serve via in-memory degradation, not error"
        );
        assert!(cap.check(Some(&cache)).await.is_ok(), "2nd at cap");
        assert!(
            cap.check(Some(&cache)).await.is_err(),
            "3rd must be rejected by the in-memory fallback counter — degradation is bounded, not unbounded"
        );
    }

    #[test]
    fn test_free_global_cap_as_rate_limit_config() {
        let cap = FreeTierGlobalCap::new(12);
        let cfg = cap.as_rate_limit_config();
        assert_eq!(cfg.max_requests, 12);
        assert_eq!(cfg.window, Duration::from_secs(60));
        // The shared 429 builder must advertise a 60s retry-after for the
        // 1-minute aggregate window.
        let resp = rate_limited_response(&cfg);
        assert_eq!(resp.headers().get("x-ratelimit-limit").unwrap(), "12");
        assert_eq!(resp.headers().get("retry-after").unwrap(), "60");
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
