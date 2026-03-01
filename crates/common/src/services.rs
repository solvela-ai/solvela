//! x402 Service Marketplace registry types.
//!
//! `ServiceRegistry` loads `config/services.toml` and provides the list of
//! registered x402-compatible services for the `GET /v1/services` endpoint.

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
}

// ---------------------------------------------------------------------------
// Registry
// ---------------------------------------------------------------------------

/// In-memory registry of x402-compatible services loaded from `services.toml`.
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
}
