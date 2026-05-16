use std::time::Duration;

/// Default maximum payment amount in atomic USDC units (10 USDC).
///
/// This is a conservative default to bound exposure to overcharge attacks
/// from malicious or buggy gateways. Callers expecting larger per-request
/// payments must opt in by setting `max_payment_amount` explicitly
/// (HIGH-2 from the security audit).
pub const DEFAULT_MAX_PAYMENT_AMOUNT_ATOMIC: u64 = 10_000_000;

/// Configuration for the `SolvelaClient`.
#[derive(Debug, Clone)]
#[allow(clippy::struct_excessive_bools)]
pub struct ClientConfig {
    /// Gateway URL (e.g., `http://localhost:8402`).
    pub gateway_url: String,
    /// Solana RPC URL.
    pub rpc_url: String,
    /// Prefer escrow payment scheme over exact (safer for agents).
    /// Currently defaults to `false` because escrow signing is not yet implemented.
    pub prefer_escrow: bool,
    /// Request timeout.
    pub timeout: Duration,
    /// Optional expected recipient wallet address. If set, the client will
    /// reject 402 responses that specify a different `pay_to` address,
    /// preventing payment redirect attacks by malicious gateways.
    pub expected_recipient: Option<String>,
    /// Maximum payment amount in atomic USDC units. If set, the client will
    /// reject 402 responses that request more than this amount, preventing
    /// overcharge attacks by malicious gateways.
    pub max_payment_amount: Option<u64>,
    /// Enable response caching (LRU, TTL-based). Default: `false`.
    pub enable_cache: bool,
    /// Enable session sticking (model affinity). Default: `false`.
    pub enable_sessions: bool,
    /// Session TTL. Default: 30 minutes.
    pub session_ttl: Duration,
    /// Enable degraded response detection and auto-retry. Default: `false`.
    pub enable_quality_check: bool,
    /// Maximum number of quality retries before returning the degraded response.
    /// Default: `1`.
    pub max_quality_retries: u32,
    /// Optional free tier fallback model. When set and wallet balance is zero,
    /// requests are routed to this model instead of paying.
    pub free_fallback_model: Option<String>,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            gateway_url: "http://localhost:8402".to_string(),
            rpc_url: "https://api.mainnet-beta.solana.com".to_string(),
            prefer_escrow: false,
            timeout: Duration::from_mins(3),
            expected_recipient: None,
            // HIGH-2: cap payments at 10 USDC by default. Callers wanting a
            // higher limit must opt out explicitly via the builder.
            max_payment_amount: Some(DEFAULT_MAX_PAYMENT_AMOUNT_ATOMIC),
            enable_cache: false,
            enable_sessions: false,
            session_ttl: Duration::from_mins(30),
            enable_quality_check: false,
            max_quality_retries: 1,
            free_fallback_model: None,
        }
    }
}

/// Builder for constructing a `ClientConfig`.
#[derive(Debug, Clone)]
pub struct ClientBuilder {
    gateway_url: Option<String>,
    rpc_url: Option<String>,
    prefer_escrow: Option<bool>,
    timeout: Option<Duration>,
    expected_recipient: Option<String>,
    max_payment_amount: Option<u64>,
    enable_cache: Option<bool>,
    enable_sessions: Option<bool>,
    session_ttl: Option<Duration>,
    enable_quality_check: Option<bool>,
    max_quality_retries: Option<u32>,
    free_fallback_model: Option<String>,
}

impl ClientBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            gateway_url: None,
            rpc_url: None,
            prefer_escrow: None,
            timeout: None,
            expected_recipient: None,
            max_payment_amount: None,
            enable_cache: None,
            enable_sessions: None,
            session_ttl: None,
            enable_quality_check: None,
            max_quality_retries: None,
            free_fallback_model: None,
        }
    }

    #[must_use]
    pub fn gateway_url(mut self, url: &str) -> Self {
        self.gateway_url = Some(url.trim_end_matches('/').to_string());
        self
    }

    #[must_use]
    pub fn rpc_url(mut self, url: &str) -> Self {
        self.rpc_url = Some(url.to_string());
        self
    }

    #[must_use]
    pub fn prefer_escrow(mut self, prefer: bool) -> Self {
        self.prefer_escrow = Some(prefer);
        self
    }

    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    #[must_use]
    pub fn expected_recipient(mut self, recipient: &str) -> Self {
        self.expected_recipient = Some(recipient.to_string());
        self
    }

    #[must_use]
    pub fn max_payment_amount(mut self, max: u64) -> Self {
        self.max_payment_amount = Some(max);
        self
    }

    #[must_use]
    pub fn enable_cache(mut self, enable: bool) -> Self {
        self.enable_cache = Some(enable);
        self
    }

    #[must_use]
    pub fn enable_sessions(mut self, enable: bool) -> Self {
        self.enable_sessions = Some(enable);
        self
    }

    #[must_use]
    pub fn session_ttl(mut self, ttl: Duration) -> Self {
        self.session_ttl = Some(ttl);
        self
    }

    #[must_use]
    pub fn enable_quality_check(mut self, enable: bool) -> Self {
        self.enable_quality_check = Some(enable);
        self
    }

    #[must_use]
    pub fn max_quality_retries(mut self, max: u32) -> Self {
        self.max_quality_retries = Some(max);
        self
    }

    #[must_use]
    pub fn free_fallback_model(mut self, model: &str) -> Self {
        self.free_fallback_model = Some(model.to_string());
        self
    }

    /// Build a `ClientConfig` from the builder state, using defaults for unset values.
    #[must_use]
    pub fn build_config(self) -> ClientConfig {
        let defaults = ClientConfig::default();
        ClientConfig {
            gateway_url: self.gateway_url.unwrap_or(defaults.gateway_url),
            rpc_url: self.rpc_url.unwrap_or(defaults.rpc_url),
            prefer_escrow: self.prefer_escrow.unwrap_or(defaults.prefer_escrow),
            timeout: self.timeout.unwrap_or(defaults.timeout),
            expected_recipient: self.expected_recipient.or(defaults.expected_recipient),
            max_payment_amount: self.max_payment_amount.or(defaults.max_payment_amount),
            enable_cache: self.enable_cache.unwrap_or(defaults.enable_cache),
            enable_sessions: self.enable_sessions.unwrap_or(defaults.enable_sessions),
            session_ttl: self.session_ttl.unwrap_or(defaults.session_ttl),
            enable_quality_check: self
                .enable_quality_check
                .unwrap_or(defaults.enable_quality_check),
            max_quality_retries: self
                .max_quality_retries
                .unwrap_or(defaults.max_quality_retries),
            free_fallback_model: self.free_fallback_model.or(defaults.free_fallback_model),
        }
    }
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ClientConfig::default();
        assert_eq!(config.gateway_url, "http://localhost:8402");
        assert_eq!(config.rpc_url, "https://api.mainnet-beta.solana.com");
        assert!(!config.prefer_escrow);
        assert_eq!(config.timeout, Duration::from_mins(3));
    }

    #[test]
    fn test_builder_defaults() {
        let config = ClientBuilder::new().build_config();
        assert_eq!(config.gateway_url, "http://localhost:8402");
        assert!(!config.prefer_escrow);
    }

    #[test]
    fn test_builder_custom_values() {
        let config = ClientBuilder::new()
            .gateway_url("https://my-gateway.fly.dev")
            .rpc_url("https://my-rpc.com")
            .prefer_escrow(false)
            .timeout(Duration::from_mins(1))
            .build_config();

        assert_eq!(config.gateway_url, "https://my-gateway.fly.dev");
        assert_eq!(config.rpc_url, "https://my-rpc.com");
        assert!(!config.prefer_escrow);
        assert_eq!(config.timeout, Duration::from_mins(1));
    }

    #[test]
    fn test_builder_gateway_url_trailing_slash_stripped() {
        let config = ClientBuilder::new()
            .gateway_url("https://my-gateway.fly.dev/")
            .build_config();
        assert_eq!(config.gateway_url, "https://my-gateway.fly.dev");
    }

    #[test]
    fn test_builder_expected_recipient() {
        let config = ClientBuilder::new()
            .expected_recipient("RecipientWalletAddress")
            .build_config();
        assert_eq!(
            config.expected_recipient.as_deref(),
            Some("RecipientWalletAddress")
        );
    }

    #[test]
    fn test_builder_max_payment_amount() {
        let config = ClientBuilder::new()
            .max_payment_amount(100_000)
            .build_config();
        assert_eq!(config.max_payment_amount, Some(100_000));
    }

    #[test]
    fn test_default_config_security_defaults() {
        // HIGH-2: max_payment_amount defaults to a 10 USDC cap.
        // expected_recipient remains opt-in; callers receive a startup warn
        // (emitted in `SolvelaClient::new`) when it is unset.
        let config = ClientConfig::default();
        assert!(config.expected_recipient.is_none());
        assert_eq!(
            config.max_payment_amount,
            Some(DEFAULT_MAX_PAYMENT_AMOUNT_ATOMIC)
        );
    }

    #[test]
    fn test_default_config_smart_features_off() {
        let config = ClientConfig::default();
        assert!(!config.enable_cache);
        assert!(!config.enable_sessions);
        assert_eq!(config.session_ttl, Duration::from_mins(30));
        assert!(!config.enable_quality_check);
        assert_eq!(config.max_quality_retries, 1);
        assert!(config.free_fallback_model.is_none());
    }

    #[test]
    fn test_builder_enable_cache() {
        let config = ClientBuilder::new().enable_cache(true).build_config();
        assert!(config.enable_cache);
    }

    #[test]
    fn test_builder_enable_sessions_with_ttl() {
        let config = ClientBuilder::new()
            .enable_sessions(true)
            .session_ttl(Duration::from_mins(10))
            .build_config();
        assert!(config.enable_sessions);
        assert_eq!(config.session_ttl, Duration::from_mins(10));
    }

    #[test]
    fn test_builder_quality_check() {
        let config = ClientBuilder::new()
            .enable_quality_check(true)
            .max_quality_retries(3)
            .build_config();
        assert!(config.enable_quality_check);
        assert_eq!(config.max_quality_retries, 3);
    }

    #[test]
    fn test_builder_free_fallback_model() {
        let config = ClientBuilder::new()
            .free_fallback_model("openai/gpt-oss-120b")
            .build_config();
        assert_eq!(
            config.free_fallback_model.as_deref(),
            Some("openai/gpt-oss-120b")
        );
    }
}
