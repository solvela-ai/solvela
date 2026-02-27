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

use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use router::models::ModelRegistry;
use x402::facilitator::Facilitator;

use crate::middleware::rate_limit::{RateLimitConfig, RateLimiter};
use crate::providers::ProviderRegistry;

/// Shared application state passed to all route handlers.
pub struct AppState {
    pub config: config::AppConfig,
    pub model_registry: ModelRegistry,
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
        .route("/v1/models", get(routes::models::list_models))
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
        .layer(CorsLayer::permissive())
        .with_state(state)
}
