use std::sync::Arc;

use tracing::{info, warn};

use gateway::{build_router, config, providers::ProviderRegistry, AppState};
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
        tracing::warn!(error = %e, "failed to parse services.toml, using empty registry");
        ServiceRegistry::empty()
    });
    info!(
        services = service_registry.all().len(),
        "loaded service registry"
    );

    // Initialize provider registry from environment API keys
    let providers = ProviderRegistry::from_env();
    let configured = providers.configured_providers();
    info!(
        providers = ?configured,
        "initialized provider registry"
    );

    // Initialize Solana payment verifier
    let solana_verifier = x402::solana::SolanaVerifier::new(
        &app_config.solana.rpc_url,
        &app_config.solana.recipient_wallet,
        &app_config.solana.usdc_mint,
    )
    .unwrap_or_else(|e| {
        tracing::warn!(error = %e, "failed to initialize Solana verifier, using default config");
        // Fallback: create with devnet defaults for development
        x402::solana::SolanaVerifier::new(
            "https://api.devnet.solana.com",
            "11111111111111111111111111111111",
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
        )
        .expect("default verifier config must be valid")
    });

    let facilitator = x402::facilitator::Facilitator::new(vec![Arc::new(solana_verifier)]);

    // Initialize usage tracker (no DB connection for now)
    let usage = gateway::usage::UsageTracker::noop();

    // Initialize response cache (optional — Redis may not be running)
    let cache = match gateway::cache::ResponseCache::new(
        "redis://127.0.0.1:6379",
        gateway::cache::CacheConfig::default(),
    ) {
        Ok(c) => {
            info!("response cache enabled (Redis)");
            Some(c)
        }
        Err(e) => {
            warn!(error = %e, "response cache disabled — Redis not available");
            None
        }
    };

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
        cache,
        provider_health,
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
