//! Solvela gateway — Axum HTTP server for AI agent LLM payments.
//!
//! This module exposes the gateway internals for integration testing.
//! The binary entry point is in `main.rs`.

pub mod a2a;
pub mod audit;
pub mod balance_monitor;
pub mod cache;
pub mod config;
pub mod error;
pub mod middleware;
pub mod orgs;
pub mod payment_util;
pub mod providers;
pub mod routes;
pub mod security;
pub mod service_health;
pub mod services;
pub mod session;
pub mod usage;

use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::http::{HeaderName, HeaderValue, Method};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;
use lru::LruCache;
use tokio::sync::RwLock;
use tower::limit::ConcurrencyLimitLayer;
use tower_http::catch_panic::CatchPanicLayer;
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::services::ServiceRegistry;
use solvela_router::models::ModelRegistry;
use solvela_x402::facilitator::Facilitator;

use crate::middleware::rate_limit::RateLimiter;
use crate::middleware::request_id::RequestIdLayer;
use crate::providers::ProviderRegistry;
use crate::routes::escrow::SlotCache;

/// Shared application state passed to all route handlers.
pub struct AppState {
    pub config: config::AppConfig,
    pub model_registry: ModelRegistry,
    pub service_registry: RwLock<ServiceRegistry>,
    pub providers: ProviderRegistry,
    pub facilitator: Facilitator,
    pub usage: usage::UsageTracker,
    pub cache: Option<cache::ResponseCache>,
    pub provider_health: providers::health::ProviderHealthTracker,
    pub escrow_claimer: Option<Arc<solvela_x402::escrow::EscrowClaimer>>,
    /// Hot wallet pool for fee payer rotation. `None` when no fee payer keys are configured.
    pub fee_payer_pool: Option<Arc<solvela_x402::fee_payer::FeePayerPool>>,
    /// Durable nonce account pool. `None` when no nonce accounts are configured.
    pub nonce_pool: Option<Arc<solvela_x402::nonce_pool::NoncePool>>,
    /// Optional PostgreSQL pool for durable claim queue and other DB operations.
    pub db_pool: Option<sqlx::PgPool>,
    /// HMAC secret for signing/verifying session tokens.
    pub session_secret: Vec<u8>,
    /// In-memory replay protection fallback used when Redis (`cache`) is absent.
    /// LRU-bounded to 10,000 entries with 120-second TTL so stale signatures
    /// are treated as expired and the oldest entries are evicted first.
    pub replay_set: Mutex<LruCache<String, std::time::Instant>>,
    /// Shared HTTP client for outbound requests (e.g., Solana RPC slot fetch).
    pub http_client: reqwest::Client,
    /// Cached Solana slot for the `/v1/escrow/config` endpoint (5s TTL).
    pub slot_cache: SlotCache,
    /// In-memory escrow claim metrics (submitted, succeeded, failed, retried).
    /// `None` when escrow or claim processor is not configured.
    pub escrow_metrics: Option<Arc<solvela_x402::escrow::EscrowMetrics>>,
    /// Admin token for protected endpoints. `None` when not configured.
    pub admin_token: Option<String>,
    /// Prometheus metrics handle for rendering the `/metrics` endpoint.
    /// `None` when the recorder failed to install (metrics unavailable).
    pub prometheus_handle: Option<metrics_exporter_prometheus::PrometheusHandle>,
    /// When `true`, skip payment verification for chat requests (dev mode only).
    /// Always `false` in production — set via `SOLVELA_DEV_BYPASS_PAYMENT=true` (RCR_DEV_BYPASS_PAYMENT accepted as deprecated fallback).
    pub dev_bypass_payment: bool,
}

impl AppState {
    /// Default capacity for the in-memory replay protection LRU cache.
    const REPLAY_SET_CAPACITY: usize = 10_000;

    /// TTL for in-memory replay entries (120 seconds matches Solana blockhash lifetime).
    pub const REPLAY_TTL: std::time::Duration = std::time::Duration::from_secs(120);

    /// Create a new in-memory replay LRU cache with the default capacity.
    pub fn new_replay_set() -> Mutex<LruCache<String, std::time::Instant>> {
        Mutex::new(LruCache::new(
            NonZeroUsize::new(Self::REPLAY_SET_CAPACITY).expect("nonzero"),
        ))
    }
}

/// Custom panic handler that returns a JSON 500 response instead of dropping
/// the TCP connection. Used by [`CatchPanicLayer`] as the outermost middleware.
pub fn handle_panic(_err: Box<dyn std::any::Any + Send + 'static>) -> axum::response::Response {
    let body = serde_json::json!({
        "error": {
            "type": "internal_error",
            "message": "Internal server error"
        }
    });
    (
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        axum::Json(body),
    )
        .into_response()
}

/// Build the Axum router with all routes and middleware.
///
/// This is used by both `main.rs` and integration tests.
/// The `rate_limiter` is passed in so callers can retain a clone for background
/// cleanup tasks (see `main.rs`).
pub fn build_router(state: Arc<AppState>, rate_limiter: RateLimiter) -> Router {
    // Configurable request timeout (default 120s)
    let timeout_secs: u64 = std::env::var("SOLVELA_REQUEST_TIMEOUT_SECS")
        .or_else(|_| std::env::var("RCR_REQUEST_TIMEOUT_SECS"))
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(120);

    // Configurable max concurrent in-flight requests (default 256)
    let max_concurrent: usize = std::env::var("SOLVELA_MAX_CONCURRENT_REQUESTS")
        .or_else(|_| std::env::var("RCR_MAX_CONCURRENT_REQUESTS"))
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(256);

    Router::new()
        .route("/v1/chat/completions", post(routes::chat::chat_completions))
        .route(
            "/v1/images/generations",
            post(routes::images::image_generations),
        )
        .route("/v1/models", get(routes::models::list_models))
        .route("/v1/services", get(routes::services::list_services))
        .route(
            "/v1/services/register",
            post(routes::services::register_service),
        )
        .route(
            "/v1/services/{service_id}/proxy",
            post(routes::proxy::proxy_service),
        )
        .route("/v1/supported", get(routes::supported::supported))
        .route("/v1/nonce", get(routes::nonce::get_nonce))
        .route(
            "/v1/wallet/{address}/stats",
            get(routes::stats::wallet_stats),
        )
        .route("/v1/escrow/config", get(routes::escrow::escrow_config))
        .route("/v1/escrow/health", get(routes::escrow::escrow_health))
        .route("/pricing", get(routes::pricing::pricing))
        .route("/health", get(routes::health::health))
        .route("/v1/admin/stats", get(routes::admin_stats::admin_stats))
        .route(
            "/v1/orgs",
            post(routes::orgs::create_org).get(routes::orgs::list_orgs),
        )
        .route("/v1/orgs/{id}", get(routes::orgs::get_org))
        .route(
            "/v1/orgs/{id}/teams",
            post(routes::orgs::create_team).get(routes::orgs::list_teams),
        )
        .route(
            "/v1/orgs/{id}/members",
            post(routes::orgs::add_member).get(routes::orgs::list_members),
        )
        .route(
            "/v1/orgs/{id}/teams/{tid}/wallets",
            post(routes::orgs::assign_wallet).get(routes::orgs::list_team_wallets),
        )
        .route(
            "/v1/orgs/{id}/api-keys",
            post(routes::orgs::create_api_key).get(routes::orgs::list_api_keys),
        )
        .route(
            "/v1/orgs/{id}/api-keys/{kid}",
            axum::routing::delete(routes::orgs::revoke_api_key),
        )
        .route(
            "/v1/orgs/{id}/audit-logs",
            get(routes::orgs::list_audit_logs),
        )
        .route(
            "/v1/orgs/{id}/teams/{tid}/budget",
            axum::routing::put(routes::orgs::set_team_budget).get(routes::orgs::get_team_budget),
        )
        .route(
            "/v1/wallets/{wallet}/budget",
            axum::routing::put(routes::orgs::set_wallet_budget)
                .get(routes::orgs::get_wallet_budget),
        )
        .route(
            "/v1/orgs/{id}/teams/{tid}/stats",
            get(routes::orgs::get_team_stats),
        )
        .route("/v1/orgs/{id}/stats", get(routes::orgs::get_org_stats))
        .route("/.well-known/agent.json", get(a2a::agent_card::agent_card))
        .route("/a2a", post(a2a::jsonrpc::a2a_endpoint))
        .route("/metrics", get(routes::metrics::get_metrics))
        .layer(axum::middleware::from_fn(
            middleware::rate_limit::rate_limit,
        ))
        .layer(axum::Extension(rate_limiter))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::api_key::extract_api_key,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::x402::extract_payment,
        ))
        .layer(RequestBodyLimitLayer::new(10 * 1024 * 1024)) // 10 MB
        .layer(TraceLayer::new_for_http())
        .layer(axum::middleware::from_fn(
            middleware::metrics::record_metrics,
        ))
        .layer(build_cors())
        // Security headers — applied to every response
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("referrer-policy"),
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("content-security-policy"),
            HeaderValue::from_static("default-src 'none'"),
        ))
        .layer({
            // Only add HSTS in production to avoid issues with local dev (HTTP).
            // SOLVELA_ENV is canonical; RCR_ENV is accepted as a deprecated fallback.
            let env_value = std::env::var("SOLVELA_ENV").or_else(|_| std::env::var("RCR_ENV"));
            let is_prod = matches!(env_value.as_deref(), Ok("production") | Ok("prod"));
            SetResponseHeaderLayer::if_not_present(
                HeaderName::from_static("strict-transport-security"),
                if is_prod {
                    HeaderValue::from_static("max-age=31536000; includeSubDomains")
                } else {
                    // Empty value — Tower's if_not_present still sets the header,
                    // so we skip via a no-op value that browsers ignore.
                    HeaderValue::from_static("")
                },
            )
        })
        // Request ID
        .layer(RequestIdLayer)
        // Concurrency limit — rejects excess requests with 503
        .layer(ConcurrencyLimitLayer::new(max_concurrent))
        // Global request timeout — returns 408 on expiry
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            Duration::from_secs(timeout_secs),
        ))
        // Catch panics — outermost layer, returns JSON 500 instead of dropping connection
        .layer(CatchPanicLayer::custom(handle_panic))
        .with_state(state)
}

/// Build a restrictive CORS policy.
///
/// Allows the dashboard, localhost dev origins, and any origin explicitly
/// listed in the `SOLVELA_CORS_ORIGINS` environment variable (comma-separated;
/// `RCR_CORS_ORIGINS` is accepted as a deprecated fallback). Falls back to
/// denying all cross-origin browser requests if no origins are configured —
/// SDK/agent clients are unaffected since they don't use CORS.
fn build_cors() -> CorsLayer {
    // Collect allowed origins: env var overrides + dev-only localhost origins
    let mut origins: Vec<HeaderValue> = Vec::new();

    // Only allow localhost origins in non-production environments.
    // SOLVELA_ENV is canonical; RCR_ENV is accepted as a deprecated fallback.
    let env_value = std::env::var("SOLVELA_ENV")
        .or_else(|_| std::env::var("RCR_ENV"))
        .unwrap_or_else(|_| "development".to_string());
    let is_dev = env_value != "production" && env_value != "prod";
    if is_dev {
        for dev_origin in &[
            "http://localhost:3000",
            "http://localhost:8080",
            "http://127.0.0.1:3000",
        ] {
            if let Ok(v) = dev_origin.parse() {
                origins.push(v);
            }
        }
    }

    // Additional origins from env var (e.g., dashboard domain in prod).
    // SOLVELA_CORS_ORIGINS is canonical; RCR_CORS_ORIGINS is deprecated.
    let cors_env = std::env::var("SOLVELA_CORS_ORIGINS").or_else(|_| {
        std::env::var("RCR_CORS_ORIGINS").inspect(|_| {
            tracing::warn!(
                old = "RCR_CORS_ORIGINS",
                new = "SOLVELA_CORS_ORIGINS",
                "RCR_CORS_ORIGINS is deprecated; use SOLVELA_CORS_ORIGINS"
            );
        })
    });
    if let Ok(env_origins) = cors_env {
        for raw in env_origins.split(',') {
            let trimmed = raw.trim();
            if !trimmed.is_empty() {
                if let Ok(v) = trimmed.parse() {
                    origins.push(v);
                }
            }
        }
    }

    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            axum::http::header::CONTENT_TYPE,
            axum::http::header::AUTHORIZATION,
            // x402 custom header
            "payment-signature"
                .parse()
                .expect("'payment-signature' is a valid header name"),
            // Debug + request correlation headers (accept both prefixes)
            "x-request-id"
                .parse()
                .expect("'x-request-id' is a valid header name"),
            "x-solvela-debug"
                .parse()
                .expect("'x-solvela-debug' is a valid header name"),
            "x-rcr-debug"
                .parse()
                .expect("'x-rcr-debug' is a valid header name"),
            "x-solvela-fallback-preference"
                .parse()
                .expect("'x-solvela-fallback-preference' is a valid header name"),
            "x-rcr-fallback-preference"
                .parse()
                .expect("'x-rcr-fallback-preference' is a valid header name"),
            "x-session-id"
                .parse()
                .expect("'x-session-id' is a valid header name"),
        ])
        .expose_headers([
            // New x-solvela-* headers
            "x-solvela-request-id"
                .parse()
                .expect("'x-solvela-request-id' is a valid header name"),
            "x-solvela-model"
                .parse()
                .expect("'x-solvela-model' is a valid header name"),
            "x-solvela-tier"
                .parse()
                .expect("'x-solvela-tier' is a valid header name"),
            "x-solvela-score"
                .parse()
                .expect("'x-solvela-score' is a valid header name"),
            "x-solvela-profile"
                .parse()
                .expect("'x-solvela-profile' is a valid header name"),
            "x-solvela-provider"
                .parse()
                .expect("'x-solvela-provider' is a valid header name"),
            "x-solvela-cache"
                .parse()
                .expect("'x-solvela-cache' is a valid header name"),
            "x-solvela-latency-ms"
                .parse()
                .expect("'x-solvela-latency-ms' is a valid header name"),
            "x-solvela-payment-status"
                .parse()
                .expect("'x-solvela-payment-status' is a valid header name"),
            "x-solvela-token-estimate-in"
                .parse()
                .expect("'x-solvela-token-estimate-in' is a valid header name"),
            "x-solvela-token-estimate-out"
                .parse()
                .expect("'x-solvela-token-estimate-out' is a valid header name"),
            "x-solvela-session"
                .parse()
                .expect("'x-solvela-session' is a valid header name"),
            "x-solvela-fallback"
                .parse()
                .expect("'x-solvela-fallback' is a valid header name"),
            // Legacy x-rcr-* headers (backward compat)
            "x-rcr-request-id"
                .parse()
                .expect("'x-rcr-request-id' is a valid header name"),
            "x-rcr-model"
                .parse()
                .expect("'x-rcr-model' is a valid header name"),
            "x-rcr-tier"
                .parse()
                .expect("'x-rcr-tier' is a valid header name"),
            "x-rcr-score"
                .parse()
                .expect("'x-rcr-score' is a valid header name"),
            "x-rcr-profile"
                .parse()
                .expect("'x-rcr-profile' is a valid header name"),
            "x-rcr-provider"
                .parse()
                .expect("'x-rcr-provider' is a valid header name"),
            "x-rcr-cache"
                .parse()
                .expect("'x-rcr-cache' is a valid header name"),
            "x-rcr-latency-ms"
                .parse()
                .expect("'x-rcr-latency-ms' is a valid header name"),
            "x-rcr-payment-status"
                .parse()
                .expect("'x-rcr-payment-status' is a valid header name"),
            "x-rcr-token-estimate-in"
                .parse()
                .expect("'x-rcr-token-estimate-in' is a valid header name"),
            "x-rcr-token-estimate-out"
                .parse()
                .expect("'x-rcr-token-estimate-out' is a valid header name"),
            "x-rcr-session"
                .parse()
                .expect("'x-rcr-session' is a valid header name"),
            "x-rcr-fallback"
                .parse()
                .expect("'x-rcr-fallback' is a valid header name"),
            "x-session-id"
                .parse()
                .expect("'x-session-id' is a valid header name"),
        ])
}
