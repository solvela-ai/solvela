use std::collections::HashMap;

use serde::Deserialize;
use thiserror::Error;

use solvela_protocol::{CostBreakdown, ModelInfo, PLATFORM_FEE_MULTIPLIER, PLATFORM_FEE_PERCENT};

/// Errors from the model registry.
#[derive(Debug, Error)]
pub enum ModelRegistryError {
    #[error("model not found: {0}")]
    NotFound(String),

    #[error("failed to parse model config: {0}")]
    ParseError(String),
}

/// TOML-deserialized model entry from `config/models.toml`.
#[derive(Debug, Clone, Deserialize)]
pub struct ModelEntry {
    pub provider: String,
    pub model_id: String,
    pub display_name: String,
    pub input_cost_per_million: f64,
    pub output_cost_per_million: f64,
    pub context_window: u32,
    #[serde(default)]
    pub supports_streaming: bool,
    #[serde(default)]
    pub supports_tools: bool,
    #[serde(default)]
    pub supports_vision: bool,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub supports_structured_output: bool,
    #[serde(default)]
    pub supports_batch: bool,
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
}

/// TOML top-level structure: `[models.<id>]`.
#[derive(Debug, Deserialize)]
pub struct ModelsConfig {
    pub models: HashMap<String, ModelEntry>,
}

/// In-memory model registry loaded from TOML config.
pub struct ModelRegistry {
    models: HashMap<String, ModelInfo>,
}

impl ModelRegistry {
    /// Load the registry from a TOML string (contents of `config/models.toml`).
    pub fn from_toml(toml_str: &str) -> Result<Self, ModelRegistryError> {
        let config: ModelsConfig =
            toml::from_str(toml_str).map_err(|e| ModelRegistryError::ParseError(e.to_string()))?;

        let models = config
            .models
            .into_iter()
            .flat_map(|(key, entry)| {
                let id = format!("{}/{}", entry.provider, entry.model_id);
                let info = ModelInfo {
                    id: id.clone(),
                    provider: entry.provider,
                    model_id: entry.model_id,
                    display_name: entry.display_name,
                    input_cost_per_million: entry.input_cost_per_million * PLATFORM_FEE_MULTIPLIER,
                    output_cost_per_million: entry.output_cost_per_million
                        * PLATFORM_FEE_MULTIPLIER,
                    context_window: entry.context_window,
                    supports_streaming: entry.supports_streaming,
                    supports_tools: entry.supports_tools,
                    supports_vision: entry.supports_vision,
                    reasoning: entry.reasoning,
                    supports_structured_output: entry.supports_structured_output,
                    supports_batch: entry.supports_batch,
                    max_output_tokens: entry.max_output_tokens,
                };
                // Register under both the key and the canonical id
                vec![(key, info.clone()), (id, info)]
            })
            .collect();

        Ok(Self { models })
    }

    /// Look up a model by its ID (e.g., "openai/gpt-4o" or "openai-gpt-4o").
    pub fn get(&self, model_id: &str) -> Option<&ModelInfo> {
        self.models.get(model_id)
    }

    /// Return all registered models.
    pub fn all(&self) -> Vec<&ModelInfo> {
        // Deduplicate — each model is stored under two keys
        let mut seen = std::collections::HashSet::new();
        self.models
            .values()
            .filter(|m| seen.insert(&m.id))
            .collect()
    }

    /// Estimate cost for a request and return a breakdown.
    pub fn estimate_cost(
        &self,
        model_id: &str,
        input_tokens: u32,
        output_tokens: u32,
    ) -> Result<CostBreakdown, ModelRegistryError> {
        let model = self
            .get(model_id)
            .ok_or_else(|| ModelRegistryError::NotFound(model_id.to_string()))?;

        let input_cost = (input_tokens as f64 / 1_000_000.0) * model.input_cost_per_million;
        let output_cost = (output_tokens as f64 / 1_000_000.0) * model.output_cost_per_million;
        let total_with_fee = input_cost + output_cost;
        let provider_cost = total_with_fee / PLATFORM_FEE_MULTIPLIER;
        let platform_fee = total_with_fee - provider_cost;

        Ok(CostBreakdown {
            provider_cost: format!("{provider_cost:.6}"),
            platform_fee: format!("{platform_fee:.6}"),
            total: format!("{total_with_fee:.6}"),
            currency: "USDC".to_string(),
            fee_percent: PLATFORM_FEE_PERCENT,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_TOML: &str = r#"
[models.openai-gpt-4o]
provider = "openai"
model_id = "gpt-4o"
display_name = "GPT-4o"
input_cost_per_million = 2.50
output_cost_per_million = 10.00
context_window = 128000
supports_streaming = true
supports_tools = true
supports_vision = true

[models.deepseek-chat]
provider = "deepseek"
model_id = "deepseek-chat"
display_name = "DeepSeek V3.2 Chat"
input_cost_per_million = 0.28
output_cost_per_million = 0.42
context_window = 128000
supports_streaming = true
"#;

    #[test]
    fn test_load_from_toml() {
        let registry = ModelRegistry::from_toml(TEST_TOML).unwrap();
        assert!(registry.get("openai/gpt-4o").is_some());
        assert!(registry.get("openai-gpt-4o").is_some());
        assert!(registry.get("deepseek/deepseek-chat").is_some());
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_pricing_includes_fee() {
        let registry = ModelRegistry::from_toml(TEST_TOML).unwrap();
        let model = registry.get("openai/gpt-4o").unwrap();

        // Provider charges $2.50/M input — with 5% fee user pays $2.625/M
        let expected = 2.50 * PLATFORM_FEE_MULTIPLIER;
        assert!(
            (model.input_cost_per_million - expected).abs() < 0.001,
            "got {}",
            model.input_cost_per_million
        );
    }

    #[test]
    fn test_cost_estimate() {
        let registry = ModelRegistry::from_toml(TEST_TOML).unwrap();
        let cost = registry.estimate_cost("openai/gpt-4o", 1000, 500).unwrap();
        assert_eq!(cost.currency, "USDC");
        assert_eq!(cost.fee_percent, 5);

        // Total should be non-zero
        let total: f64 = cost.total.parse().unwrap();
        assert!(total > 0.0);
    }

    #[test]
    fn test_all_models() {
        let registry = ModelRegistry::from_toml(TEST_TOML).unwrap();
        let all = registry.all();
        assert_eq!(all.len(), 2);
    }
}
