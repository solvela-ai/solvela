use std::sync::Arc;

use tracing::{info, warn};

use gateway::services::ServiceRegistry;
use gateway::{
    balance_monitor::BalanceMonitor,
    build_router, cache, config,
    middleware::rate_limit::{RateLimitConfig, RateLimiter},
    providers::ProviderRegistry,
    AppState,
};
use router::models::ModelRegistry;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env file if present (before any env var reads).
    // Existing env vars take precedence — .env values are not overwritten.
    dotenvy::dotenv().ok();

    // Initialize tracing — text by default, JSON when RCR_LOG_FORMAT=json
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "gateway=info,tower_http=info".into());

    match std::env::var("RCR_LOG_FORMAT").as_deref() {
        Ok("json") => {
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .json()
                .init();
        }
        _ => {
            tracing_subscriber::fmt().with_env_filter(filter).init();
        }
    };

    // Load configuration: TOML file as base, then env var overrides
    let mut app_config = match std::fs::read_to_string("config/default.toml") {
        Ok(toml_str) => toml::from_str::<config::AppConfig>(&toml_str).unwrap_or_else(|e| {
            tracing::warn!(error = %e, "failed to parse config/default.toml, using defaults");
            config::AppConfig::default()
        }),
        Err(_) => config::AppConfig::default(),
    };

    // Override Solana config from environment variables.
    // Supports both double-underscore (Fly.io style: RCR_SOLANA__RPC_URL) and
    // single-underscore (RCR_SOLANA_RPC_URL) conventions.
    if let Ok(val) = std::env::var("RCR_SOLANA__RPC_URL")
        .or_else(|_| std::env::var("RCR_SOLANA_RPC_URL"))
    {
        app_config.solana.rpc_url = val;
    }
    if let Ok(val) = std::env::var("RCR_SOLANA__RECIPIENT_WALLET")
        .or_else(|_| std::env::var("RCR_SOLANA_RECIPIENT_WALLET"))
    {
        app_config.solana.recipient_wallet = val;
    }
    if let Ok(val) = std::env::var("RCR_SOLANA__USDC_MINT") {
        app_config.solana.usdc_mint = val;
    }
    if let Ok(val) = std::env::var("RCR_SOLANA__ESCROW_PROGRAM_ID")
        .or_else(|_| std::env::var("RCR_SOLANA_ESCROW_PROGRAM_ID"))
    {
        app_config.solana.escrow_program_id = Some(val);
    }
    if let Ok(val) = std::env::var("RCR_SOLANA__FEE_PAYER_KEY")
        .or_else(|_| std::env::var("RCR_SOLANA_FEE_PAYER_KEY"))
    {
        app_config.solana.fee_payer_key = Some(val);
    }
    // Server config overrides
    if let Ok(val) = std::env::var("RCR_HOST") {
        app_config.server.host = val;
    }
    if let Ok(val) = std::env::var("RCR_PORT") {
        if let Ok(port) = val.parse::<u16>() {
            app_config.server.port = port;
        }
    }

    // Load model registry from config file
    let models_toml = std::fs::read_to_string("config/models.toml")
        .unwrap_or_else(|_| include_str!("../../../config/models.toml").to_string());
    let model_registry = ModelRegistry::from_toml(&models_toml)?;

    let model_count = model_registry.all().len();

    // Load service registry from config file
    let services_toml = std::fs::read_to_string("config/services.toml")
        .unwrap_or_else(|_| include_str!("../../../config/services.toml").to_string());
    let service_registry_inner = ServiceRegistry::from_toml(&services_toml).unwrap_or_else(|e| {
        warn!(error = %e, "failed to parse services.toml, using empty registry");
        ServiceRegistry::empty()
    });
    info!(
        services = service_registry_inner.all().len(),
        "loaded service registry"
    );
    let service_registry = tokio::sync::RwLock::new(service_registry_inner);

    // ── Shared HTTP client ─────────────────────────────────────────────────
    //
    // A single reqwest::Client is shared across all provider adapters and
    // general-purpose outbound calls (Solana RPC, health checks). The 10s
    // client-level timeout applies to non-LLM calls; each provider adapter
    // overrides it with a 90s per-request timeout for LLM API calls.
    let http_client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .expect("failed to build HTTP client");

    // Initialize provider registry from environment API keys
    let providers = ProviderRegistry::from_env(http_client.clone());
    let configured = providers.configured_providers();
    info!(providers = ?configured, "initialized provider registry");

    // Initialize Solana payment verifier.
    //
    // SECURITY: In production mode (RCR_ENV=production), a valid Solana config
    // is required. Falling back to devnet in production would accept devnet
    // transactions as valid payments or cause all real payments to fail.
    let is_production = matches!(
        std::env::var("RCR_ENV").as_deref(),
        Ok("production") | Ok("prod")
    );

    let solana_verifier = match x402::solana::SolanaVerifier::new(
        &app_config.solana.rpc_url,
        &app_config.solana.recipient_wallet,
        &app_config.solana.usdc_mint,
        http_client.clone(),
    ) {
        Ok(v) => {
            if app_config.solana.rpc_url.contains("devnet") {
                if is_production {
                    panic!(
                        "FATAL: Solana RPC URL points to devnet in production mode. \
                         Set RCR_SOLANA_RPC_URL to a mainnet-beta endpoint."
                    );
                }
                warn!(
                    "⚠ Solana RPC URL points to devnet — payments will use devnet. \
                     Set RCR_SOLANA_RPC_URL for mainnet-beta in production."
                );
            }
            v
        }
        Err(e) => {
            if is_production {
                panic!(
                    "FATAL: Failed to initialize Solana verifier in production: {e}. \
                     Set RCR_SOLANA_RPC_URL and RCR_SOLANA_RECIPIENT_WALLET."
                );
            }
            warn!(
                error = %e,
                "failed to initialize Solana verifier, using devnet defaults. \
                 THIS IS UNSAFE FOR PRODUCTION — set RCR_ENV=production to enforce."
            );
            x402::solana::SolanaVerifier::new(
                "https://api.devnet.solana.com",
                "11111111111111111111111111111111",
                "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                http_client.clone(),
            )
            .expect("default verifier config must be valid")
        }
    };

    // Build verifiers list — always include the direct SolanaVerifier
    let mut verifiers: Vec<Arc<dyn x402::traits::PaymentVerifier>> =
        vec![Arc::new(solana_verifier)];

    // ── Fee payer pool (hot wallet rotation) ──────────────────────────────────
    //
    // Build a FeePayerPool from all configured fee payer keys.
    // Falls back to env-var scanning (RCR_SOLANA__FEE_PAYER_KEY[_N]) when the
    // merged config list is empty.
    // Constructed before EscrowClaimer because the claimer now uses the pool.
    let fee_payer_pool = {
        let merged_keys = app_config.solana.all_fee_payer_keys();
        if merged_keys.is_empty() {
            // Try loading directly from env vars as a fallback
            match x402::fee_payer::FeePayerPool::from_env() {
                Ok(pool) => {
                    info!(
                        wallets = pool.len(),
                        "fee payer pool initialized from env vars"
                    );
                    Some(Arc::new(pool))
                }
                Err(_) => {
                    info!("no fee payer keys configured — fee payer pool disabled");
                    None
                }
            }
        } else {
            match x402::fee_payer::FeePayerPool::from_keys(&merged_keys) {
                Ok(pool) => {
                    info!(
                        wallets = pool.len(),
                        "fee payer pool initialized from config"
                    );
                    Some(Arc::new(pool))
                }
                Err(e) => {
                    warn!(error = %e, "failed to init FeePayerPool — pool disabled");
                    None
                }
            }
        }
    };

    // ── Durable nonce pool ────────────────────────────────────────────────────
    //
    // Build a NoncePool from environment variables. Returns an empty pool
    // (not an error) if no nonce accounts are configured — clients fall back
    // to using a recent blockhash in that case.
    // Constructed before EscrowClaimer because the claimer now uses the pool.
    let nonce_pool = {
        let pool = x402::nonce_pool::NoncePool::from_env();
        if pool.is_empty() {
            info!("nonce pool empty — clients will use recent blockhash (set RCR_SOLANA__NONCE_ACCOUNT to enable)");
            None
        } else {
            info!(accounts = pool.len(), "durable nonce pool initialized");
            Some(Arc::new(pool))
        }
    };

    // Conditionally build EscrowVerifier + EscrowClaimer if configured.
    // The claimer now uses FeePayerPool for rotation and optionally NoncePool
    // for durable nonces.
    let escrow_claimer = if let (Some(prog_id), Some(ref pool)) =
        (&app_config.solana.escrow_program_id, &fee_payer_pool)
    {
        let escrow_verifier = x402::escrow::EscrowVerifier {
            rpc_url: app_config.solana.rpc_url.clone(),
            recipient_wallet: app_config.solana.recipient_wallet.clone(),
            usdc_mint: app_config.solana.usdc_mint.clone(),
            escrow_program_id: prog_id.clone(),
            http_client: http_client.clone(),
        };
        verifiers.push(Arc::new(escrow_verifier));

        match x402::escrow::EscrowClaimer::new(
            app_config.solana.rpc_url.clone(),
            Arc::clone(pool),
            prog_id,
            &app_config.solana.recipient_wallet,
            &app_config.solana.usdc_mint,
            nonce_pool.clone(),
        ) {
            Ok(claimer) => {
                info!("escrow payment mode enabled (fee payer pool rotation + optional durable nonces)");
                Some(Arc::new(claimer))
            }
            Err(e) => {
                warn!(error = %e, "failed to init EscrowClaimer — escrow disabled");
                None
            }
        }
    } else {
        if app_config.solana.escrow_program_id.is_some() && fee_payer_pool.is_none() {
            warn!("escrow program ID configured but no fee payer pool — escrow disabled");
        } else {
            info!("escrow payment mode disabled (set RCR_SOLANA_ESCROW_PROGRAM_ID + RCR_SOLANA_FEE_PAYER_KEY to enable)");
        }
        None
    };

    let facilitator = x402::facilitator::Facilitator::new(verifiers);

    // ── PostgreSQL connection (optional — gracefully degrades to noop) ──────────
    //
    // Set DATABASE_URL to enable persistent spend logging and wallet budgets.
    // Without it, requests still work but spend data is only logged to stdout.
    let max_connections: u32 = std::env::var("RCR_DB_MAX_CONNECTIONS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);

    let db_pool = match std::env::var("DATABASE_URL") {
        Ok(url) => {
            // Warn if production mode but DATABASE_URL lacks sslmode
            if is_production && !url.contains("sslmode=") {
                warn!(
                    "DATABASE_URL does not contain sslmode= parameter — \
                     database connections may be unencrypted. \
                     Add ?sslmode=require (or &sslmode=require) for production."
                );
            }
            match sqlx::postgres::PgPoolOptions::new()
                .max_connections(max_connections)
                .acquire_timeout(std::time::Duration::from_secs(5))
                .connect(&url)
                .await
            {
                Ok(pool) => {
                    info!("PostgreSQL connected — spend logging enabled");
                    // Run migrations on startup so the schema is always up to date.
                    if let Err(e) = run_migrations(&pool).await {
                        warn!(error = %e, "migration failed — spend logging may not work correctly");
                    }
                    Some(pool)
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        url = %redact_connection_url(&url),
                        "PostgreSQL connection failed — spend logging disabled"
                    );
                    None
                }
            }
        }
        Err(_) => {
            warn!("DATABASE_URL not set — spend logging disabled (set in .env to enable)");
            None
        }
    };

    // ── Redis connection (optional — gracefully degrades to no cache) ──────────
    //
    // Set REDIS_URL to enable response caching and hot-path rate limiting.
    // Without it, every request hits the upstream LLM provider directly.
    let redis_url =
        std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());

    let redis_client = match redis::Client::open(redis_url.as_str()) {
        Ok(client) => {
            // Probe connection — Client::open is lazy; we need to actually connect.
            match client.get_multiplexed_async_connection().await {
                Ok(_) => {
                    info!("Redis connected — response cache and rate limiting enabled");
                    Some(client)
                }
                Err(e) => {
                    warn!(
                        error = %e,
                        url = %redact_connection_url(&redis_url),
                        "Redis connection failed — cache disabled"
                    );
                    None
                }
            }
        }
        Err(e) => {
            warn!(error = %e, "invalid Redis URL — cache disabled");
            None
        }
    };

    // Build usage tracker with whatever backends are available
    let usage = gateway::usage::UsageTracker::new(db_pool.clone(), redis_client.clone());

    // Build response cache (requires Redis)
    let response_cache = redis_client.as_ref().and_then(|client| {
        match cache::ResponseCache::from_client(client.clone(), cache::CacheConfig::default()) {
            Ok(c) => {
                info!("response cache enabled");
                Some(c)
            }
            Err(e) => {
                warn!(error = %e, "response cache initialization failed");
                None
            }
        }
    });

    // Initialize provider health tracker
    let provider_health = gateway::providers::health::ProviderHealthTracker::new(
        gateway::providers::health::CircuitBreakerConfig::default(),
    );

    // ── Escrow metrics (in-memory atomic counters) ─────────────────────────
    //
    // Created here so both the claim processor and the AppState can share
    // the same Arc.  `None` when escrow is not configured or no DB.
    let escrow_metrics: Option<Arc<x402::escrow::EscrowMetrics>> =
        if escrow_claimer.is_some() && db_pool.is_some() {
            Some(Arc::new(x402::escrow::EscrowMetrics::new()))
        } else {
            None
        };

    // ── Prometheus metrics recorder ─────────────────────────────────────────
    //
    // Install the global Prometheus recorder. The PrometheusHandle is stored
    // in AppState so the /metrics endpoint can call `handle.render()`.
    let prometheus_handle = match metrics_exporter_prometheus::PrometheusBuilder::new()
        .install_recorder()
    {
        Ok(handle) => Some(handle),
        Err(e) => {
            tracing::error!(error = %e, "failed to install Prometheus recorder — metrics will be unavailable");
            None
        }
    };

    // Read admin token once at startup
    let admin_token = std::env::var("RCR_ADMIN_TOKEN")
        .ok()
        .filter(|t| !t.is_empty());

    // Dev-mode payment bypass — only when RCR_DEV_BYPASS_PAYMENT=true AND not production
    let dev_bypass_payment = if is_production {
        if std::env::var("RCR_DEV_BYPASS_PAYMENT").as_deref() == Ok("true") {
            warn!("RCR_DEV_BYPASS_PAYMENT is set but ignored — payment bypass is NEVER allowed in production");
        }
        false
    } else {
        let enabled = std::env::var("RCR_DEV_BYPASS_PAYMENT").as_deref() == Ok("true");
        if enabled {
            warn!("DEV MODE: payment bypass ENABLED — all chat requests will skip payment verification. DO NOT use in production!");
        }
        enabled
    };

    // Build shared state
    let state = Arc::new(AppState {
        config: app_config.clone(),
        model_registry,
        service_registry,
        providers,
        facilitator,
        usage,
        cache: response_cache,
        provider_health,
        escrow_claimer,
        fee_payer_pool,
        nonce_pool,
        db_pool,
        escrow_metrics: escrow_metrics.clone(),
        admin_token,
        session_secret: {
            let secret = match std::env::var("RCR_SESSION_SECRET") {
                Ok(b64) => {
                    use base64::Engine;
                    base64::engine::general_purpose::STANDARD
                        .decode(&b64)
                        .unwrap_or_else(|e| {
                            warn!(error = %e, "invalid RCR_SESSION_SECRET base64 — generating random secret");
                            generate_random_secret()
                        })
                }
                Err(_) => {
                    warn!("RCR_SESSION_SECRET not set — generating ephemeral session secret (sessions will not survive restarts)");
                    generate_random_secret()
                }
            };
            if secret.len() < 32 {
                warn!(
                    len = secret.len(),
                    "session secret is shorter than 32 bytes — this is cryptographically weak; generating random secret"
                );
                generate_random_secret()
            } else {
                secret
            }
        },
        http_client,
        replay_set: AppState::new_replay_set(),
        slot_cache: gateway::routes::escrow::new_slot_cache(),
        prometheus_handle,
        dev_bypass_payment,
    });

    // ── Shutdown signal for background tasks ────────────────────────────────
    //
    // A watch channel that signals background tasks (like the claim processor)
    // to shut down gracefully. The sender fires when SIGTERM / Ctrl+C arrives.
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

    // ── Service health checker (background task) ────────────────────────────
    //
    // Periodically probes external x402 services and updates their health
    // status in the registry. Only starts if there are external services.
    {
        let has_external = {
            let registry = state.service_registry.read().await;
            !registry.external().is_empty()
        };
        if has_external {
            let health_shutdown_rx = shutdown_rx.clone();
            let _handle = gateway::service_health::start_service_health_checker(
                Arc::clone(&state),
                health_shutdown_rx,
            );
            info!("service health checker started");
        } else {
            info!("service health checker disabled — no external services");
        }
    }

    // ── Claim processor (durable escrow claim background task) ──────────────
    //
    // If both PostgreSQL and an EscrowClaimer are available, start the
    // background claim processor that polls the escrow_claim_queue table.
    if let (Some(ref pool), Some(ref claimer)) = (&state.db_pool, &state.escrow_claimer) {
        let claim_shutdown_rx = shutdown_rx.clone();
        let _handle = x402::escrow::claim_processor::start_claim_processor(
            pool.clone(),
            Arc::clone(claimer),
            std::time::Duration::from_secs(10),
            escrow_metrics.clone(),
            claim_shutdown_rx,
        );
        info!("escrow claim processor started");
    } else if state.escrow_claimer.is_some() && state.db_pool.is_none() {
        warn!("escrow configured but no database — claim processor disabled");
    }

    // ── Balance monitor (fire-and-forget background task) ─────────────────────
    //
    // Monitors SOL balances of the fee-payer wallet(s) and emits structured
    // tracing events when balances drop below configured thresholds.
    // Uses the RPC URL from monitor config, falling back to the Solana config RPC URL.
    {
        let mut wallet_pubkeys: Vec<String> = Vec::new();

        // Monitor the recipient wallet (always configured)
        if !app_config.solana.recipient_wallet.is_empty() {
            wallet_pubkeys.push(app_config.solana.recipient_wallet.clone());
        }

        // Add fee payer pool wallets to the monitor
        if let Some(ref pool) = state.fee_payer_pool {
            for pubkey in pool.pubkeys() {
                // Avoid duplicates (fee payer may equal recipient wallet)
                if !wallet_pubkeys.contains(&pubkey) {
                    wallet_pubkeys.push(pubkey);
                }
            }
        }

        if wallet_pubkeys.is_empty() {
            info!("balance monitor disabled — no wallet pubkeys configured");
        } else {
            let mut monitor_config = app_config.monitor.clone();
            // If the monitor RPC URL is still the default, use the Solana config RPC URL
            // so operators only need to set one RPC URL.
            if monitor_config.rpc_url == "https://api.devnet.solana.com"
                && app_config.solana.rpc_url != "https://api.devnet.solana.com"
            {
                monitor_config.rpc_url = app_config.solana.rpc_url.clone();
            }

            let monitor = Arc::new(BalanceMonitor::new(monitor_config, wallet_pubkeys));
            info!(
                wallets = monitor.wallet_count(),
                interval_secs = app_config.monitor.check_interval_secs,
                warn_sol = %app_config.monitor.warn_threshold_sol,
                critical_sol = %app_config.monitor.critical_threshold_sol,
                "balance monitor started"
            );
            // Fire-and-forget — we intentionally drop the handle so the background
            // task runs independently for the lifetime of the process.
            // Shutdown signal ensures clean exit on SIGTERM/Ctrl+C.
            let monitor_shutdown_rx = shutdown_rx.clone();
            drop(BalanceMonitor::spawn(monitor, monitor_shutdown_rx));
        }
    }

    // ── Rate limiter + periodic cleanup ───────────────────────────────────────
    let rate_limiter = RateLimiter::new(RateLimitConfig::default());

    // Spawn periodic cleanup task — removes expired entries every 60 seconds
    // to prevent the in-memory HashMap from growing without bound.
    // Shuts down gracefully when the shutdown signal fires.
    let mut rl_shutdown_rx = shutdown_rx.clone();
    let rl_clone = rate_limiter.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    rl_clone.cleanup().await;
                }
                _ = rl_shutdown_rx.changed() => {
                    info!("rate limiter cleanup shutting down gracefully");
                    break;
                }
            }
        }
    });

    // Build router
    let app = build_router(state, rate_limiter);

    let addr = format!("{}:{}", app_config.server.host, app_config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    info!(
        addr,
        models = model_count,
        "RustyClawRouter gateway started"
    );

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .with_graceful_shutdown(async move {
        let ctrl_c = tokio::signal::ctrl_c();

        #[cfg(unix)]
        {
            let mut sigterm =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("failed to install SIGTERM handler");
            tokio::select! {
                _ = ctrl_c => {
                    info!("received Ctrl+C, shutting down");
                }
                _ = sigterm.recv() => {
                    info!("received SIGTERM, shutting down");
                }
            }
        }

        #[cfg(not(unix))]
        {
            ctrl_c.await.expect("failed to listen for Ctrl+C");
            info!("received Ctrl+C, shutting down");
        }

        // Signal the claim processor (and any other background tasks) to stop
        let _ = shutdown_tx.send(true);
    })
    .await?;

    Ok(())
}

/// Apply all migrations from `migrations/001_initial_schema.sql`.
///
/// Uses `CREATE TABLE IF NOT EXISTS` and `CREATE INDEX IF NOT EXISTS` throughout,
/// so running this multiple times is safe (idempotent).
async fn run_migrations(pool: &sqlx::PgPool) -> anyhow::Result<()> {
    sqlx::raw_sql(gateway::usage::MIGRATION_SQL)
        .execute(pool)
        .await?;
    info!("database migrations applied");
    Ok(())
}

/// Redact credentials from a connection URL before logging.
///
/// Replaces everything between `://` and `@` (userinfo) with `[REDACTED]`
/// so that passwords are never written to log sinks.
///
/// # Examples
/// ```
/// assert_eq!(
///     redact_connection_url("postgres://rcr:secret@localhost:5432/db"),
///     "postgres://[REDACTED]@localhost:5432/db"
/// );
/// assert_eq!(
///     redact_connection_url("redis://localhost:6379"),
///     "redis://localhost:6379"
/// );
/// ```
fn redact_connection_url(url: &str) -> String {
    if let Some(at_pos) = url.find('@') {
        if let Some(scheme_end) = url.find("://") {
            return format!("{}://[REDACTED]@{}", &url[..scheme_end], &url[at_pos + 1..]);
        }
    }
    // No credentials found — safe to log as-is (e.g., redis://localhost:6379)
    url.to_string()
}

/// Generate a random 32-byte secret using two UUIDv4 values.
///
/// UUIDv4 provides 122 bits of randomness per call (backed by the OS CSPRNG),
/// so two calls give 244 bits — more than sufficient for HMAC-SHA256 keying.
fn generate_random_secret() -> Vec<u8> {
    let a = uuid::Uuid::new_v4();
    let b = uuid::Uuid::new_v4();
    let mut secret = Vec::with_capacity(32);
    secret.extend_from_slice(a.as_bytes());
    secret.extend_from_slice(b.as_bytes());
    secret
}
