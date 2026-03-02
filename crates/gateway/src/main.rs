use std::sync::Arc;

use tracing::{info, warn};

use gateway::{build_router, cache, config, providers::ProviderRegistry, AppState};
use rcr_common::services::ServiceRegistry;
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
                warn!(error = %e, url = %url, "PostgreSQL connection failed — spend logging disabled");
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
                    warn!(error = %e, url = %redis_url, "Redis connection failed — cache disabled");
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
    let usage = gateway::usage::UsageTracker::new(db_pool, redis_client.clone());

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
    });

    // Build router
    let app = build_router(state);

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
