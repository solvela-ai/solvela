//! RustyClawRouter gateway — Axum HTTP server for AI agent LLM payments.
//!
//! This module exposes the gateway internals for integration testing.
//! The binary entry point is in `main.rs`.

pub mod balance_monitor;
pub mod cache;
pub mod config;
pub mod error;
pub mod middleware;
pub mod providers;
pub mod routes;
pub mod security;
pub mod service_health;
pub mod services;
pub mod session;
pub mod usage;

use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};

use axum::http::{HeaderName, HeaderValue, Method};
use axum::routing::{get, post};
use axum::Router;
use lru::LruCache;
use tokio::sync::RwLock;
use tower_http::cors::CorsLayer;
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

use crate::services::ServiceRegistry;
use router::models::ModelRegistry;
use x402::facilitator::Facilitator;

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
    pub escrow_claimer: Option<Arc<x402::escrow::EscrowClaimer>>,
    /// Hot wallet pool for fee payer rotation. `None` when no fee payer keys are configured.
    pub fee_payer_pool: Option<Arc<x402::fee_payer::FeePayerPool>>,
    /// Durable nonce account pool. `None` when no nonce accounts are configured.
    pub nonce_pool: Option<Arc<x402::nonce_pool::NoncePool>>,
    /// Optional PostgreSQL pool for durable claim queue and other DB operations.
    pub db_pool: Option<sqlx::PgPool>,
    /// HMAC secret for signing/verifying session tokens.
    pub session_secret: Vec<u8>,
    /// In-memory replay protection fallback used when Redis (`cache`) is absent.
    /// LRU-bounded to 10,000 entries so the oldest signatures are evicted first.
    pub replay_set: Mutex<LruCache<String, ()>>,
    /// Shared HTTP client for outbound requests (e.g., Solana RPC slot fetch).
    pub http_client: reqwest::Client,
    /// Cached Solana slot for the `/v1/escrow/config` endpoint (5s TTL).
    pub slot_cache: SlotCache,
    /// In-memory escrow claim metrics (submitted, succeeded, failed, retried).
    /// `None` when escrow or claim processor is not configured.
    pub escrow_metrics: Option<Arc<x402::escrow::EscrowMetrics>>,
    /// Prometheus metrics handle for rendering the `/metrics` endpoint.
    pub prometheus_handle: metrics_exporter_prometheus::PrometheusHandle,
}

impl AppState {
    /// Default capacity for the in-memory replay protection LRU cache.
    const REPLAY_SET_CAPACITY: usize = 10_000;

    /// Create a new in-memory replay LRU cache with the default capacity.
    pub fn new_replay_set() -> Mutex<LruCache<String, ()>> {
        Mutex::new(LruCache::new(
            NonZeroUsize::new(Self::REPLAY_SET_CAPACITY).expect("nonzero"),
        ))
    }
}

/// Build the Axum router with all routes and middleware.
///
/// This is used by both `main.rs` and integration tests.
/// The `rate_limiter` is passed in so callers can retain a clone for background
/// cleanup tasks (see `main.rs`).
pub fn build_router(state: Arc<AppState>, rate_limiter: RateLimiter) -> Router {
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
        .route("/metrics", get(routes::metrics::get_metrics))
        .layer(axum::middleware::from_fn(
            middleware::rate_limit::rate_limit,
        ))
        .layer(axum::Extension(rate_limiter))
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
        // Request ID — outermost layer, always attaches X-RCR-Request-Id
        .layer(RequestIdLayer)
        .with_state(state)
}

/// Build a restrictive CORS policy.
///
/// Allows the OpenClaw dashboard, localhost dev origins, and any origin
/// explicitly listed in the `RCR_CORS_ORIGINS` environment variable
/// (comma-separated). Falls back to denying all cross-origin browser requests
/// if no origins are configured — SDK/agent clients are unaffected since they
/// don't use CORS.
fn build_cors() -> CorsLayer {
    // Collect allowed origins: env var overrides + dev-only localhost origins
    let mut origins: Vec<HeaderValue> = Vec::new();

    // Only allow localhost origins in non-production environments
    let is_dev =
        std::env::var("RCR_ENV").unwrap_or_else(|_| "development".to_string()) != "production";
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

    // Additional origins from env var (e.g., dashboard domain in prod)
    if let Ok(env_origins) = std::env::var("RCR_CORS_ORIGINS") {
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
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([
            axum::http::header::CONTENT_TYPE,
            axum::http::header::AUTHORIZATION,
            // x402 custom header
            "payment-signature"
                .parse()
                .expect("'payment-signature' is a valid header name"),
            // Debug + request correlation headers
            "x-request-id"
                .parse()
                .expect("'x-request-id' is a valid header name"),
            "x-rcr-debug"
                .parse()
                .expect("'x-rcr-debug' is a valid header name"),
            "x-session-id"
                .parse()
                .expect("'x-session-id' is a valid header name"),
        ])
}
