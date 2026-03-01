//! RustyClawRouter gateway — Axum HTTP server for AI agent LLM payments.
//!
//! This module exposes the gateway internals for integration testing.
//! The binary entry point is in `main.rs`.

pub mod cache;
pub mod config;
pub mod error;
pub mod middleware;
pub mod providers;
pub mod routes;
pub mod usage;

use std::sync::Arc;

use axum::http::{HeaderValue, Method};
use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use rcr_common::services::ServiceRegistry;
use router::models::ModelRegistry;
use x402::facilitator::Facilitator;

use crate::middleware::rate_limit::{RateLimitConfig, RateLimiter};
use crate::providers::ProviderRegistry;

/// Shared application state passed to all route handlers.
pub struct AppState {
    pub config: config::AppConfig,
    pub model_registry: ModelRegistry,
    pub service_registry: ServiceRegistry,
    pub providers: ProviderRegistry,
    pub facilitator: Facilitator,
    pub usage: usage::UsageTracker,
    pub cache: Option<cache::ResponseCache>,
    pub provider_health: providers::health::ProviderHealthTracker,
}

/// Build the Axum router with all routes and middleware.
///
/// This is used by both `main.rs` and integration tests.
pub fn build_router(state: Arc<AppState>) -> Router {
    let rate_limiter = RateLimiter::new(RateLimitConfig::default());

    Router::new()
        .route("/v1/chat/completions", post(routes::chat::chat_completions))
        .route(
            "/v1/images/generations",
            post(routes::images::image_generations),
        )
        .route("/v1/models", get(routes::models::list_models))
        .route("/v1/services", get(routes::services::list_services))
        .route("/v1/supported", get(routes::supported::supported))
        .route("/pricing", get(routes::pricing::pricing))
        .route("/health", get(routes::health::health))
        .layer(axum::middleware::from_fn(
            middleware::rate_limit::rate_limit,
        ))
        .layer(axum::Extension(rate_limiter))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            middleware::x402::extract_payment,
        ))
        .layer(TraceLayer::new_for_http())
        .layer(build_cors())
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
    // Collect allowed origins: env var overrides + always-allowed dev origins
    let mut origins: Vec<HeaderValue> = Vec::new();

    // Always allow localhost for development
    for dev_origin in &[
        "http://localhost:3000",
        "http://localhost:8080",
        "http://127.0.0.1:3000",
    ] {
        if let Ok(v) = dev_origin.parse() {
            origins.push(v);
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
            "payment-signature".parse().unwrap(),
        ])
}
