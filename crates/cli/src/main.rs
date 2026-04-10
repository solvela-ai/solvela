use anyhow::Result;
use clap::{Parser, Subcommand};

mod commands;

/// Global mutex serializing tests that mutate the HOME environment variable.
/// All test modules that call `std::env::set_var("HOME", ...)` must hold this
/// lock for their entire test duration to prevent races in parallel test runs.
#[cfg(test)]
pub(crate) static ENV_MUTEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
        /// Skip the cost confirmation prompt (for scripted use)
        #[arg(short, long)]
        yes: bool,
        /// Force a specific payment scheme: "exact" or "escrow" (default: prefer escrow)
        #[arg(long)]
        scheme: Option<String>,
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
    /// Load test the gateway with configurable concurrency and payment modes
    Loadtest(commands::loadtest::LoadTestArgs),
    /// Recover stranded escrow deposits (refund expired PDAs)
    Recover {
        /// Submit refund transactions (default is dry-run list)
        #[arg(long)]
        execute: bool,
        /// Skip the confirmation prompt (requires --execute)
        #[arg(short, long)]
        yes: bool,
        /// Override escrow program ID (defaults to mainnet)
        #[arg(long)]
        program_id: Option<String>,
    },
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
        Commands::Chat {
            prompt,
            model,
            yes,
            scheme,
        } => commands::chat::run(&cli.api_url, &model, &prompt, yes, scheme.as_deref()).await?,
        Commands::Stats { days } => commands::stats::show(&cli.api_url, days).await?,
        Commands::Health => commands::health::check(&cli.api_url).await?,
        Commands::Doctor => commands::doctor::run(&cli.api_url).await?,
        Commands::Loadtest(args) => commands::loadtest::run(&cli.api_url, args).await?,
        Commands::Recover {
            execute,
            yes,
            program_id,
        } => commands::recover::run(&cli.api_url, execute, yes, program_id).await?,
    }

    Ok(())
}
