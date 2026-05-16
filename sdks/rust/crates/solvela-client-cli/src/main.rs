use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use solvela_client_cli_args::{GatewayArgs, RpcArgs, WalletArgs};

mod commands;

/// solvela-client — agent-side payer CLI for Solana-paid AI from your terminal.
#[derive(Debug, Parser)]
#[command(name = "solvela-client", version, about, long_about = None)]
struct Cli {
    #[command(flatten)]
    gateway: GatewayArgs,

    #[command(flatten)]
    rpc: RpcArgs,

    #[command(flatten)]
    wallet: WalletArgs,

    /// Enable verbose logging output.
    #[arg(long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Manage your Solana wallet (create, import, balance, address, export).
    Wallet {
        #[command(subcommand)]
        action: WalletAction,
    },
    /// Send a chat completion request via the gateway.
    Chat {
        /// The prompt message to send.
        prompt: String,

        /// Model to use (e.g. "openai/gpt-4o", "sonnet", "auto").
        #[arg(short, long, default_value = "auto")]
        model: String,

        /// Disable streaming (receive the full response at once).
        #[arg(long)]
        no_stream: bool,

        /// Maximum payment in atomic USDC units (safety cap).
        #[arg(long)]
        max_payment: Option<u64>,
    },
    /// List available models from the gateway.
    Models {
        /// Filter by provider name.
        #[arg(short, long)]
        provider: Option<String>,

        /// Output as JSON instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Check connectivity and configuration.
    Doctor,
}

#[derive(Debug, Subcommand)]
enum WalletAction {
    /// Create a new wallet with a fresh keypair and mnemonic.
    Create {
        /// Overwrite existing wallet file.
        #[arg(long)]
        force: bool,
    },
    /// Import a wallet from a mnemonic phrase or base58 keypair.
    Import {
        /// Import from a BIP39 mnemonic phrase (prompted on stdin).
        #[arg(long, group = "import_source")]
        mnemonic: bool,

        /// Import from a base58-encoded keypair (prompted on stdin).
        #[arg(long, group = "import_source")]
        keypair: bool,

        /// Overwrite existing wallet file.
        #[arg(long)]
        force: bool,
    },
    /// Show the USDC-SPL balance of your wallet.
    Balance,
    /// Print your wallet's public address.
    Address,
    /// Export your private keypair as base58 (dangerous!).
    Export {
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
}

fn init_tracing(verbose: bool) {
    let filter = if verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"))
    };

    tracing_subscriber::fmt().with_env_filter(filter).init();
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    let result = match cli.command {
        Commands::Wallet { action } => {
            commands::wallet::run(action, &cli.wallet, &cli.gateway, &cli.rpc).await
        }
        Commands::Chat {
            prompt,
            model,
            no_stream,
            max_payment,
        } => {
            commands::chat::run(
                &prompt,
                &model,
                no_stream,
                max_payment,
                &cli.wallet,
                &cli.gateway,
                &cli.rpc,
            )
            .await
        }
        Commands::Models { provider, json } => {
            commands::models::run(provider.as_deref(), json, &cli.gateway).await
        }
        Commands::Doctor => commands::doctor::run(&cli.wallet, &cli.gateway, &cli.rpc).await,
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
