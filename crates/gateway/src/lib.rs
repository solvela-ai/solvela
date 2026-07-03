//! Solvela gateway — Axum HTTP server for AI agent LLM payments.
//!
//! This module exposes the gateway internals for integration testing.
//! The binary entry point is in `main.rs`.

pub mod a2a;
pub mod audit;
pub mod balance_monitor;
pub mod cache;
pub mod channel_refunds;
pub mod channels;
pub mod config;
pub mod error;
pub mod middleware;
pub mod orgs;
pub mod payment_util;
pub mod providers;
pub mod receipts;
pub mod routes;
pub mod secret;
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

use crate::middleware::rate_limit::{FreeTierGlobalCap, RateLimiter};
use crate::middleware::request_id::RequestIdLayer;
use crate::providers::ProviderRegistry;
use crate::routes::escrow::SlotCache;

/// Shared application state passed to all route handlers.
pub struct AppState {
    pub config: config::AppConfig,
    pub model_registry: ModelRegistry,
    pub service_registry: RwLock<ServiceRegistry>,
    pub providers: ProviderRegistry,
    /// Dedicated NATIVE Anthropic relay handle for the `POST /v1/messages`
    /// passthrough. `Some` exactly when `ANTHROPIC_API_KEY` is configured (the
    /// same gate as the OpenAI-shaped `providers` Anthropic entry). The native
    /// fork uses [`providers::anthropic::AnthropicProvider::relay_native`] —
    /// an inherent method the `LLMProvider` trait cannot carry — so it lives on
    /// a concrete handle here rather than inside [`ProviderRegistry`]. When
    /// `None`, an Anthropic-resolved `/v1/messages` request cannot relay
    /// natively and fails closed (no silent reshape, no silent free serve).
    pub native_anthropic: Option<Arc<providers::anthropic::AnthropicProvider>>,
    /// Web-search tool upstream adapter (e.g. Tavily). `None` unless a search
    /// API key is configured (`TAVILY_API_KEY`) — mirrors [`ProviderRegistry`]
    /// env-gating. When `None`, `POST /v1/search` returns 503 (never a free or
    /// stub-paid response). See [`crate::providers::search`].
    pub search_provider: Option<Arc<dyn providers::search::SearchProvider>>,
    pub facilitator: Facilitator,
    pub usage: usage::UsageTracker,
    pub cache: Option<cache::ResponseCache>,
    /// Tier 2 semantic (embedding-similarity) cache. `None` unless
    /// `[cache.semantic].enabled` is set and both Redis (with RediSearch) and
    /// the embedding model are available. `Arc` so it can be cheaply cloned
    /// into the fire-and-forget store task.
    pub semantic_cache: Option<Arc<cache::semantic::SemanticCache>>,
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
    /// In-memory replay protection fallback used when Redis (`cache`) is
    /// absent. Backed by **three independent** per-path LRU buckets
    /// (`/v1/chat/completions`, `/v1/services/{id}/proxy`, `/a2a`) so a
    /// burst on one path cannot evict another path's replay entries.
    /// Each bucket is bounded to 10,000 entries with the same 120-second
    /// TTL as before. See [`ReplaySet`] / [`ReplayPath`].
    pub replay_set: ReplaySet,
    /// Shared HTTP client for outbound requests (e.g., Solana RPC slot fetch).
    pub http_client: reqwest::Client,
    /// Cached Solana slot for the `/v1/escrow/config` endpoint (5s TTL).
    pub slot_cache: SlotCache,
    /// In-memory escrow claim metrics (submitted, succeeded, failed, retried).
    /// `None` when escrow or claim processor is not configured.
    pub escrow_metrics: Option<Arc<solvela_x402::escrow::EscrowMetrics>>,
    /// Admin token for protected endpoints. `None` when not configured.
    /// Wrapped in [`secret::AdminToken`] so the value is redacted from `Debug`
    /// output, zeroized on drop, and only comparable via the constant-time
    /// `verify` method. See issue #173 (L4 GW).
    pub admin_token: Option<secret::AdminToken>,
    /// HMAC keying secret for API-key hashing at rest. When `Some`, new API
    /// keys are stored under HMAC-SHA256(secret, key) instead of plain
    /// SHA-256(key). Verification accepts both forms during the migration
    /// window — see [`secret::HmacSecret`] and [`crate::orgs::queries`].
    /// `None` falls back to legacy plain-SHA-256 behavior with a startup
    /// warning. See issue #173 (L1 GW).
    pub api_key_hmac_secret: Option<Arc<secret::HmacSecret>>,
    /// Optional pluggable authentication backend, consulted by
    /// [`middleware::api_key::extract_api_key`] **only when the built-in
    /// API-key path abstains**. `None` (the default) preserves the exact
    /// API-key-only behavior — the built-in path is always authoritative and
    /// runs first, so a provider can never override an API-key identity. This
    /// is the public extension seam the enterprise build uses to plug in
    /// SSO/OIDC/SAML without forking the request pipeline; self-hosters can use
    /// it for custom auth (LDAP, mTLS, …). See [`middleware::auth`].
    pub auth_provider: Option<Arc<dyn middleware::auth::AuthProvider>>,
    /// Prometheus metrics handle for rendering the `/metrics` endpoint.
    /// `None` when the recorder failed to install (metrics unavailable).
    pub prometheus_handle: Option<metrics_exporter_prometheus::PrometheusHandle>,
    /// When `true`, skip payment verification for chat requests (dev mode only).
    /// Always `false` in production — set via `SOLVELA_DEV_BYPASS_PAYMENT=true` (RCR_DEV_BYPASS_PAYMENT accepted as deprecated fallback).
    pub dev_bypass_payment: bool,
    /// Per-client (IP) rate limiter for the **anonymous free-tier bypass** path.
    ///
    /// Distinct from the global outer-layer limiter (wired in `build_router`):
    /// the outer limiter runs BEFORE model resolution and cannot know a request
    /// is free, so the free-tier cap is enforced inside the chat handler on the
    /// zero-cost branch only. Stricter default than the paid limit
    /// ([`RateLimitConfig::free_default`]); override via
    /// `SOLVELA_FREE_TIER_RATE_LIMIT`. Keyed on the TCP peer IP (never a
    /// client-supplied header — GHSA-6ggq-cvwx-4f67).
    pub free_rate_limiter: RateLimiter,
    /// Aggregate (global, all-clients-combined) free-tier rate cap.
    ///
    /// Complements [`free_rate_limiter`](Self::free_rate_limiter): the per-IP
    /// limiter rejects single-IP spammers cheaply, but cannot protect the
    /// upstream provider's SHARED free-tier ceiling (Google's free Gemini tier
    /// ~15 RPM across the whole API key) — many distinct IPs each under their
    /// per-IP cap can still collectively exceed it. This cap bounds the COMBINED
    /// free throughput so the gateway 429s before the provider does. Backed by
    /// Redis (cross-instance) when `cache` is `Some`, degrading to an in-memory
    /// per-instance counter otherwise. Default
    /// [`FREE_TIER_GLOBAL_RPM_DEFAULT`](crate::middleware::rate_limit::FREE_TIER_GLOBAL_RPM_DEFAULT);
    /// override via `SOLVELA_FREE_TIER_GLOBAL_RPM`.
    pub free_global_cap: FreeTierGlobalCap,
    /// Gas-drip faucet. `Some` only when the faucet is enabled, a dedicated
    /// gas `source_key` is configured, AND a DB pool is present (the once-per-
    /// wallet idempotency ledger lives in Postgres). `None` ⇒ the
    /// `POST /v1/faucet/gas` route returns `{funded:false, reason:"disabled"}`.
    /// See [`routes::faucet`].
    pub faucet: Option<Arc<routes::faucet::Faucet>>,
    /// Per-client (IP) rate limiter for the public, unauthenticated
    /// `GET /v1/receipts/{id}` route.
    ///
    /// Same in-handler pattern as [`free_rate_limiter`](Self::free_rate_limiter):
    /// every receipts GET is a DB query gated only by an unguessable-UUID
    /// capability, so this cap bounds enumeration/scanning per peer IP, STRICTER
    /// than the generic outer limiter ([`RateLimitConfig::receipts_default`]).
    /// Keyed on the TCP peer IP, never a client-supplied header
    /// (GHSA-6ggq-cvwx-4f67); absent `ConnectInfo` falls back to the shared
    /// stricter "unknown" bucket.
    pub receipts_rate_limiter: RateLimiter,
    /// Per-client (IP) rate limiter for the public, unauthenticated
    /// `POST /v1/faucet/gas` gas-drip route (security review finding F6).
    ///
    /// Same in-handler pattern as
    /// [`receipts_rate_limiter`](Self::receipts_rate_limiter): the faucet is
    /// unauthenticated and the per-wallet DB primary key only stops repeat drips
    /// to one wallet, NOT mass enumeration (mint fresh wallets, pre-fund each
    /// past the USDC floor, drain the daily cap in a one-IP burst). This cap
    /// bounds drip attempts per peer IP, STRICTER than the generic outer limiter
    /// ([`RateLimitConfig::faucet_default`]). Keyed on the TCP peer IP, never a
    /// client-supplied header (GHSA-6ggq-cvwx-4f67); absent `ConnectInfo` falls
    /// back to the shared stricter "unknown" bucket.
    pub faucet_rate_limiter: RateLimiter,
    /// Per-client (IP) rate limiter for the public, unauthenticated
    /// `POST /v1/escrow/deposit-tx` unsigned-deposit-tx builder route.
    ///
    /// Same in-handler pattern as
    /// [`faucet_rate_limiter`](Self::faucet_rate_limiter): the route is
    /// unauthenticated and each call can fan out to Solana RPC (slot +
    /// blockhash), so this cap bounds RPC-amplification per peer IP, STRICTER
    /// than the generic outer limiter but more generous than the faucet
    /// ([`RateLimitConfig::deposit_tx_default`]) because building deposit
    /// transactions is a legitimate, occasionally-bursty funding flow. Keyed on
    /// the TCP peer IP, never a client-supplied header (GHSA-6ggq-cvwx-4f67);
    /// absent `ConnectInfo` falls back to the shared stricter "unknown" bucket.
    pub deposit_tx_rate_limiter: RateLimiter,
}

impl AppState {
    /// TTL for in-memory replay entries (120 seconds matches Solana blockhash lifetime).
    pub const REPLAY_TTL: std::time::Duration = std::time::Duration::from_secs(120);

    /// Create a new in-memory replay set (per-path bucketed LRU cache).
    ///
    /// Returns a [`ReplaySet`] with four independent 10,000-entry LRU
    /// buckets — one per route group. Construction sites continue to
    /// call this helper unchanged; lookup sites navigate via
    /// [`ReplaySet::for_path`].
    pub fn new_replay_set() -> ReplaySet {
        ReplaySet::new()
    }
}

/// Per-path bucket selector for the in-memory replay LRU.
///
/// Each variant maps to one route group whose payment payloads should
/// not affect another group's replay-cache eviction order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayPath {
    /// `POST /v1/chat/completions`
    Chat,
    /// `POST /v1/services/{service_id}/proxy`
    Proxy,
    /// `POST /a2a` (`message/send` flow)
    A2a,
    /// `POST /v1/search` (internal web-search tool)
    Search,
}

/// In-memory replay protection fallback used when Redis is absent.
///
/// Holds four independent LRUs keyed by [`ReplayPath`] so eviction in
/// one path's bucket does not affect the others. Per-bucket capacity
/// matches the previous shared capacity (10K) — total memory is 4× the
/// prior single-LRU footprint, but the absolute size is small (≈8 MB
/// worst case for ~200-byte entries).
pub struct ReplaySet {
    chat: Mutex<LruCache<String, std::time::Instant>>,
    proxy: Mutex<LruCache<String, std::time::Instant>>,
    a2a: Mutex<LruCache<String, std::time::Instant>>,
    search: Mutex<LruCache<String, std::time::Instant>>,
}

impl ReplaySet {
    /// Default capacity for each per-path bucket.
    const PER_PATH_CAPACITY: usize = 10_000;

    pub fn new() -> Self {
        let bucket = || {
            Mutex::new(LruCache::new(
                NonZeroUsize::new(Self::PER_PATH_CAPACITY).expect("nonzero"),
            ))
        };
        Self {
            chat: bucket(),
            proxy: bucket(),
            a2a: bucket(),
            search: bucket(),
        }
    }

    /// Select the LRU bucket for `path`. Callers lock the returned
    /// `Mutex` to access the underlying cache directly — preserving
    /// the existing ad-hoc TTL handling at each call site.
    pub fn for_path(&self, path: ReplayPath) -> &Mutex<LruCache<String, std::time::Instant>> {
        match path {
            ReplayPath::Chat => &self.chat,
            ReplayPath::Proxy => &self.proxy,
            ReplayPath::A2a => &self.a2a,
            ReplayPath::Search => &self.search,
        }
    }
}

impl Default for ReplaySet {
    fn default() -> Self {
        Self::new()
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
        // GET serves the x402 discovery 402 (so registry health-checkers probing
        // with a GET see the challenge instead of a 405); POST is the real
        // OpenAI-compatible endpoint (which itself returns the discovery 402 for
        // an UNPAID empty/malformed body — see `chat_completions`).
        .route(
            "/v1/chat/completions",
            get(routes::chat::chat_completions_discovery_get).post(routes::chat::chat_completions),
        )
        // Inbound Anthropic-Messages-compatible endpoint. POST translates the
        // Anthropic wire format to the internal OpenAI-shaped pipeline and back,
        // riding the SAME x402 money path as /v1/chat/completions (via the
        // shared `chat_completions_inner` core — no forked payment logic). GET
        // serves the x402 discovery 402 for registry health-checkers, mirroring
        // the chat route.
        .route(
            "/v1/messages",
            get(routes::messages::create_message_discovery_get)
                .post(routes::messages::create_message),
        )
        // Anthropic token-counting endpoint. Free (Anthropic does not charge for
        // count_tokens), no payment path — a verbatim reverse-proxy to Anthropic's
        // own count_tokens endpoint so a native Claude client gets exact counts.
        .route(
            "/v1/messages/count_tokens",
            post(routes::messages::count_message_tokens),
        )
        .route(
            "/v1/images/generations",
            post(routes::images::image_generations),
        )
        .route("/v1/search", post(routes::search::search))
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
        .route(
            "/v1/receipts/{receipt_id}",
            get(routes::receipts::get_receipt),
        )
        .route("/v1/supported", get(routes::supported::supported))
        .route("/v1/nonce", get(routes::nonce::get_nonce))
        .route(
            "/v1/wallet/{address}/stats",
            get(routes::stats::wallet_stats),
        )
        .route("/v1/escrow/config", get(routes::escrow::escrow_config))
        .route("/v1/escrow/health", get(routes::escrow::escrow_health))
        .route("/v1/escrow/deposit-tx", post(routes::escrow::deposit_tx))
        .route(
            "/v1/escrow/settle",
            post(routes::escrow_settle::handle_settle),
        )
        .route("/v1/channel/open", post(routes::channel::open))
        .route("/v1/channel/close", post(routes::channel::close))
        .route("/v1/faucet/gas", post(routes::faucet::gas_faucet))
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
        // A2A v0.3 canonical AgentCard path (RFC 8615) + pre-v0.3 alias.
        .route(
            "/.well-known/agent-card.json",
            get(a2a::agent_card::agent_card),
        )
        .route("/.well-known/agent.json", get(a2a::agent_card::agent_card))
        // Static well-known files (x402-registry domain verification, etc.),
        // served verbatim from `server.wellknown_files`; 404 when unconfigured.
        .route(
            "/.well-known/402index-verify.txt",
            get(
                |axum::extract::State(state): axum::extract::State<std::sync::Arc<AppState>>| async move {
                    match state.config.server.wellknown_files.get("402index-verify.txt") {
                        Some(contents) => contents.clone().into_response(),
                        None => axum::http::StatusCode::NOT_FOUND.into_response(),
                    }
                },
            ),
        )
        .route("/a2a", post(a2a::jsonrpc::a2a_endpoint))
        // OpenAPI-first x402 discovery surfaces (x402scan et al.): the spec
        // served verbatim at the root path, the `/.well-known/x402` resource
        // list, and a favicon to clear the FAVICON_MISSING audit flag. All
        // public, additive discovery metadata — no payment/wire change.
        .route("/openapi.json", get(routes::openapi::openapi_spec))
        .route("/.well-known/x402", get(routes::openapi::well_known_x402))
        .route("/favicon.ico", get(routes::openapi::favicon))
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
            // Only add HSTS in production — omit the layer entirely in non-prod
            // environments so the header is never emitted (not even as an empty value).
            // SOLVELA_ENV is canonical; RCR_ENV is accepted as a deprecated fallback.
            let env_value = std::env::var("SOLVELA_ENV").or_else(|_| std::env::var("RCR_ENV"));
            let is_prod = matches!(env_value.as_deref(), Ok("production") | Ok("prod"));
            tower::util::option_layer(is_prod.then(|| {
                SetResponseHeaderLayer::if_not_present(
                    HeaderName::from_static("strict-transport-security"),
                    HeaderValue::from_static("max-age=31536000; includeSubDomains"),
                )
            }))
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
            "x-tenant"
                .parse()
                .expect("'x-tenant' is a valid header name"),
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
            "x-solvela-receipt"
                .parse()
                .expect("'x-solvela-receipt' is a valid header name"),
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

#[cfg(test)]
mod replay_set_tests {
    use super::{ReplayPath, ReplaySet};

    #[test]
    fn for_path_returns_distinct_buckets() {
        // Four buckets, four distinct memory addresses — the wrapper
        // hands out independent Mutex pointers per path.
        let rs = ReplaySet::new();
        let chat = rs.for_path(ReplayPath::Chat) as *const _;
        let proxy = rs.for_path(ReplayPath::Proxy) as *const _;
        let a2a = rs.for_path(ReplayPath::A2a) as *const _;
        let search = rs.for_path(ReplayPath::Search) as *const _;
        assert_ne!(chat, proxy, "chat and proxy buckets must differ");
        assert_ne!(chat, a2a, "chat and a2a buckets must differ");
        assert_ne!(proxy, a2a, "proxy and a2a buckets must differ");
        assert_ne!(search, chat, "search and chat buckets must differ");
        assert_ne!(search, proxy, "search and proxy buckets must differ");
        assert_ne!(search, a2a, "search and a2a buckets must differ");
    }

    #[test]
    fn entries_are_isolated_across_paths() {
        // The same tx_raw inserted into one path's bucket must NOT be
        // visible in another path's bucket — the eviction-isolation
        // property the M1 fix is about.
        let rs = ReplaySet::new();
        let tx = "FAKE_TX_BASE64".to_string();
        let now = std::time::Instant::now();

        rs.for_path(ReplayPath::Chat)
            .lock()
            .expect("fresh mutex must lock")
            .put(tx.clone(), now);

        // Chat sees it
        assert!(
            rs.for_path(ReplayPath::Chat)
                .lock()
                .expect("fresh mutex must lock")
                .get(&tx)
                .is_some(),
            "chat bucket must hold the entry it just inserted"
        );

        // Proxy and A2a do NOT see it
        assert!(
            rs.for_path(ReplayPath::Proxy)
                .lock()
                .expect("fresh mutex must lock")
                .get(&tx)
                .is_none(),
            "proxy bucket must not see chat's entry"
        );
        assert!(
            rs.for_path(ReplayPath::A2a)
                .lock()
                .expect("fresh mutex must lock")
                .get(&tx)
                .is_none(),
            "a2a bucket must not see chat's entry"
        );
    }

    #[test]
    fn burst_on_one_path_does_not_evict_others() {
        // The eviction-isolation regression sentinel. Fill chat's bucket
        // beyond capacity; proxy's bucket must still hold its single
        // entry.
        let rs = ReplaySet::new();
        let proxy_tx = "PROXY_ONLY_TX".to_string();
        let now = std::time::Instant::now();

        rs.for_path(ReplayPath::Proxy)
            .lock()
            .expect("fresh mutex must lock")
            .put(proxy_tx.clone(), now);

        // Push enough fillers into chat to exceed PER_PATH_CAPACITY.
        // Even with the chat bucket fully evicting its own contents,
        // proxy's bucket is untouched.
        let mut chat = rs
            .for_path(ReplayPath::Chat)
            .lock()
            .expect("fresh mutex must lock");
        for i in 0..(ReplaySet::PER_PATH_CAPACITY + 100) {
            chat.put(format!("chat_filler_{i}"), now);
        }
        drop(chat); // release lock before checking proxy

        assert!(
            rs.for_path(ReplayPath::Proxy)
                .lock()
                .expect("fresh mutex must lock")
                .get(&proxy_tx)
                .is_some(),
            "proxy entry must survive a chat-bucket capacity overflow"
        );
    }
}
