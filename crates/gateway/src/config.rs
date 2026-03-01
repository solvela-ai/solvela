use std::fmt;

use serde::Deserialize;

/// Top-level gateway configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub solana: SolanaConfig,
    pub providers: ProvidersConfig,
}

/// HTTP server settings.
#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    /// Host to bind to (default: "0.0.0.0").
    #[serde(default = "default_host")]
    pub host: String,
    /// Port to listen on (default: 8402).
    #[serde(default = "default_port")]
    pub port: u16,
}

/// Solana network settings.
#[derive(Debug, Clone, Deserialize)]
pub struct SolanaConfig {
    /// Solana RPC endpoint URL.
    pub rpc_url: String,
    /// The gateway's USDC recipient wallet address.
    pub recipient_wallet: String,
    /// USDC-SPL mint address.
    #[serde(default = "default_usdc_mint")]
    pub usdc_mint: String,
}

/// Provider API key configuration.
/// Keys come from environment variables, never config files.
///
/// `Debug` is intentionally NOT derived — API keys must never appear in
/// debug output, log lines, or panic messages.
#[derive(Clone, Deserialize)]
pub struct ProvidersConfig {
    #[serde(default)]
    pub openai_api_key: Option<String>,
    #[serde(default)]
    pub anthropic_api_key: Option<String>,
    #[serde(default)]
    pub google_api_key: Option<String>,
    #[serde(default)]
    pub xai_api_key: Option<String>,
    #[serde(default)]
    pub deepseek_api_key: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                host: default_host(),
                port: default_port(),
            },
            solana: SolanaConfig {
                rpc_url: "https://api.devnet.solana.com".to_string(),
                recipient_wallet: String::new(),
                usdc_mint: default_usdc_mint(),
            },
            providers: ProvidersConfig {
                openai_api_key: None,
                anthropic_api_key: None,
                google_api_key: None,
                xai_api_key: None,
                deepseek_api_key: None,
            },
        }
    }
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}

fn default_port() -> u16 {
    8402
}

fn default_usdc_mint() -> String {
    "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string()
}

/// Custom Debug impl for ProvidersConfig that redacts all API key values.
/// This ensures keys never appear in log output, panic messages, or debug traces.
impl fmt::Debug for ProvidersConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProvidersConfig")
            .field(
                "openai_api_key",
                &self.openai_api_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "anthropic_api_key",
                &self.anthropic_api_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "google_api_key",
                &self.google_api_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "xai_api_key",
                &self.xai_api_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "deepseek_api_key",
                &self.deepseek_api_key.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}
