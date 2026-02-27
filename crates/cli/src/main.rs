use anyhow::Result;
use clap::{Parser, Subcommand};

mod commands;

#[derive(Parser)]
#[command(name = "rcr")]
#[command(about = "RustyClawRouter CLI — AI agent payments with USDC on Solana")]
#[command(version)]
struct Cli {
    /// Gateway API URL
    #[arg(long, env = "RCR_API_URL", default_value = "http://localhost:8402")]
    api_url: String,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Wallet management
    Wallet {
        #[command(subcommand)]
        action: WalletAction,
    },
    /// List available models and pricing
    Models,
    /// Quick chat with auto-routing
    Chat {
        /// The prompt to send
        prompt: String,
        /// Model or routing profile (auto, eco, premium, free)
        #[arg(short, long, default_value = "auto")]
        model: String,
    },
    /// Usage statistics
    Stats {
        /// Number of days to show
        #[arg(long, default_value = "7")]
        days: u32,
    },
    /// Health check
    Health,
    /// AI-powered diagnostics
    Doctor,
}

#[derive(Subcommand)]
enum WalletAction {
    /// Generate a new Solana keypair
    Init,
    /// Show wallet address and USDC balance
    Status,
    /// Export private key (base58)
    Export,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Wallet { action } => match action {
            WalletAction::Init => commands::wallet::init().await?,
            WalletAction::Status => commands::wallet::status(&cli.api_url).await?,
            WalletAction::Export => commands::wallet::export()?,
        },
        Commands::Models => commands::models::list(&cli.api_url).await?,
        Commands::Chat { prompt, model } => {
            commands::chat::run(&cli.api_url, &model, &prompt).await?
        }
        Commands::Stats { days } => commands::stats::show(&cli.api_url, days).await?,
        Commands::Health => commands::health::check(&cli.api_url).await?,
        Commands::Doctor => commands::doctor::run(&cli.api_url).await?,
    }

    Ok(())
}
