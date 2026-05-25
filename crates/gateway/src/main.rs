use std::sync::Arc;

use anyhow::Context;
use tracing::{error, info, warn};

/// Read an environment variable with dual-prefix support.
///
/// Checks `SOLVELA_*` first, then falls back to the legacy `RCR_*` prefix.
/// Emits a deprecation warning via `tracing::warn!` when the old prefix is used.
fn env_with_fallback(solvela_name: &str, rcr_name: &str) -> Result<String, std::env::VarError> {
    if let Ok(val) = std::env::var(solvela_name) {
        return Ok(val);
    }
    match std::env::var(rcr_name) {
        Ok(val) => {
            tracing::warn!(
                old = rcr_name,
                new = solvela_name,
                "deprecated env var: use {solvela_name} instead of {rcr_name}"
            );
            Ok(val)
        }
        Err(e) => Err(e),
    }
}

use gateway::services::ServiceRegistry;
use gateway::{
    balance_monitor::BalanceMonitor,
    build_router, cache, config,
    middleware::rate_limit::{RateLimitConfig, RateLimiter},
    providers::ProviderRegistry,
    AppState,
};
use solvela_router::models::ModelRegistry;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load .env file if present (before any env var reads).
    // Existing env vars take precedence — .env values are not overwritten.
    dotenvy::dotenv().ok();

    // Initialize tracing — text by default, JSON when SOLVELA_LOG_FORMAT=json (or RCR_LOG_FORMAT)
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "gateway=info,tower_http=info".into());

    match env_with_fallback("SOLVELA_LOG_FORMAT", "RCR_LOG_FORMAT").as_deref() {
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
    // Supports SOLVELA_ prefix (preferred), RCR_ prefix (deprecated), and
    // double-underscore Fly.io convention (e.g., SOLVELA_SOLANA__RPC_URL).
    if let Ok(val) = env_with_fallback("SOLVELA_SOLANA__RPC_URL", "RCR_SOLANA__RPC_URL")
        .or_else(|_| env_with_fallback("SOLVELA_SOLANA_RPC_URL", "RCR_SOLANA_RPC_URL"))
    {
        app_config.solana.rpc_url = val;
    }
    if let Ok(val) = env_with_fallback(
        "SOLVELA_SOLANA__RECIPIENT_WALLET",
        "RCR_SOLANA__RECIPIENT_WALLET",
    )
    .or_else(|_| {
        env_with_fallback(
            "SOLVELA_SOLANA_RECIPIENT_WALLET",
            "RCR_SOLANA_RECIPIENT_WALLET",
        )
    }) {
        app_config.solana.recipient_wallet = val;
    }
    if let Ok(val) = env_with_fallback("SOLVELA_SOLANA__USDC_MINT", "RCR_SOLANA__USDC_MINT") {
        app_config.solana.usdc_mint = val;
    }
    if let Ok(val) = env_with_fallback(
        "SOLVELA_SOLANA__ESCROW_PROGRAM_ID",
        "RCR_SOLANA__ESCROW_PROGRAM_ID",
    )
    .or_else(|_| {
        env_with_fallback(
            "SOLVELA_SOLANA_ESCROW_PROGRAM_ID",
            "RCR_SOLANA_ESCROW_PROGRAM_ID",
        )
    }) {
        app_config.solana.escrow_program_id = Some(val);
    }
    if let Ok(val) = env_with_fallback("SOLVELA_SOLANA__FEE_PAYER_KEY", "RCR_SOLANA__FEE_PAYER_KEY")
        .or_else(|_| env_with_fallback("SOLVELA_SOLANA_FEE_PAYER_KEY", "RCR_SOLANA_FEE_PAYER_KEY"))
    {
        app_config.solana.fee_payer_key = Some(val);
    }
    // Server config overrides
    if let Ok(val) = env_with_fallback("SOLVELA_HOST", "RCR_HOST") {
        app_config.server.host = val;
    }
    if let Ok(val) = env_with_fallback("SOLVELA_PORT", "RCR_PORT") {
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
        env_with_fallback("SOLVELA_ENV", "RCR_ENV").as_deref(),
        Ok("production") | Ok("prod")
    );

    // ── Durable nonce pool ────────────────────────────────────────────────────
    //
    // Build a NoncePool from environment variables. Returns an empty pool
    // (not an error) if no nonce accounts are configured. When unconfigured
    // the SolanaVerifier rejects all client-submitted durable-nonce
    // transactions (see #173 L3 GW) — clients must use a recent blockhash
    // instead. Constructed before SolanaVerifier so we can plumb it in via
    // `with_nonce_pool` below; also reused by EscrowClaimer.
    let nonce_pool = {
        let pool = solvela_x402::nonce_pool::NoncePool::from_env();
        if pool.is_empty() {
            warn!(
                "nonce_pool not configured — client durable-nonce transactions \
                 will be REJECTED. Set SOLVELA_SOLANA__NONCE_ACCOUNT[_N] to \
                 allowlist nonce accounts, or instruct clients to use a recent \
                 blockhash."
            );
            None
        } else {
            info!(accounts = pool.len(), "durable nonce pool initialized");
            Some(Arc::new(pool))
        }
    };

    let solana_verifier = match solvela_x402::solana::SolanaVerifier::new(
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
                         Set SOLVELA_SOLANA_RPC_URL to a mainnet-beta endpoint."
                    );
                }
                warn!(
                    "⚠ Solana RPC URL points to devnet — payments will use devnet. \
                     Set SOLVELA_SOLANA_RPC_URL for mainnet-beta in production."
                );
            }
            v
        }
        Err(e) => {
            if is_production {
                panic!(
                    "FATAL: Failed to initialize Solana verifier in production: {e}. \
                     Set SOLVELA_SOLANA_RPC_URL and SOLVELA_SOLANA_RECIPIENT_WALLET."
                );
            }
            warn!(
                error = %e,
                "failed to initialize Solana verifier, using devnet defaults. \
                 THIS IS UNSAFE FOR PRODUCTION — set SOLVELA_ENV=production to enforce."
            );
            solvela_x402::solana::SolanaVerifier::new(
                "https://api.devnet.solana.com",
                "11111111111111111111111111111111",
                "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
                http_client.clone(),
            )
            .expect("default verifier config must be valid")
        }
    };

    // Attach the durable-nonce allowlist to the verifier so client-submitted
    // nonce transactions can be checked against operator-approved accounts.
    // Without this, the verifier rejects every nonce-based payment (#173 L3 GW).
    let solana_verifier = if let Some(pool) = &nonce_pool {
        solana_verifier.with_nonce_pool(Arc::clone(pool))
    } else {
        solana_verifier
    };

    // Build verifiers list — always include the direct SolanaVerifier
    let mut verifiers: Vec<Arc<dyn solvela_x402::traits::PaymentVerifier>> =
        vec![Arc::new(solana_verifier)];

    // ── Fee payer pool (hot wallet rotation) ──────────────────────────────────
    //
    // Build a FeePayerPool from all configured fee payer keys.
    // Falls back to env-var scanning (SOLVELA_SOLANA__FEE_PAYER_KEY[_N] or RCR_SOLANA__FEE_PAYER_KEY[_N]) when the
    // merged config list is empty.
    // Constructed before EscrowClaimer because the claimer now uses the pool.
    let fee_payer_pool = {
        let merged_keys = app_config.solana.all_fee_payer_keys();
        if merged_keys.is_empty() {
            // Try loading directly from env vars as a fallback
            match solvela_x402::fee_payer::FeePayerPool::from_env() {
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
            match solvela_x402::fee_payer::FeePayerPool::from_keys(&merged_keys) {
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

    // Conditionally build EscrowVerifier + EscrowClaimer if configured.
    // The claimer now uses FeePayerPool for rotation and optionally NoncePool
    // for durable nonces.
    let escrow_claimer = if let (Some(prog_id), Some(ref pool)) =
        (&app_config.solana.escrow_program_id, &fee_payer_pool)
    {
        let escrow_verifier = solvela_x402::escrow::EscrowVerifier {
            rpc_url: app_config.solana.rpc_url.clone(),
            recipient_wallet: app_config.solana.recipient_wallet.clone(),
            usdc_mint: app_config.solana.usdc_mint.clone(),
            escrow_program_id: prog_id.clone(),
            http_client: http_client.clone(),
        };
        verifiers.push(Arc::new(escrow_verifier));

        match solvela_x402::escrow::EscrowClaimer::new(
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
            info!("escrow payment mode disabled (set SOLVELA_SOLANA_ESCROW_PROGRAM_ID + SOLVELA_SOLANA_FEE_PAYER_KEY to enable)");
        }
        None
    };

    let facilitator = solvela_x402::facilitator::Facilitator::new(verifiers);

    // ── PostgreSQL connection (optional — gracefully degrades to noop) ──────────
    //
    // Set DATABASE_URL to enable persistent spend logging and wallet budgets.
    // Without it, requests still work but spend data is only logged to stdout.
    let max_connections: u32 =
        env_with_fallback("SOLVELA_DB_MAX_CONNECTIONS", "RCR_DB_MAX_CONNECTIONS")
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
                    // Run migrations on startup. If they fail, refuse to boot —
                    // serving traffic against a broken schema would produce
                    // silent query errors across every org / audit / budget path.
                    run_migrations(&pool).await?;
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

    // ── Semantic cache (Tier 2) — opt-in via [cache.semantic].enabled ──────────
    //
    // Requires Redis-with-RediSearch and the local embedding model. Degrades
    // gracefully to `None` (Tier 1 exact cache still works) on any failure —
    // consistent with rule #12 (both Redis and PostgreSQL are optional).
    // Operator misconfiguration of the semantic cache is fatal — fail fast
    // rather than silently disabling the tier (which `build_semantic_cache`
    // does for genuine infra failures like Redis being down).
    if let Err(e) = app_config.cache.semantic.validate() {
        anyhow::bail!("invalid [cache.semantic] configuration: {e}");
    }
    let semantic_cache = build_semantic_cache(&app_config, redis_url.as_str()).await;

    // Initialize provider health tracker
    let provider_health = gateway::providers::health::ProviderHealthTracker::new(
        gateway::providers::health::CircuitBreakerConfig::default(),
    );

    // ── Escrow metrics (in-memory atomic counters) ─────────────────────────
    //
    // Created here so both the claim processor and the AppState can share
    // the same Arc.  `None` when escrow is not configured or no DB.
    let escrow_metrics: Option<Arc<solvela_x402::escrow::EscrowMetrics>> =
        if escrow_claimer.is_some() && db_pool.is_some() {
            Some(Arc::new(solvela_x402::escrow::EscrowMetrics::new()))
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

    // Read admin token once at startup, wrap in the secret-safe newtype so
    // it is redacted from Debug output and zeroized on drop (see #173 L4 GW).
    let admin_token = env_with_fallback("SOLVELA_ADMIN_TOKEN", "RCR_ADMIN_TOKEN")
        .ok()
        .filter(|t| !t.is_empty())
        .map(gateway::secret::AdminToken::new);

    // Read the API-key HMAC keying secret. When set, new API keys are stored
    // under HMAC-SHA256(secret, key) instead of plain SHA-256(key). Defense
    // against database-dump offline brute-force — without the secret, an
    // attacker holding `api_keys.key_hash` cannot verify candidate keys
    // (see #173 L1 GW). Optional for back-compat; gateway emits a startup
    // warn when unset.
    let api_key_hmac_secret =
        env_with_fallback("SOLVELA_API_KEY_HMAC_SECRET", "RCR_API_KEY_HMAC_SECRET")
            .ok()
            .filter(|s| !s.is_empty())
            .map(|s| Arc::new(gateway::secret::HmacSecret::new(s.into_bytes())));
    if api_key_hmac_secret.is_none() {
        warn!(
            "SOLVELA_API_KEY_HMAC_SECRET is not set — API keys are hashed with \
             plain SHA-256 (no keying secret). Set this env var to a long, \
             random value to harden the api_keys.key_hash column against \
             offline brute-force from a database dump. See #173 L1 GW."
        );
    }

    // Dev-mode payment bypass — only when SOLVELA_DEV_BYPASS_PAYMENT=true AND not production
    let dev_bypass_payment = if is_production {
        if env_with_fallback("SOLVELA_DEV_BYPASS_PAYMENT", "RCR_DEV_BYPASS_PAYMENT").as_deref()
            == Ok("true")
        {
            warn!("SOLVELA_DEV_BYPASS_PAYMENT is set but ignored — payment bypass is NEVER allowed in production");
        }
        false
    } else {
        let enabled = env_with_fallback("SOLVELA_DEV_BYPASS_PAYMENT", "RCR_DEV_BYPASS_PAYMENT")
            .as_deref()
            == Ok("true");
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
        semantic_cache,
        provider_health,
        escrow_claimer,
        fee_payer_pool,
        nonce_pool,
        db_pool,
        escrow_metrics: escrow_metrics.clone(),
        admin_token,
        api_key_hmac_secret,
        session_secret: {
            let secret = match env_with_fallback("SOLVELA_SESSION_SECRET", "RCR_SESSION_SECRET") {
                Ok(b64) => {
                    use base64::Engine;
                    base64::engine::general_purpose::STANDARD
                        .decode(&b64)
                        .unwrap_or_else(|e| {
                            warn!(error = %e, "invalid SOLVELA_SESSION_SECRET base64 — generating random secret");
                            generate_random_secret()
                        })
                }
                Err(_) => {
                    warn!("SOLVELA_SESSION_SECRET not set — generating ephemeral session secret (sessions will not survive restarts)");
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
        let _handle = solvela_x402::escrow::claim_processor::start_claim_processor(
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

            let monitor = Arc::new(
                BalanceMonitor::new(monitor_config, wallet_pubkeys)
                    .context("failed to build balance monitor HTTP client")?,
            );
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
    let rate_limit_config = match env_with_fallback("SOLVELA_RATE_LIMIT_MAX", "RCR_RATE_LIMIT_MAX")
    {
        Ok(val) => {
            let max: u32 = val.parse().unwrap_or_else(|_| {
                tracing::warn!(value = %val, "Invalid SOLVELA_RATE_LIMIT_MAX, using default");
                60
            });
            tracing::info!(max_requests = max, "Rate limit override from env");
            RateLimitConfig::with_max_requests(max)
        }
        Err(_) => RateLimitConfig::default(),
    };
    let rate_limiter = RateLimiter::new(rate_limit_config);

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

    info!(addr, models = model_count, "Solvela gateway started");

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

/// Apply every migration file in `../../migrations/` in version order.
///
/// Uses `sqlx::migrate!`, which embeds the migration SQL at compile time and
/// tracks applied versions in a `_sqlx_migrations` table. Order is determined
/// by the numeric version prefix on each filename (`001_…`, `002_…`). Safe
/// to run on every startup — sqlx skips migrations that have already been
/// applied.
async fn run_migrations(pool: &sqlx::PgPool) -> anyhow::Result<()> {
    sqlx::migrate!("../../migrations").run(pool).await?;
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

/// Build the Tier 2 semantic cache if `[cache.semantic].enabled`.
///
/// Returns `None` (with a logged warning) on any failure — a missing model or a
/// Redis without RediSearch must not take the gateway down; the exact cache and
/// the rest of the pipeline keep working. The embedding model is loaded on a
/// blocking thread (the load is CPU-bound and can take ~1s).
async fn build_semantic_cache(
    config: &config::AppConfig,
    redis_url: &str,
) -> Option<Arc<cache::semantic::SemanticCache>> {
    use cache::embedder::LocalBge;
    use cache::semantic::{SemanticCache, SemanticConfig};

    let settings = &config.cache.semantic;
    if !settings.enabled {
        return None;
    }

    let model_dir = settings.model_cache_dir.clone();
    let embedder = match tokio::task::spawn_blocking(move || match model_dir {
        Some(dir) => LocalBge::with_cache_dir(dir),
        None => LocalBge::new(),
    })
    .await
    {
        Ok(Ok(e)) => e,
        // `error!` + counter, not `warn!`: the operator explicitly opted in
        // to the semantic tier (`[cache.semantic].enabled = true`). Silently
        // degrading to all-miss on a corrupt/missing model is the same failure
        // mode that caused the original "silent off" gap — ops sees zero hit
        // rate, no alert. Failure here is loud, and the `reason` label lets
        // alerting separate "model file gone" from "task panic".
        Ok(Err(e)) => {
            metrics::counter!(
                SEMANTIC_CACHE_STARTUP_FAILURE_METRIC,
                "reason" => STARTUP_FAILURE_REASON_MODEL_LOAD
            )
            .increment(1);
            error!(error = %e, "semantic cache disabled: embedding model failed to load");
            return None;
        }
        Err(e) => {
            metrics::counter!(
                SEMANTIC_CACHE_STARTUP_FAILURE_METRIC,
                "reason" => STARTUP_FAILURE_REASON_TASK_PANIC
            )
            .increment(1);
            error!(error = %e, "semantic cache disabled: embedder load task panicked");
            return None;
        }
    };

    let sem_config = SemanticConfig {
        enabled: true,
        threshold: settings.threshold,
        ttl_secs: settings.ttl_secs,
    };

    match SemanticCache::connect(redis_url, Arc::new(embedder), sem_config).await {
        Ok(cache) => {
            info!(
                threshold = settings.threshold,
                hit_price_percent = settings.hit_price_percent,
                "semantic cache enabled"
            );
            Some(Arc::new(cache))
        }
        Err(e) => {
            metrics::counter!(
                SEMANTIC_CACHE_STARTUP_FAILURE_METRIC,
                "reason" => STARTUP_FAILURE_REASON_REDIS_CONNECT
            )
            .increment(1);
            error!(error = %e, "semantic cache disabled: Redis/RediSearch unavailable");
            None
        }
    }
}

/// Metric and label-value constants for `build_semantic_cache` startup
/// failures. Production and tests reference these same consts so a typo can
/// only happen in one place — keeps alerting and the test counter assertions
/// from silently drifting apart.
const SEMANTIC_CACHE_STARTUP_FAILURE_METRIC: &str = "solvela_semantic_cache_startup_failure_total";
const STARTUP_FAILURE_REASON_MODEL_LOAD: &str = "model_load";
const STARTUP_FAILURE_REASON_TASK_PANIC: &str = "task_panic";
const STARTUP_FAILURE_REASON_REDIS_CONNECT: &str = "redis_connect";

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::OnceLock;

    /// Process-wide Prometheus recorder for counter assertions. Mirrors the
    /// pattern in `cache::exact::tests` — `install_recorder` can only succeed
    /// once per process, so the OnceLock makes repeated calls idempotent.
    fn install_test_recorder() -> metrics_exporter_prometheus::PrometheusHandle {
        static HANDLE: OnceLock<metrics_exporter_prometheus::PrometheusHandle> = OnceLock::new();
        HANDLE
            .get_or_init(|| {
                metrics_exporter_prometheus::PrometheusBuilder::new()
                    .install_recorder()
                    .expect("failed to install test Prometheus recorder")
            })
            .clone()
    }

    fn counter_sum(
        handle: &metrics_exporter_prometheus::PrometheusHandle,
        name: &str,
        label_match: &str,
    ) -> u64 {
        let body = handle.render();
        body.lines()
            .filter(|l| l.starts_with(name) && l.contains(label_match))
            .filter_map(|l| l.rsplit_once(' ').and_then(|(_, v)| v.parse::<u64>().ok()))
            .sum()
    }

    /// F1 startup-failure counter must actually increment when
    /// `build_semantic_cache` cannot reach Redis. A typo in the metric name or
    /// a refactor that drops the `metrics::counter!` call would make alerting
    /// silently lose this signal — this test catches both. References the
    /// production consts directly so they cannot drift apart.
    #[tokio::test]
    async fn build_semantic_cache_increments_failure_counter_on_unreachable_redis() {
        let handle = install_test_recorder();
        let label_match = format!("reason=\"{}\"", STARTUP_FAILURE_REASON_REDIS_CONNECT);
        let before = counter_sum(&handle, SEMANTIC_CACHE_STARTUP_FAILURE_METRIC, &label_match);

        let mut config = config::AppConfig::default();
        config.cache.semantic.enabled = true;
        // Point at a closed port so connect() must fail. We also try to skip
        // out if the model isn't available, because build_semantic_cache loads
        // the embedder before touching Redis — without the model the test
        // would short-circuit on a different failure mode.
        if !std::path::Path::new(
            &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.fastembed_cache"),
        )
        .exists()
        {
            eprintln!("skipping: fastembed cache not present");
            return;
        }
        config.cache.semantic.model_cache_dir = Some(
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../.fastembed_cache")
                .to_string_lossy()
                .into_owned(),
        );

        let result = build_semantic_cache(&config, "redis://127.0.0.1:1").await;
        assert!(
            result.is_none(),
            "build_semantic_cache must return None on Redis failure"
        );

        let after = counter_sum(&handle, SEMANTIC_CACHE_STARTUP_FAILURE_METRIC, &label_match);
        assert_eq!(
            after,
            before + 1,
            "redis_connect failure path must increment the startup-failure counter; \
             a typo in the metric name or a missing counter! call would slip past this test"
        );
    }

    /// Lock the exhaustive set of reason labels for the startup-failure
    /// counter. The production code emits exactly these three values; if a new
    /// failure branch is added without extending this list, alert rules built
    /// against the known set will silently miss the new reason. Acts as a
    /// schema test for the operator-facing metric.
    #[test]
    fn startup_failure_reason_labels_are_the_expected_three() {
        let reasons = [
            STARTUP_FAILURE_REASON_MODEL_LOAD,
            STARTUP_FAILURE_REASON_TASK_PANIC,
            STARTUP_FAILURE_REASON_REDIS_CONNECT,
        ];
        assert_eq!(reasons, ["model_load", "task_panic", "redis_connect"]);
        assert_eq!(
            SEMANTIC_CACHE_STARTUP_FAILURE_METRIC,
            "solvela_semantic_cache_startup_failure_total"
        );
    }
}
