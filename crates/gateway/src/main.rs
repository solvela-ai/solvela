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
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "gateway=info,tower_http=info".into()),
        )
        .init();

    // Load configuration
    let app_config = config::AppConfig::default();

    // Load model registry from config file
    let models_toml = std::fs::read_to_string("config/models.toml")
        .unwrap_or_else(|_| include_str!("../../../config/models.toml").to_string());
    let model_registry = ModelRegistry::from_toml(&models_toml)?;

    let model_count = model_registry.all().len();

    // Load service registry from config file
    let services_toml = std::fs::read_to_string("config/services.toml")
        .unwrap_or_else(|_| include_str!("../../../config/services.toml").to_string());
    let service_registry = ServiceRegistry::from_toml(&services_toml).unwrap_or_else(|e| {
        warn!(error = %e, "failed to parse services.toml, using empty registry");
        ServiceRegistry::empty()
    });
    info!(
        services = service_registry.all().len(),
        "loaded service registry"
    );

    // Initialize provider registry from environment API keys
    let providers = ProviderRegistry::from_env();
    let configured = providers.configured_providers();
    info!(providers = ?configured, "initialized provider registry");

    // Initialize Solana payment verifier
    let solana_verifier = x402::solana::SolanaVerifier::new(
        &app_config.solana.rpc_url,
        &app_config.solana.recipient_wallet,
        &app_config.solana.usdc_mint,
    )
    .unwrap_or_else(|e| {
        warn!(error = %e, "failed to initialize Solana verifier, using devnet defaults");
        x402::solana::SolanaVerifier::new(
            "https://api.devnet.solana.com",
            "11111111111111111111111111111111",
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
        )
        .expect("default verifier config must be valid")
    });

    // Build verifiers list — always include the direct SolanaVerifier
    let mut verifiers: Vec<Arc<dyn x402::traits::PaymentVerifier>> =
        vec![Arc::new(solana_verifier)];

    // Conditionally build EscrowVerifier + EscrowClaimer if configured
    let escrow_claimer = if let (Some(prog_id), Some(fee_payer_key)) = (
        &app_config.solana.escrow_program_id,
        &app_config.solana.fee_payer_key,
    ) {
        let escrow_verifier = x402::escrow::EscrowVerifier {
            rpc_url: app_config.solana.rpc_url.clone(),
            recipient_wallet: app_config.solana.recipient_wallet.clone(),
            usdc_mint: app_config.solana.usdc_mint.clone(),
            escrow_program_id: prog_id.clone(),
            http_client: reqwest::Client::new(),
        };
        verifiers.push(Arc::new(escrow_verifier));

        match x402::escrow::EscrowClaimer::new(
            app_config.solana.rpc_url.clone(),
            fee_payer_key,
            prog_id,
            &app_config.solana.recipient_wallet,
            &app_config.solana.usdc_mint,
        ) {
            Ok(claimer) => {
                info!("escrow payment mode enabled");
                Some(Arc::new(claimer))
            }
            Err(e) => {
                warn!(error = %e, "failed to init EscrowClaimer — escrow disabled");
                None
            }
        }
    } else {
        info!("escrow payment mode disabled (set RCR_SOLANA_ESCROW_PROGRAM_ID + RCR_SOLANA_FEE_PAYER_KEY to enable)");
        None
    };

    let facilitator = x402::facilitator::Facilitator::new(verifiers);

    // ── PostgreSQL connection (optional — gracefully degrades to noop) ──────────
    //
    // Set DATABASE_URL to enable persistent spend logging and wallet budgets.
    // Without it, requests still work but spend data is only logged to stdout.
    let db_pool = match std::env::var("DATABASE_URL") {
        Ok(url) => match sqlx::PgPool::connect(&url).await {
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
        },
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

    // ── Fee payer pool (hot wallet rotation) ──────────────────────────────────
    //
    // Build a FeePayerPool from all configured fee payer keys.
    // Falls back to env-var scanning (RCR_SOLANA__FEE_PAYER_KEY[_N]) when the
    // merged config list is empty.
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
        session_secret: match std::env::var("RCR_SESSION_SECRET") {
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
                info!("RCR_SESSION_SECRET not set — generating ephemeral session secret");
                generate_random_secret()
            }
        },
    });

    // ── Claim processor (durable escrow claim background task) ──────────────
    //
    // If both PostgreSQL and an EscrowClaimer are available, start the
    // background claim processor that polls the escrow_claim_queue table.
    if let (Some(ref pool), Some(ref claimer)) = (&state.db_pool, &state.escrow_claimer) {
        let _handle = x402::escrow::claim_processor::start_claim_processor(
            pool.clone(),
            Arc::clone(claimer),
            std::time::Duration::from_secs(10),
        );
        info!("background escrow claim processor started (10s poll interval)");
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
            drop(BalanceMonitor::spawn(monitor));
        }
    }

    // ── Rate limiter + periodic cleanup ───────────────────────────────────────
    let rate_limiter = RateLimiter::new(RateLimitConfig::default());

    // Spawn periodic cleanup task — removes expired entries every 60 seconds
    // to prevent the in-memory HashMap from growing without bound.
    let rl_clone = rate_limiter.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
        loop {
            interval.tick().await;
            rl_clone.cleanup().await;
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

    axum::serve(listener, app).await?;

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
