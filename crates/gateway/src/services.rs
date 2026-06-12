//! x402 Service Marketplace registry types.
//!
//! `ServiceRegistry` loads `config/services.toml` and provides the list of
//! registered x402-compatible services for the `GET /v1/services` endpoint.
//! Supports runtime registration via `register()` for the admin API.

use std::collections::HashMap;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum ServiceRegistryError {
    #[error("failed to parse services.toml: {0}")]
    ParseError(#[from] toml::de::Error),

    #[error("invalid service entry '{id}': {source}")]
    InvalidEntry {
        id: String,
        #[source]
        source: RegistrationError,
    },
}

/// Errors returned by `ServiceRegistry::register()`.
#[derive(Debug, Error, PartialEq)]
pub enum RegistrationError {
    #[error("service with id '{0}' already exists")]
    DuplicateId(String),

    #[error("id must be non-empty, max 64 chars, and match [a-z0-9\\-]")]
    InvalidId,

    #[error("name must be non-empty and max 128 chars")]
    InvalidName,

    #[error("category must be non-empty and max 32 chars")]
    InvalidCategory,

    #[error("endpoint must start with https://")]
    InvalidEndpoint,

    #[error("price_per_request_usdc must be greater than 0")]
    InvalidPrice,

    #[error("vendor_wallet must be a valid base58-encoded 32-byte Solana pubkey")]
    InvalidVendorWallet,

    #[error("vendor_wallet is only supported on external services")]
    VendorWalletOnInternalService,
}

// ---------------------------------------------------------------------------
// TOML schema (mirrors services.toml table structure)
// ---------------------------------------------------------------------------

/// Raw TOML representation of a single service entry.
#[derive(Debug, Clone, Deserialize)]
struct RawServiceEntry {
    pub name: String,
    pub endpoint: String,
    pub category: String,
    pub x402_enabled: bool,
    pub internal: bool,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub pricing_label: Option<String>,
    #[serde(default)]
    pub price_per_request_usdc: Option<f64>,
    #[serde(default)]
    pub vendor_wallet: Option<String>,
}

/// Outer TOML wrapper: `[services.<id>]` tables.
#[derive(Debug, Deserialize)]
struct RawServices {
    services: HashMap<String, RawServiceEntry>,
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A single registered x402-compatible service.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ServiceEntry {
    /// Unique machine-readable identifier (e.g. `"llm-gateway"`).
    pub id: String,
    /// Human-readable service name.
    pub name: String,
    /// Service category (e.g. `"intelligence"`, `"media"`, `"search"`).
    pub category: String,
    /// API endpoint (absolute URL for external services, path for internal).
    pub endpoint: String,
    /// Whether x402 payment is required for this service.
    pub x402_enabled: bool,
    /// `true` if the service is hosted by the gateway itself.
    pub internal: bool,
    /// Optional human-readable description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Pricing label shown to callers (e.g. `"per-token (see /pricing)"`).
    pub pricing_label: String,
    /// Supported payment chains (always includes `"solana"`).
    pub chains: Vec<String>,
    /// How this service was registered: `"config"` for TOML-loaded, `"api"` for runtime.
    pub source: String,
    /// Health status: `None` = never checked, `Some(true)` = healthy, `Some(false)` = unhealthy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub healthy: Option<bool>,
    /// Flat per-request price in USDC (used by external services).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price_per_request_usdc: Option<f64>,
    /// Per-service payment recipient (base58 Solana wallet pubkey).
    ///
    /// Settlement-platform P1, per the "Vendor-Settlement Fee Mechanics" RFC
    /// (2026-06-12, mechanism C / record + invoice, **vendor absorbs** the fee):
    /// when set, the agent pays EXACTLY `price_per_request_usdc` (no 5%
    /// fee-on-top) in a single transfer addressed to this wallet, and
    /// Solvela's 5% platform fee is RECORDED as an off-chain receivable
    /// against the vendor (spend_logs), never charged to the agent. When
    /// `None`, today's behavior applies unchanged: `price × 1.05` paid to the
    /// gateway's global recipient wallet.
    ///
    /// Validated as a base58 32-byte Solana pubkey at load/registration time
    /// (fail closed) and only permitted on external services — the internal
    /// chat pipeline does not honor it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vendor_wallet: Option<String>,
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// In-memory registry of x402-compatible services loaded from `services.toml`.
///
/// All read/write access is mediated through methods that operate on the
/// internal `Vec<ServiceEntry>`. When shared via `tokio::sync::RwLock` in
/// `AppState`, concurrent reads are cheap and writes (registration) are rare.
#[derive(Debug, Clone)]
pub struct ServiceRegistry {
    services: Vec<ServiceEntry>,
}

impl ServiceRegistry {
    /// Create an empty registry (no services registered).
    pub fn empty() -> Self {
        Self {
            services: Vec::new(),
        }
    }

    /// Parse a TOML string (content of `services.toml`) into a registry.
    ///
    /// Each entry is validated through the same checks `register()` enforces
    /// (id format, name/category bounds, endpoint scheme, positive price).
    /// A malformed entry causes the whole load to fail with
    /// `ServiceRegistryError::InvalidEntry` — fail-closed beats serving an
    /// unsafe registry.
    pub fn from_toml(toml_str: &str) -> Result<Self, ServiceRegistryError> {
        let raw: RawServices = toml::from_str(toml_str)?;

        let mut services: Vec<ServiceEntry> = Vec::with_capacity(raw.services.len());
        for (id, raw) in raw.services {
            let entry = ServiceEntry {
                id: id.clone(),
                name: raw.name,
                category: raw.category,
                endpoint: raw.endpoint,
                x402_enabled: raw.x402_enabled,
                internal: raw.internal,
                description: raw.description,
                pricing_label: raw
                    .pricing_label
                    .unwrap_or_else(|| "per-request (see /pricing)".to_string()),
                chains: vec!["solana".to_string()],
                source: "config".to_string(),
                healthy: None,
                price_per_request_usdc: raw.price_per_request_usdc,
                vendor_wallet: raw.vendor_wallet,
            };
            validate_entry(&entry).map_err(|source| ServiceRegistryError::InvalidEntry {
                id: id.clone(),
                source,
            })?;
            services.push(entry);
        }

        // Stable alphabetical order so response is deterministic.
        services.sort_by(|a, b| a.id.cmp(&b.id));

        Ok(Self { services })
    }

    /// Return all registered services.
    pub fn all(&self) -> &[ServiceEntry] {
        &self.services
    }

    /// Return only internal (gateway-hosted) services.
    pub fn internal(&self) -> Vec<&ServiceEntry> {
        self.services.iter().filter(|s| s.internal).collect()
    }

    /// Return only external (third-party) services.
    pub fn external(&self) -> Vec<&ServiceEntry> {
        self.services.iter().filter(|s| !s.internal).collect()
    }

    /// Look up a service by ID.
    pub fn get(&self, id: &str) -> Option<&ServiceEntry> {
        self.services.iter().find(|s| s.id == id)
    }

    /// Register a new external service at runtime.
    ///
    /// Validates the entry and inserts it into the registry. Returns
    /// `Err(RegistrationError)` if validation fails or the ID is taken.
    pub fn register(&mut self, entry: ServiceEntry) -> Result<(), RegistrationError> {
        validate_entry(&entry)?;

        // Check uniqueness against the current registry.
        if self.services.iter().any(|s| s.id == entry.id) {
            return Err(RegistrationError::DuplicateId(entry.id));
        }

        self.services.push(entry);
        // Re-sort to maintain deterministic order.
        self.services.sort_by(|a, b| a.id.cmp(&b.id));

        Ok(())
    }
}

/// Validate a `ServiceEntry`'s field values without checking uniqueness.
///
/// Single source of truth for both `from_toml` (load-time) and `register()`
/// (runtime registration) so a malformed `services.toml` cannot bypass the
/// checks runtime registration enforces.
///
/// Internal services (`internal == true`) use relative paths like
/// `/v1/chat/completions` for their `endpoint`; external services must use
/// HTTPS absolute URLs.
fn validate_entry(entry: &ServiceEntry) -> Result<(), RegistrationError> {
    // id: non-empty, max 64, [a-z0-9-] only
    if entry.id.is_empty()
        || entry.id.len() > 64
        || !entry
            .id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(RegistrationError::InvalidId);
    }

    // name
    if entry.name.is_empty() || entry.name.len() > 128 {
        return Err(RegistrationError::InvalidName);
    }

    // category
    if entry.category.is_empty() || entry.category.len() > 32 {
        return Err(RegistrationError::InvalidCategory);
    }

    // endpoint — HTTPS-only for external; relative path for internal.
    if entry.internal {
        if !entry.endpoint.starts_with('/') {
            return Err(RegistrationError::InvalidEndpoint);
        }
    } else if !entry.endpoint.starts_with("https://") {
        return Err(RegistrationError::InvalidEndpoint);
    }

    // Price — only required for external services. Internal services are
    // billed per-token via the chat pipeline, so `price_per_request_usdc`
    // is optional for them.
    if !entry.internal {
        match entry.price_per_request_usdc {
            Some(price) if price > 0.0 => {}
            _ => return Err(RegistrationError::InvalidPrice),
        }
    }

    // vendor_wallet — fail closed on anything that is not a base58 32-byte
    // Solana pubkey: an invalid recipient must reject the entry at load /
    // registration time, never surface later as an unpayable 402 quote or a
    // misdirected transfer. Internal services reject it outright: the chat
    // pipeline does not honor vendor settlement, so accepting it there would
    // let config claim a vendor gets paid while the gateway pays itself.
    if let Some(ref vendor_wallet) = entry.vendor_wallet {
        if entry.internal {
            return Err(RegistrationError::VendorWalletOnInternalService);
        }
        if solvela_x402::solana_types::Pubkey::from_str(vendor_wallet).is_err() {
            return Err(RegistrationError::InvalidVendorWallet);
        }
    }

    Ok(())
}

impl ServiceRegistry {
    /// Update the health status for a service by ID.
    ///
    /// Returns `true` if the service was found (and updated), `false` otherwise.
    pub fn set_health(&mut self, service_id: &str, healthy: bool) -> bool {
        if let Some(entry) = self.services.iter_mut().find(|s| s.id == service_id) {
            entry.healthy = Some(healthy);
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_TOML: &str = r#"
[services.llm-gateway]
name = "LLM Intelligence"
endpoint = "/v1/chat/completions"
category = "intelligence"
x402_enabled = true
internal = true
description = "OpenAI-compatible LLM inference with smart routing"
pricing_label = "per-token (see /pricing)"

[services.image-gen]
name = "Image Generation"
endpoint = "/v1/images/generations"
category = "media"
x402_enabled = true
internal = true
pricing_label = "per-image"

[services.web-search]
name = "Web Search"
endpoint = "https://search.example.com/v1/query"
category = "search"
x402_enabled = true
internal = false
pricing_label = "$0.005/query"
price_per_request_usdc = 0.005
"#;

    #[test]
    fn test_parse_services_toml() {
        let registry = ServiceRegistry::from_toml(SAMPLE_TOML).unwrap();
        assert_eq!(registry.all().len(), 3);
    }

    #[test]
    fn test_services_sorted_alphabetically() {
        let registry = ServiceRegistry::from_toml(SAMPLE_TOML).unwrap();
        let ids: Vec<&str> = registry.all().iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, vec!["image-gen", "llm-gateway", "web-search"]);
    }

    #[test]
    fn test_internal_filter() {
        let registry = ServiceRegistry::from_toml(SAMPLE_TOML).unwrap();
        assert_eq!(registry.internal().len(), 2);
        for s in registry.internal() {
            assert!(s.internal);
        }
    }

    #[test]
    fn test_external_filter() {
        let registry = ServiceRegistry::from_toml(SAMPLE_TOML).unwrap();
        assert_eq!(registry.external().len(), 1);
        assert_eq!(registry.external()[0].id, "web-search");
    }

    #[test]
    fn test_get_by_id() {
        let registry = ServiceRegistry::from_toml(SAMPLE_TOML).unwrap();
        let svc = registry.get("llm-gateway").unwrap();
        assert_eq!(svc.name, "LLM Intelligence");
        assert_eq!(svc.category, "intelligence");
        assert!(svc.internal);
    }

    #[test]
    fn test_chains_always_includes_solana() {
        let registry = ServiceRegistry::from_toml(SAMPLE_TOML).unwrap();
        for svc in registry.all() {
            assert!(svc.chains.contains(&"solana".to_string()));
        }
    }

    #[test]
    fn test_default_pricing_label() {
        let toml = r#"
[services.test-svc]
name = "Test"
endpoint = "/test"
category = "test"
x402_enabled = true
internal = true
"#;
        let registry = ServiceRegistry::from_toml(toml).unwrap();
        assert_eq!(
            registry.all()[0].pricing_label,
            "per-request (see /pricing)"
        );
    }

    #[test]
    fn test_invalid_toml_returns_error() {
        let bad = "not valid toml [[[";
        assert!(ServiceRegistry::from_toml(bad).is_err());
    }

    #[test]
    fn test_source_is_config_for_toml_loaded() {
        let registry = ServiceRegistry::from_toml(SAMPLE_TOML).unwrap();
        for svc in registry.all() {
            assert_eq!(svc.source, "config");
        }
    }

    #[test]
    fn test_healthy_defaults_to_none() {
        let registry = ServiceRegistry::from_toml(SAMPLE_TOML).unwrap();
        for svc in registry.all() {
            assert_eq!(svc.healthy, None);
        }
    }

    #[test]
    fn test_price_per_request_parsed() {
        let registry = ServiceRegistry::from_toml(SAMPLE_TOML).unwrap();
        let ws = registry.get("web-search").unwrap();
        assert_eq!(ws.price_per_request_usdc, Some(0.005));
        // Internal services have no per-request price
        let llm = registry.get("llm-gateway").unwrap();
        assert_eq!(llm.price_per_request_usdc, None);
    }

    #[test]
    fn test_register_valid_service() {
        let mut registry = ServiceRegistry::empty();
        let entry = ServiceEntry {
            id: "my-api".to_string(),
            name: "My API".to_string(),
            category: "data".to_string(),
            endpoint: "https://api.example.com/v1".to_string(),
            x402_enabled: true,
            internal: false,
            description: Some("A test API".to_string()),
            pricing_label: "$0.01/request".to_string(),
            chains: vec!["solana".to_string()],
            source: "api".to_string(),
            healthy: None,
            price_per_request_usdc: Some(0.01),
            vendor_wallet: None,
        };
        assert!(registry.register(entry).is_ok());
        assert_eq!(registry.all().len(), 1);
        assert_eq!(registry.get("my-api").unwrap().source, "api");
    }

    #[test]
    fn test_register_duplicate_id_fails() {
        let mut registry = ServiceRegistry::empty();
        let entry = ServiceEntry {
            id: "my-api".to_string(),
            name: "My API".to_string(),
            category: "data".to_string(),
            endpoint: "https://api.example.com/v1".to_string(),
            x402_enabled: true,
            internal: false,
            description: None,
            pricing_label: "$0.01/request".to_string(),
            chains: vec!["solana".to_string()],
            source: "api".to_string(),
            healthy: None,
            price_per_request_usdc: Some(0.01),
            vendor_wallet: None,
        };
        registry.register(entry.clone()).unwrap();
        let result = registry.register(entry);
        assert_eq!(
            result,
            Err(RegistrationError::DuplicateId("my-api".to_string()))
        );
    }

    #[test]
    fn test_register_invalid_endpoint() {
        let mut registry = ServiceRegistry::empty();
        let entry = ServiceEntry {
            id: "bad-endpoint".to_string(),
            name: "Bad".to_string(),
            category: "data".to_string(),
            endpoint: "http://insecure.example.com".to_string(),
            x402_enabled: true,
            internal: false,
            description: None,
            pricing_label: "$0.01/request".to_string(),
            chains: vec!["solana".to_string()],
            source: "api".to_string(),
            healthy: None,
            price_per_request_usdc: Some(0.01),
            vendor_wallet: None,
        };
        assert_eq!(
            registry.register(entry),
            Err(RegistrationError::InvalidEndpoint)
        );
    }

    #[test]
    fn test_register_empty_id_fails() {
        let mut registry = ServiceRegistry::empty();
        let entry = ServiceEntry {
            id: "".to_string(),
            name: "No ID".to_string(),
            category: "data".to_string(),
            endpoint: "https://example.com".to_string(),
            x402_enabled: true,
            internal: false,
            description: None,
            pricing_label: "$0.01/request".to_string(),
            chains: vec!["solana".to_string()],
            source: "api".to_string(),
            healthy: None,
            price_per_request_usdc: Some(0.01),
            vendor_wallet: None,
        };
        assert_eq!(registry.register(entry), Err(RegistrationError::InvalidId));
    }

    #[test]
    fn test_register_invalid_price_fails() {
        let mut registry = ServiceRegistry::empty();
        let entry = ServiceEntry {
            id: "no-price".to_string(),
            name: "No Price".to_string(),
            category: "data".to_string(),
            endpoint: "https://example.com".to_string(),
            x402_enabled: true,
            internal: false,
            description: None,
            pricing_label: "$0.00/request".to_string(),
            chains: vec!["solana".to_string()],
            source: "api".to_string(),
            healthy: None,
            price_per_request_usdc: None,
            vendor_wallet: None,
        };
        assert_eq!(
            registry.register(entry),
            Err(RegistrationError::InvalidPrice)
        );
    }

    /// Regression for the SSRF gap where `from_toml` skipped validation:
    /// loading a `services.toml` with an `http://` endpoint for an
    /// external service must now fail, not be silently accepted.
    #[test]
    fn from_toml_rejects_external_service_with_http_endpoint() {
        let bad = r#"
[services.bad-external]
name = "Bad External"
endpoint = "http://10.0.0.1/internal"
category = "data"
x402_enabled = true
internal = false
pricing_label = "$0.001"
price_per_request_usdc = 0.001
"#;
        let err = ServiceRegistry::from_toml(bad).expect_err("must reject http external endpoint");
        match err {
            ServiceRegistryError::InvalidEntry { id, source } => {
                assert_eq!(id, "bad-external");
                assert_eq!(source, RegistrationError::InvalidEndpoint);
            }
            other => panic!("expected InvalidEntry, got {other:?}"),
        }
    }

    #[test]
    fn from_toml_rejects_external_service_with_zero_price() {
        let bad = r#"
[services.zero-price]
name = "Zero Price"
endpoint = "https://example.com/api"
category = "data"
x402_enabled = true
internal = false
pricing_label = "$0"
price_per_request_usdc = 0.0
"#;
        let err = ServiceRegistry::from_toml(bad).expect_err("must reject zero price");
        match err {
            ServiceRegistryError::InvalidEntry { id, source } => {
                assert_eq!(id, "zero-price");
                assert_eq!(source, RegistrationError::InvalidPrice);
            }
            other => panic!("expected InvalidEntry, got {other:?}"),
        }
    }

    #[test]
    fn from_toml_rejects_invalid_id_charset() {
        // Capital letters and other non-allowlist chars must be rejected.
        let bad = r#"
[services."BadId"]
name = "Bad"
endpoint = "https://example.com/api"
category = "data"
x402_enabled = true
internal = false
pricing_label = "$0.001"
price_per_request_usdc = 0.001
"#;
        let err = ServiceRegistry::from_toml(bad).expect_err("must reject invalid id");
        match err {
            ServiceRegistryError::InvalidEntry { source, .. } => {
                assert_eq!(source, RegistrationError::InvalidId);
            }
            other => panic!("expected InvalidEntry, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // vendor_wallet (settlement-platform P1: per-service payment recipient)
    // -----------------------------------------------------------------------

    /// A valid base58 Solana pubkey (the exact-scheme golden vector's pay_to).
    const VALID_VENDOR_WALLET: &str = "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM";

    #[test]
    fn from_toml_parses_vendor_wallet() {
        let toml = format!(
            r#"
[services.vendor-svc]
name = "Vendor Service"
endpoint = "https://vendor.example.com/api"
category = "data"
x402_enabled = true
internal = false
pricing_label = "$0.01/request"
price_per_request_usdc = 0.01
vendor_wallet = "{VALID_VENDOR_WALLET}"
"#
        );
        let registry = ServiceRegistry::from_toml(&toml).expect("valid vendor_wallet must load");
        assert_eq!(
            registry.get("vendor-svc").unwrap().vendor_wallet.as_deref(),
            Some(VALID_VENDOR_WALLET)
        );
    }

    #[test]
    fn vendor_wallet_defaults_to_none_when_absent() {
        let registry = ServiceRegistry::from_toml(SAMPLE_TOML).unwrap();
        for svc in registry.all() {
            assert_eq!(
                svc.vendor_wallet, None,
                "service '{}' must default to no vendor wallet",
                svc.id
            );
        }
    }

    /// Fail closed at load time: a vendor_wallet that is not a valid Solana
    /// pubkey must reject the whole registry, not surface later as an
    /// unpayable 402 or a misdirected transfer.
    #[test]
    fn from_toml_rejects_invalid_vendor_wallet() {
        let bad = r#"
[services.bad-vendor]
name = "Bad Vendor"
endpoint = "https://vendor.example.com/api"
category = "data"
x402_enabled = true
internal = false
pricing_label = "$0.01/request"
price_per_request_usdc = 0.01
vendor_wallet = "not-a-valid-pubkey"
"#;
        let err = ServiceRegistry::from_toml(bad).expect_err("must reject invalid vendor_wallet");
        match err {
            ServiceRegistryError::InvalidEntry { id, source } => {
                assert_eq!(id, "bad-vendor");
                assert_eq!(source, RegistrationError::InvalidVendorWallet);
            }
            other => panic!("expected InvalidEntry, got {other:?}"),
        }
    }

    /// Valid base58 but not 32 bytes — must also reject.
    #[test]
    fn from_toml_rejects_short_base58_vendor_wallet() {
        let bad = r#"
[services.short-vendor]
name = "Short Vendor"
endpoint = "https://vendor.example.com/api"
category = "data"
x402_enabled = true
internal = false
pricing_label = "$0.01/request"
price_per_request_usdc = 0.01
vendor_wallet = "abc"
"#;
        let err = ServiceRegistry::from_toml(bad).expect_err("must reject short vendor_wallet");
        match err {
            ServiceRegistryError::InvalidEntry { source, .. } => {
                assert_eq!(source, RegistrationError::InvalidVendorWallet);
            }
            other => panic!("expected InvalidEntry, got {other:?}"),
        }
    }

    /// vendor_wallet on an internal service would be silently ignored by the
    /// chat pipeline (this P1 only wires the proxy path) — reject it outright
    /// so config cannot claim a vendor gets paid when the gateway pays itself.
    #[test]
    fn from_toml_rejects_vendor_wallet_on_internal_service() {
        let bad = format!(
            r#"
[services.internal-vendor]
name = "Internal With Vendor"
endpoint = "/v1/chat/completions"
category = "intelligence"
x402_enabled = true
internal = true
vendor_wallet = "{VALID_VENDOR_WALLET}"
"#
        );
        let err = ServiceRegistry::from_toml(&bad)
            .expect_err("must reject vendor_wallet on internal service");
        match err {
            ServiceRegistryError::InvalidEntry { source, .. } => {
                assert_eq!(source, RegistrationError::VendorWalletOnInternalService);
            }
            other => panic!("expected InvalidEntry, got {other:?}"),
        }
    }

    fn external_entry_with_vendor_wallet(vendor_wallet: Option<String>) -> ServiceEntry {
        ServiceEntry {
            id: "vendor-svc".to_string(),
            name: "Vendor Service".to_string(),
            category: "data".to_string(),
            endpoint: "https://vendor.example.com/api".to_string(),
            x402_enabled: true,
            internal: false,
            description: None,
            pricing_label: "$0.01/request".to_string(),
            chains: vec!["solana".to_string()],
            source: "api".to_string(),
            healthy: None,
            price_per_request_usdc: Some(0.01),
            vendor_wallet,
        }
    }

    #[test]
    fn register_accepts_valid_vendor_wallet() {
        let mut registry = ServiceRegistry::empty();
        let entry = external_entry_with_vendor_wallet(Some(VALID_VENDOR_WALLET.to_string()));
        assert!(registry.register(entry).is_ok());
        assert_eq!(
            registry.get("vendor-svc").unwrap().vendor_wallet.as_deref(),
            Some(VALID_VENDOR_WALLET)
        );
    }

    #[test]
    fn register_rejects_invalid_vendor_wallet() {
        let mut registry = ServiceRegistry::empty();
        let entry = external_entry_with_vendor_wallet(Some("0OIl-not-base58".to_string()));
        assert_eq!(
            registry.register(entry),
            Err(RegistrationError::InvalidVendorWallet)
        );
    }

    #[test]
    fn register_rejects_vendor_wallet_on_internal_service() {
        let mut registry = ServiceRegistry::empty();
        let mut entry = external_entry_with_vendor_wallet(Some(VALID_VENDOR_WALLET.to_string()));
        entry.internal = true;
        entry.endpoint = "/v1/test".to_string();
        assert_eq!(
            registry.register(entry),
            Err(RegistrationError::VendorWalletOnInternalService)
        );
    }

    #[test]
    fn from_toml_accepts_internal_service_without_price() {
        // Internal services are billed per-token via the chat pipeline; their
        // `price_per_request_usdc` is optional and a missing field must NOT
        // cause from_toml to reject the entry.
        let ok = r#"
[services.llm-gateway]
name = "LLM Intelligence"
endpoint = "/v1/chat/completions"
category = "intelligence"
x402_enabled = true
internal = true
"#;
        let registry = ServiceRegistry::from_toml(ok).expect("internal-without-price must load");
        assert_eq!(registry.all().len(), 1);
    }
}
