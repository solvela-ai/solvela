use solvela_client_proxy::proxy;

use std::net::SocketAddr;
use std::sync::Arc;

use clap::Parser;
use tracing::{error, info};

use solvela_client::{ClientConfig, SolvelaClient};
use solvela_client_cli_args::{load_wallet, GatewayArgs, RpcArgs, WalletArgs};

/// Localhost reverse proxy with transparent x402 USDC-SPL payment handling.
///
/// Any OpenAI-compatible client can point at this proxy and get automatic
/// Solana payment handling — no code changes needed.
#[derive(Parser, Debug)]
#[command(name = "solvela-client-proxy", version)]
struct Cli {
    #[command(flatten)]
    wallet: WalletArgs,

    #[command(flatten)]
    gateway: GatewayArgs,

    #[command(flatten)]
    rpc: RpcArgs,

    /// Port to listen on (binds 127.0.0.1 only).
    #[arg(short, long, default_value_t = 8402)]
    port: u16,

    /// Maximum payment per request in USDC (e.g., 0.10).
    #[arg(long)]
    max_payment: Option<f64>,

    /// Expected gateway recipient wallet address (security: reject mismatches).
    #[arg(long)]
    expected_recipient: Option<String>,

    /// Enable debug logging.
    #[arg(short, long)]
    verbose: bool,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // Initialize tracing
    let filter = if cli.verbose {
        "solvela_client_proxy=debug,solvela_client=debug,tower_http=debug"
    } else {
        "solvela_client_proxy=info,tower_http=info"
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| filter.into()),
        )
        .init();

    // Load wallet: env var takes priority, then file
    let wallet = match load_wallet(&cli.wallet) {
        Ok(w) => w,
        Err(e) => {
            error!("failed to load wallet: {e}");
            eprintln!("Error: {e}");
            eprintln!();
            eprintln!("Provide a wallet via:");
            eprintln!(
                "  1. Set {} env var with base58-encoded keypair",
                cli.wallet.wallet_env
            );
            eprintln!("  2. Create wallet file at {}", cli.wallet.wallet_file);
            std::process::exit(1);
        }
    };

    info!(wallet = %wallet.address(), "wallet loaded");

    // Build client config
    let gateway_url = cli.gateway.gateway.trim_end_matches('/').to_string();
    let mut config = ClientConfig {
        gateway_url: gateway_url.clone(),
        rpc_url: cli.rpc.rpc_url,
        ..ClientConfig::default()
    };

    if let Some(max_usdc) = cli.max_payment {
        // Convert USDC to atomic units (1 USDC = 1_000_000 atomic)
        // Round to avoid floating-point truncation (e.g., 0.10 * 1e6 = 99999.99...)
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let atomic = (max_usdc * 1_000_000.0).round() as u64;
        config.max_payment_amount = Some(atomic);
        info!(max_usdc = %max_usdc, max_atomic = atomic, "payment cap enabled");
    }

    if let Some(ref recipient) = cli.expected_recipient {
        config.expected_recipient = Some(recipient.clone());
        info!(recipient = %recipient, "recipient validation enabled");
    }

    let client = match SolvelaClient::new(wallet, config) {
        Ok(c) => c,
        Err(e) => {
            error!("failed to create client: {e}");
            std::process::exit(1);
        }
    };

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_mins(2))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    let state = Arc::new(proxy::ProxyState {
        client,
        gateway_url,
        http,
    });

    let app = proxy::build_proxy_router(state);

    let addr = SocketAddr::from(([127, 0, 0, 1], cli.port));
    info!(
        addr = %addr,
        "solvela-client-proxy started — forwarding to gateway"
    );

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .unwrap_or_else(|e| {
            error!(addr = %addr, error = %e, "failed to bind");
            std::process::exit(1);
        });

    axum::serve(listener, app).await.unwrap_or_else(|e| {
        error!(error = %e, "server error");
        std::process::exit(1);
    });
}
