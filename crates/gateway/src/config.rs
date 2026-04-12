use std::fmt;

use serde::Deserialize;

use crate::balance_monitor::MonitorConfig;

/// Top-level gateway configuration.
#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub solana: SolanaConfig,
    pub providers: ProvidersConfig,
    /// Balance monitoring configuration.
    #[serde(default)]
    pub monitor: MonitorConfig,
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
///
/// `Debug` is manually implemented to redact `fee_payer_key` and
/// `fee_payer_keys` — ed25519 private keys that must never appear in log
/// output or panic messages.
#[derive(Clone, Deserialize)]
pub struct SolanaConfig {
    /// Solana RPC endpoint URL.
    pub rpc_url: String,
    /// The gateway's USDC recipient wallet address.
    pub recipient_wallet: String,
    /// USDC-SPL mint address.
    #[serde(default = "default_usdc_mint")]
    pub usdc_mint: String,
    /// Escrow program ID (base58). Set to enable escrow payment mode.
    #[serde(default)]
    pub escrow_program_id: Option<String>,
    /// Hot wallet private key (base58, 64 bytes) for signing claim transactions.
    /// Backward-compatible: merged into the fee payer pool at index 0.
    #[serde(default)]
    pub fee_payer_key: Option<String>,
    /// Additional hot wallet keys for rotation (base58, 64 bytes each).
    /// Loaded from `SOLVELA_SOLANA__FEE_PAYER_KEY_2` .. `_8` (or `RCR_SOLANA__FEE_PAYER_KEY_2` .. `_8`) env vars.
    #[serde(default)]
    pub fee_payer_keys: Vec<String>,
}

impl SolanaConfig {
    /// Merge the legacy `fee_payer_key` with the `fee_payer_keys` vec into a
    /// single ordered list. The primary key is always at index 0.
    pub fn all_fee_payer_keys(&self) -> Vec<String> {
        let mut keys = Vec::new();
        if let Some(ref primary) = self.fee_payer_key {
            if !primary.is_empty() {
                keys.push(primary.clone());
            }
        }
        for k in &self.fee_payer_keys {
            if !k.is_empty() {
                keys.push(k.clone());
            }
        }
        keys
    }
}

impl fmt::Debug for SolanaConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SolanaConfig")
            .field("rpc_url", &self.rpc_url)
            .field("recipient_wallet", &self.recipient_wallet)
            .field("usdc_mint", &self.usdc_mint)
            .field("escrow_program_id", &self.escrow_program_id)
            .field(
                "fee_payer_key",
                &self.fee_payer_key.as_ref().map(|_| "[REDACTED]"),
            )
            .field(
                "fee_payer_keys",
                &format!("[{} keys REDACTED]", self.fee_payer_keys.len()),
            )
            .finish()
    }
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
                escrow_program_id: None,
                fee_payer_key: None,
                fee_payer_keys: Vec::new(),
            },
            providers: ProvidersConfig {
                openai_api_key: None,
                anthropic_api_key: None,
                google_api_key: None,
                xai_api_key: None,
                deepseek_api_key: None,
            },
            monitor: MonitorConfig::default(),
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

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // Default values
    // -------------------------------------------------------------------------

    #[test]
    fn test_default_config_server_defaults() {
        let config = AppConfig::default();
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 8402);
    }

    #[test]
    fn test_default_config_solana_defaults() {
        let config = AppConfig::default();
        assert_eq!(config.solana.rpc_url, "https://api.devnet.solana.com");
        assert!(config.solana.recipient_wallet.is_empty());
        assert_eq!(
            config.solana.usdc_mint,
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
        );
        assert!(config.solana.escrow_program_id.is_none());
        assert!(config.solana.fee_payer_key.is_none());
        assert!(config.solana.fee_payer_keys.is_empty());
    }

    #[test]
    fn test_default_config_providers_all_none() {
        let config = AppConfig::default();
        assert!(config.providers.openai_api_key.is_none());
        assert!(config.providers.anthropic_api_key.is_none());
        assert!(config.providers.google_api_key.is_none());
        assert!(config.providers.xai_api_key.is_none());
        assert!(config.providers.deepseek_api_key.is_none());
    }

    // -------------------------------------------------------------------------
    // Security: secret redaction in Debug output (100% coverage required)
    // -------------------------------------------------------------------------

    #[test]
    fn test_providers_debug_redacts_all_keys() {
        let providers = ProvidersConfig {
            openai_api_key: Some("sk-real-openai-key-abc123".to_string()),
            anthropic_api_key: Some("sk-ant-real-key-def456".to_string()),
            google_api_key: Some("AIzaSy-real-google-key".to_string()),
            xai_api_key: Some("xai-real-key-ghi789".to_string()),
            deepseek_api_key: Some("sk-deepseek-real-key".to_string()),
        };

        let debug_output = format!("{:?}", providers);

        // Must contain [REDACTED] for each key
        assert!(
            debug_output.contains("[REDACTED]"),
            "debug output must contain [REDACTED]"
        );

        // Must NOT contain any actual key values
        assert!(
            !debug_output.contains("sk-real-openai-key-abc123"),
            "debug output must not contain OpenAI API key"
        );
        assert!(
            !debug_output.contains("sk-ant-real-key-def456"),
            "debug output must not contain Anthropic API key"
        );
        assert!(
            !debug_output.contains("AIzaSy-real-google-key"),
            "debug output must not contain Google API key"
        );
        assert!(
            !debug_output.contains("xai-real-key-ghi789"),
            "debug output must not contain xAI API key"
        );
        assert!(
            !debug_output.contains("sk-deepseek-real-key"),
            "debug output must not contain DeepSeek API key"
        );
    }

    #[test]
    fn test_providers_debug_none_keys_show_none() {
        let providers = ProvidersConfig {
            openai_api_key: None,
            anthropic_api_key: None,
            google_api_key: None,
            xai_api_key: None,
            deepseek_api_key: None,
        };

        let debug_output = format!("{:?}", providers);
        assert!(
            debug_output.contains("None"),
            "debug output should show None for unconfigured keys"
        );
        assert!(
            !debug_output.contains("[REDACTED]"),
            "debug output should not show [REDACTED] for None keys"
        );
    }

    #[test]
    fn test_solana_config_debug_redacts_fee_payer_key() {
        let solana = SolanaConfig {
            rpc_url: "https://api.mainnet-beta.solana.com".to_string(),
            recipient_wallet: "GatewayWallet111111111111111111111111111111".to_string(),
            usdc_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            escrow_program_id: None,
            fee_payer_key: Some("5KhPpRwmBMaRrjmVyPmvPqEBPcSxR3Z7ZuNbxbT5PFgT".to_string()),
            fee_payer_keys: vec!["AnotherSecretKey123".to_string()],
        };

        let debug_output = format!("{:?}", solana);
        assert!(
            debug_output.contains("[REDACTED]"),
            "debug output must contain [REDACTED] for fee_payer_key"
        );
        assert!(
            !debug_output.contains("5KhPpRwmBMaRrjmVyPmvPqEBPcSxR3Z7ZuNbxbT5PFgT"),
            "debug output must not contain actual fee payer key"
        );
        assert!(
            !debug_output.contains("AnotherSecretKey123"),
            "debug output must not contain pool key values"
        );
        // Non-secret fields should still be visible
        assert!(debug_output.contains("api.mainnet-beta.solana.com"));
        assert!(debug_output.contains("GatewayWallet"));
    }

    #[test]
    fn test_solana_config_debug_no_fee_payer_key() {
        let solana = SolanaConfig {
            rpc_url: "https://api.devnet.solana.com".to_string(),
            recipient_wallet: "Wallet111".to_string(),
            usdc_mint: default_usdc_mint(),
            escrow_program_id: None,
            fee_payer_key: None,
            fee_payer_keys: Vec::new(),
        };

        let debug_output = format!("{:?}", solana);
        assert!(
            debug_output.contains("None"),
            "should show None when no fee_payer_key"
        );
    }

    // -------------------------------------------------------------------------
    // AppConfig Debug output (derives Debug — includes child Debug impls)
    // -------------------------------------------------------------------------

    #[test]
    fn test_app_config_debug_output_safe() {
        let mut config = AppConfig::default();
        config.providers.openai_api_key = Some("super-secret-key".to_string());
        config.solana.fee_payer_key = Some("secret-fee-payer".to_string());

        let debug_output = format!("{:?}", config);
        assert!(
            !debug_output.contains("super-secret-key"),
            "AppConfig debug must not leak provider API keys"
        );
        assert!(
            !debug_output.contains("secret-fee-payer"),
            "AppConfig debug must not leak fee payer key"
        );
    }

    // -------------------------------------------------------------------------
    // fee_payer_keys merge logic
    // -------------------------------------------------------------------------

    #[test]
    fn test_all_fee_payer_keys_empty_when_none_set() {
        let config = AppConfig::default();
        assert!(config.solana.all_fee_payer_keys().is_empty());
    }

    #[test]
    fn test_all_fee_payer_keys_primary_only() {
        let mut config = AppConfig::default();
        config.solana.fee_payer_key = Some("primary-key".to_string());
        let keys = config.solana.all_fee_payer_keys();
        assert_eq!(keys, vec!["primary-key"]);
    }

    #[test]
    fn test_all_fee_payer_keys_merges_primary_and_additional() {
        let mut config = AppConfig::default();
        config.solana.fee_payer_key = Some("primary".to_string());
        config.solana.fee_payer_keys = vec!["second".to_string(), "third".to_string()];
        let keys = config.solana.all_fee_payer_keys();
        assert_eq!(keys, vec!["primary", "second", "third"]);
    }

    #[test]
    fn test_all_fee_payer_keys_skips_empty_strings() {
        let mut config = AppConfig::default();
        config.solana.fee_payer_key = Some("".to_string());
        config.solana.fee_payer_keys = vec!["".to_string(), "valid".to_string()];
        let keys = config.solana.all_fee_payer_keys();
        assert_eq!(keys, vec!["valid"]);
    }

    #[test]
    fn test_solana_config_debug_redacts_fee_payer_keys_pool() {
        let solana = SolanaConfig {
            rpc_url: "https://api.devnet.solana.com".to_string(),
            recipient_wallet: "Wallet111".to_string(),
            usdc_mint: default_usdc_mint(),
            escrow_program_id: None,
            fee_payer_key: None,
            fee_payer_keys: vec!["secret-key-2".to_string(), "secret-key-3".to_string()],
        };

        let debug_output = format!("{solana:?}");
        assert!(
            !debug_output.contains("secret-key-2"),
            "debug must not leak pool keys"
        );
        assert!(
            !debug_output.contains("secret-key-3"),
            "debug must not leak pool keys"
        );
        assert!(
            debug_output.contains("2 keys REDACTED"),
            "debug should show redacted key count"
        );
    }

    // -------------------------------------------------------------------------
    // Clone semantics
    // -------------------------------------------------------------------------

    #[test]
    fn test_config_clone_is_deep() {
        let config = AppConfig::default();
        let cloned = config.clone();
        assert_eq!(cloned.server.host, config.server.host);
        assert_eq!(cloned.server.port, config.server.port);
        assert_eq!(cloned.solana.rpc_url, config.solana.rpc_url);
    }
}
