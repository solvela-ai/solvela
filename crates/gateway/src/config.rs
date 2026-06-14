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
    /// Caching configuration (response/semantic cache).
    #[serde(default)]
    pub cache: CacheSettings,
    /// Gas-drip faucet configuration (`[faucet]` / `SOLVELA_FAUCET__*`).
    #[serde(default)]
    pub faucet: FaucetConfig,
}

/// Gas-drip faucet configuration (`[faucet]`).
///
/// The faucet drips a dust of native SOL to USDC-funded agent wallets so the
/// agent (the on-chain fee payer) can pay its own gas — the user funds USDC
/// only. Disabled by default; requires BOTH `enabled = true` AND a configured
/// `source_key` (`SOLVELA_FAUCET__SOURCE_KEY`) to be active. A DB is also
/// required for once-per-wallet idempotency (the route disables itself with a
/// `warn!` when no `DATABASE_URL` is configured).
///
/// `source_key` is a dedicated gas keypair — it is deliberately NOT the
/// fee-payer reserve (which pays providers and is regulatory-sensitive). When
/// `source_key` is unset the faucet is disabled even if `enabled = true`.
///
/// `Debug` is manually implemented to redact `source_key` (an ed25519 private
/// key that must never appear in logs or panic output).
#[derive(Clone, Deserialize)]
pub struct FaucetConfig {
    /// Master switch. Default `false`.
    #[serde(default)]
    pub enabled: bool,
    /// Lamports dripped per funded wallet. Default `10_000_000` (0.01 SOL).
    #[serde(default = "default_drip_lamports")]
    pub drip_lamports: u64,
    /// Minimum wallet USDC balance (atomic, 6-decimal) to qualify — the abuse
    /// moat. Default `100_000` (0.10 USDC).
    #[serde(default = "default_usdc_floor_atomic")]
    pub usdc_floor_atomic: u64,
    /// SOL low-water mark (lamports): only drip when the wallet's SOL balance is
    /// strictly below this. Default `3_000_000`.
    #[serde(default = "default_sol_low_water_lamports")]
    pub sol_low_water_lamports: u64,
    /// Global per-UTC-day cap on total lamports dripped. Default
    /// `1_000_000_000` (1 SOL).
    #[serde(default = "default_daily_cap_lamports")]
    pub daily_cap_lamports: u64,
    /// Base58-encoded 64-byte dedicated gas keypair. `None` (or empty) disables
    /// the faucet. Loaded from `SOLVELA_FAUCET__SOURCE_KEY`. SECRET — redacted
    /// in `Debug`, never logged.
    #[serde(default)]
    pub source_key: Option<String>,
}

impl Default for FaucetConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            drip_lamports: default_drip_lamports(),
            usdc_floor_atomic: default_usdc_floor_atomic(),
            sol_low_water_lamports: default_sol_low_water_lamports(),
            daily_cap_lamports: default_daily_cap_lamports(),
            source_key: None,
        }
    }
}

impl FaucetConfig {
    /// Whether the faucet has the minimum config to operate: enabled AND a
    /// non-empty source key. (A DB is additionally required and checked at the
    /// route, since the pool lives on `AppState`, not the config.)
    pub fn is_active(&self) -> bool {
        self.enabled
            && self
                .source_key
                .as_ref()
                .map(|k| !k.is_empty())
                .unwrap_or(false)
    }
}

impl fmt::Debug for FaucetConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FaucetConfig")
            .field("enabled", &self.enabled)
            .field("drip_lamports", &self.drip_lamports)
            .field("usdc_floor_atomic", &self.usdc_floor_atomic)
            .field("sol_low_water_lamports", &self.sol_low_water_lamports)
            .field("daily_cap_lamports", &self.daily_cap_lamports)
            .field(
                "source_key",
                &self.source_key.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
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
    /// Public base URL the gateway is reachable at (e.g. `https://api.solvela.ai`).
    ///
    /// Used as the `url` field of the A2A AgentCard, which external agents and
    /// registries follow to reach `/a2a`. When unset, the card falls back to
    /// `http://{host}:{port}` — correct for local dev but NOT a public address
    /// (the default bind host is `0.0.0.0`). Set via `SOLVELA_PUBLIC_URL` in any
    /// deployment that publishes its card.
    #[serde(default)]
    pub public_url: Option<String>,
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

/// Caching configuration (`[cache]`).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct CacheSettings {
    /// Semantic (embedding-similarity) cache tier (`[cache.semantic]`).
    #[serde(default)]
    pub semantic: SemanticSettings,
}

/// Semantic cache tier settings (`[cache.semantic]`).
///
/// Defaults are deliberately conservative: the tier is **off** unless an
/// operator opts in, so enabling the feature in code is zero behaviour change.
#[derive(Clone, Deserialize)]
pub struct SemanticSettings {
    /// Enable the semantic cache tier. Default `false`.
    #[serde(default)]
    pub enabled: bool,
    /// Minimum cosine similarity for a hit, in `[0, 1]`. Default `0.85`.
    #[serde(default = "default_semantic_threshold")]
    pub threshold: f32,
    /// Percent of the full price billed on a semantic hit (e.g. `30` = pay 30%,
    /// a 70% discount). Default `30`, which sits in the 10–50% band used by
    /// provider prompt caching (DeepSeek ~10%, OpenAI 25–50%, Anthropic ~10%).
    ///
    /// This is a flat fraction of the **all-in** miss price (provider cost +
    /// 5% platform fee), so the fee scales down proportionally — matching how
    /// those services discount the all-in rate. Note this is *not* a loss on
    /// the fee: a cache hit pays the upstream provider nothing, so the entire
    /// claimed amount is gateway revenue (minus the sub-cent embedding cost).
    /// Realised on the escrow scheme (the gateway claims only this fraction and
    /// refunds the remainder); the direct-transfer `exact` scheme settled the
    /// full amount up front, so no discount applies there. Clamped to `0..=100`.
    #[serde(default = "default_semantic_hit_price_percent")]
    pub hit_price_percent: u8,
    /// TTL (seconds) for stored semantic entries. Default `600` (10 min).
    #[serde(default = "default_semantic_ttl_secs")]
    pub ttl_secs: u64,
    /// Explicit on-disk directory for the embedding model. `None` → `fastembed`'s
    /// default (`./.fastembed_cache` relative to the process working dir).
    #[serde(default)]
    pub model_cache_dir: Option<String>,
}

impl Default for SemanticSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            threshold: default_semantic_threshold(),
            hit_price_percent: default_semantic_hit_price_percent(),
            ttl_secs: default_semantic_ttl_secs(),
            model_cache_dir: None,
        }
    }
}

impl SemanticSettings {
    /// Validate operator-supplied semantic-cache settings. Only checked when
    /// the tier is enabled — a disabled cache's values are inert.
    ///
    /// These must be **fatal at startup**, not silently degraded to a disabled
    /// cache: a misconfigured `threshold` or `hit_price_percent` is an operator
    /// error (the operator believes the cache is running), distinct from
    /// infrastructure being unavailable (Redis down → graceful disable).
    ///
    /// - `threshold` must be in `(0.0, 1.0]`. `0.0` makes `similarity >= 0`
    ///   vacuously true (every query a hit); `> 1.0` makes it vacuously false.
    /// - `hit_price_percent` must be in `1..=100`. `0` would bill nothing on a
    ///   hit → the escrow claim is skipped by the `amount == 0` guard → a free
    ///   response with no settlement. If you want free serving, disable the tier.
    /// - `ttl_secs` must be `>= 1`. A `0` TTL makes the write-path `EXPIRE key 0`
    ///   delete each entry the instant it is stored → a silent all-miss cache the
    ///   operator believes is running. If you want no caching, disable the tier.
    pub fn validate(&self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        if !(self.threshold > 0.0 && self.threshold <= 1.0) {
            return Err(format!(
                "cache.semantic.threshold must be in (0.0, 1.0], got {}",
                self.threshold
            ));
        }
        if !(1..=100).contains(&self.hit_price_percent) {
            return Err(format!(
                "cache.semantic.hit_price_percent must be in 1..=100, got {}",
                self.hit_price_percent
            ));
        }
        if self.ttl_secs == 0 {
            return Err(
                "cache.semantic.ttl_secs must be >= 1 (got 0); a 0 TTL makes Redis EXPIRE \
                 delete each entry on write, silently producing an all-miss cache. \
                 Disable the tier instead if you want no caching."
                    .to_string(),
            );
        }
        Ok(())
    }
}

// `SemanticSettings` carries no secrets, so the derived `Debug` is fine — but we
// implement it explicitly to keep the field set obvious alongside the redacted
// configs above.
impl fmt::Debug for SemanticSettings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SemanticSettings")
            .field("enabled", &self.enabled)
            .field("threshold", &self.threshold)
            .field("hit_price_percent", &self.hit_price_percent)
            .field("ttl_secs", &self.ttl_secs)
            .field("model_cache_dir", &self.model_cache_dir)
            .finish()
    }
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                host: default_host(),
                port: default_port(),
                public_url: None,
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
            cache: CacheSettings::default(),
            faucet: FaucetConfig::default(),
        }
    }
}

fn default_host() -> String {
    "0.0.0.0".to_string()
}

fn default_semantic_threshold() -> f32 {
    0.85
}

fn default_semantic_hit_price_percent() -> u8 {
    30
}

fn default_semantic_ttl_secs() -> u64 {
    600
}

fn default_port() -> u16 {
    8402
}

fn default_drip_lamports() -> u64 {
    10_000_000 // 0.01 SOL
}

fn default_usdc_floor_atomic() -> u64 {
    100_000 // 0.10 USDC (6-decimal atomic)
}

fn default_sol_low_water_lamports() -> u64 {
    3_000_000
}

fn default_daily_cap_lamports() -> u64 {
    1_000_000_000 // 1 SOL
}

fn default_usdc_mint() -> String {
    solvela_protocol::constants::USDC_MINT.to_string()
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
    // Semantic cache settings ([cache.semantic])
    // -------------------------------------------------------------------------

    #[test]
    fn semantic_cache_defaults_off_and_conservative() {
        let config = AppConfig::default();
        assert!(
            !config.cache.semantic.enabled,
            "semantic cache MUST default off — zero behaviour change"
        );
        assert_eq!(config.cache.semantic.threshold, 0.85);
        assert_eq!(config.cache.semantic.hit_price_percent, 30);
        assert_eq!(config.cache.semantic.ttl_secs, 600);
        assert!(config.cache.semantic.model_cache_dir.is_none());
    }

    #[test]
    fn semantic_cache_parses_from_toml() {
        let toml = r#"
[server]
[solana]
rpc_url = "https://api.devnet.solana.com"
recipient_wallet = ""
[providers]
[cache.semantic]
enabled = true
threshold = 0.9
hit_price_percent = 25
ttl_secs = 1200
model_cache_dir = "/models/bge"
"#;
        let config: AppConfig = toml::from_str(toml).expect("valid config TOML");
        assert!(config.cache.semantic.enabled);
        assert_eq!(config.cache.semantic.threshold, 0.9);
        assert_eq!(config.cache.semantic.hit_price_percent, 25);
        assert_eq!(config.cache.semantic.ttl_secs, 1200);
        assert_eq!(
            config.cache.semantic.model_cache_dir.as_deref(),
            Some("/models/bge")
        );
    }

    fn enabled_semantic(threshold: f32, hit_price_percent: u8) -> SemanticSettings {
        SemanticSettings {
            enabled: true,
            threshold,
            hit_price_percent,
            ttl_secs: 600,
            model_cache_dir: None,
        }
    }

    #[test]
    fn semantic_validate_accepts_sane_values() {
        assert!(enabled_semantic(0.85, 30).validate().is_ok());
        assert!(enabled_semantic(1.0, 100).validate().is_ok()); // boundaries
        assert!(enabled_semantic(0.0001, 1).validate().is_ok());
    }

    #[test]
    fn semantic_validate_rejects_zero_hit_price_percent() {
        // 0% → zero claim → escrow skip → free serve. Must be fatal, not silent.
        let err = enabled_semantic(0.85, 0).validate().unwrap_err();
        assert!(err.contains("hit_price_percent"), "got: {err}");
    }

    #[test]
    fn semantic_validate_rejects_out_of_range_hit_price_percent() {
        assert!(enabled_semantic(0.85, 101).validate().is_err());
    }

    #[test]
    fn semantic_validate_rejects_zero_ttl_secs() {
        // ttl_secs=0 → EXPIRE key 0 deletes the entry on write → silent all-miss
        // cache. Must be fatal at startup, not silently degraded (bug_015).
        let mut s = enabled_semantic(0.85, 30);
        s.ttl_secs = 0;
        let err = s.validate().unwrap_err();
        assert!(err.contains("ttl_secs"), "got: {err}");
    }

    #[test]
    fn semantic_validate_rejects_out_of_range_threshold() {
        // 0.0 makes every query a vacuous hit; >1.0 makes every query a miss.
        assert!(enabled_semantic(0.0, 30).validate().is_err());
        assert!(enabled_semantic(1.5, 30).validate().is_err());
    }

    #[test]
    fn semantic_validate_skips_when_disabled() {
        // A disabled cache's values are inert — never block startup on them.
        let mut s = enabled_semantic(0.0, 0);
        s.enabled = false;
        assert!(s.validate().is_ok());
    }

    #[test]
    fn cache_section_absent_falls_back_to_defaults() {
        let toml = r#"
[server]
[solana]
rpc_url = "https://api.devnet.solana.com"
recipient_wallet = ""
[providers]
"#;
        let config: AppConfig = toml::from_str(toml).expect("valid config TOML");
        assert!(!config.cache.semantic.enabled);
        assert_eq!(config.cache.semantic.threshold, 0.85);
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
    // Gas-drip faucet config ([faucet] / SOLVELA_FAUCET__*)
    // -------------------------------------------------------------------------

    #[test]
    fn faucet_defaults_off_and_conservative() {
        let config = AppConfig::default();
        assert!(
            !config.faucet.enabled,
            "faucet MUST default off — zero behaviour change"
        );
        assert_eq!(config.faucet.drip_lamports, 10_000_000);
        assert_eq!(config.faucet.usdc_floor_atomic, 100_000);
        assert_eq!(config.faucet.sol_low_water_lamports, 3_000_000);
        assert_eq!(config.faucet.daily_cap_lamports, 1_000_000_000);
        assert!(config.faucet.source_key.is_none());
    }

    #[test]
    fn faucet_is_active_requires_enabled_and_source_key() {
        // Disabled (default) → not active even with a key.
        let mut f = FaucetConfig {
            source_key: Some("some-key".to_string()),
            ..FaucetConfig::default()
        };
        assert!(!f.is_active(), "disabled faucet must not be active");

        // Enabled but no source key → not active (do NOT reuse the fee-payer reserve).
        f.enabled = true;
        f.source_key = None;
        assert!(
            !f.is_active(),
            "enabled faucet without a source key must not be active"
        );

        // Enabled but EMPTY source key → not active.
        f.source_key = Some(String::new());
        assert!(
            !f.is_active(),
            "empty source key must not activate the faucet"
        );

        // Enabled AND non-empty source key → active.
        f.source_key = Some("dedicated-gas-key".to_string());
        assert!(f.is_active(), "enabled + source key must be active");
    }

    #[test]
    fn faucet_parses_from_toml() {
        let toml = r#"
[server]
[solana]
rpc_url = "https://api.devnet.solana.com"
recipient_wallet = ""
[providers]
[faucet]
enabled = true
drip_lamports = 5000000
usdc_floor_atomic = 250000
sol_low_water_lamports = 2000000
daily_cap_lamports = 500000000
"#;
        let config: AppConfig = toml::from_str(toml).expect("valid config TOML");
        assert!(config.faucet.enabled);
        assert_eq!(config.faucet.drip_lamports, 5_000_000);
        assert_eq!(config.faucet.usdc_floor_atomic, 250_000);
        assert_eq!(config.faucet.sol_low_water_lamports, 2_000_000);
        assert_eq!(config.faucet.daily_cap_lamports, 500_000_000);
    }

    #[test]
    fn faucet_debug_redacts_source_key() {
        let f = FaucetConfig {
            enabled: true,
            source_key: Some("5KhPpRwmBMaRrjmVyPmvPqEBPcSxR3Z7ZuNbxbT5PFgT".to_string()),
            ..FaucetConfig::default()
        };
        let debug_output = format!("{f:?}");
        assert!(
            debug_output.contains("[REDACTED]"),
            "Debug must redact source_key: {debug_output}"
        );
        assert!(
            !debug_output.contains("5KhPpRwmBMaRrjmVyPmvPqEBPcSxR3Z7ZuNbxbT5PFgT"),
            "Debug must not leak source_key: {debug_output}"
        );
        // Non-secret fields still visible.
        assert!(debug_output.contains("drip_lamports"));
    }

    #[test]
    fn faucet_debug_none_source_key_shows_none() {
        let f = FaucetConfig::default();
        let debug_output = format!("{f:?}");
        assert!(
            !debug_output.contains("[REDACTED]"),
            "no source key → no REDACTED marker: {debug_output}"
        );
        assert!(debug_output.contains("None"));
    }

    #[test]
    fn app_config_debug_does_not_leak_faucet_source_key() {
        let mut config = AppConfig::default();
        config.faucet.source_key = Some("super-secret-gas-key".to_string());
        let debug_output = format!("{config:?}");
        assert!(
            !debug_output.contains("super-secret-gas-key"),
            "AppConfig debug must not leak the faucet source key"
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
