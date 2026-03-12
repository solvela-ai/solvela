//! x402 Service Marketplace registry types.
//!
//! `ServiceRegistry` loads `config/services.toml` and provides the list of
//! registered x402-compatible services for the `GET /v1/services` endpoint.
//! Supports runtime registration via `register()` for the admin API.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum ServiceRegistryError {
    #[error("failed to parse services.toml: {0}")]
    ParseError(#[from] toml::de::Error),
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
    pub fn from_toml(toml_str: &str) -> Result<Self, ServiceRegistryError> {
        let raw: RawServices = toml::from_str(toml_str)?;

        let mut services: Vec<ServiceEntry> = raw
            .services
            .into_iter()
            .map(|(id, raw)| ServiceEntry {
                id,
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
            })
            .collect();

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
        // Validate id: non-empty, max 64, [a-z0-9\-] only
        if entry.id.is_empty()
            || entry.id.len() > 64
            || !entry
                .id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(RegistrationError::InvalidId);
        }

        // Check uniqueness
        if self.services.iter().any(|s| s.id == entry.id) {
            return Err(RegistrationError::DuplicateId(entry.id));
        }

        // Validate name
        if entry.name.is_empty() || entry.name.len() > 128 {
            return Err(RegistrationError::InvalidName);
        }

        // Validate category
        if entry.category.is_empty() || entry.category.len() > 32 {
            return Err(RegistrationError::InvalidCategory);
        }

        // Validate endpoint
        if !entry.endpoint.starts_with("https://") {
            return Err(RegistrationError::InvalidEndpoint);
        }

        // Validate price
        match entry.price_per_request_usdc {
            Some(price) if price > 0.0 => {}
            _ => return Err(RegistrationError::InvalidPrice),
        }

        self.services.push(entry);
        // Re-sort to maintain deterministic order.
        self.services.sort_by(|a, b| a.id.cmp(&b.id));

        Ok(())
    }

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
        };
        assert_eq!(
            registry.register(entry),
            Err(RegistrationError::InvalidPrice)
        );
    }
}
