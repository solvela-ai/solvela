use std::collections::HashMap;

use serde::Deserialize;
use thiserror::Error;

use solvela_protocol::{
    CostBreakdown, ModelRegistration, PLATFORM_FEE_MULTIPLIER, PLATFORM_FEE_PERCENT,
};

/// Errors from the model registry.
#[derive(Debug, Error)]
pub enum ModelRegistryError {
    #[error("model not found: {0}")]
    NotFound(String),

    #[error("failed to parse model config: {0}")]
    ParseError(String),
}

/// TOML-deserialized model entry from `config/models.toml`.
///
/// `deny_unknown_fields` catches typos at startup — pre-fix, a typo like
/// `input_cost_per_milion = 2.50` would silently leave the real field
/// at its default (zero) and produce a $0 cost quote.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub struct ModelsConfig {
    pub models: HashMap<String, ModelEntry>,
}

/// In-memory model registry loaded from TOML config.
#[derive(Debug)]
pub struct ModelRegistry {
    models: HashMap<String, ModelRegistration>,
}

impl ModelRegistry {
    /// Load the registry from a TOML string (contents of `config/models.toml`).
    ///
    /// Validation pass at load time rejects:
    /// - Non-finite (`NaN`/`±∞`) or negative `input_cost_per_million` /
    ///   `output_cost_per_million`. Without this guard, a corrupt TOML
    ///   entry would silently propagate `NaN` through `estimate_cost` and
    ///   ultimately result in a `$0` quote (gateway PR #191 fixed the
    ///   downstream cast in `compute_actual_atomic_cost`; this is the
    ///   upstream guard at config-load time).
    /// - Duplicate canonical keys (`provider/model_id`) where the two
    ///   entries have **different** pricing. Equal-pricing duplicates are
    ///   allowed because the TOML lets operators name the same underlying
    ///   provider model under two human-friendly aliases.
    pub fn from_toml(toml_str: &str) -> Result<Self, ModelRegistryError> {
        let config: ModelsConfig =
            toml::from_str(toml_str).map_err(|e| ModelRegistryError::ParseError(e.to_string()))?;

        // Validation pass: every entry must have finite, non-negative pricing.
        for (key, entry) in &config.models {
            if !entry.input_cost_per_million.is_finite() || entry.input_cost_per_million < 0.0 {
                return Err(ModelRegistryError::ParseError(format!(
                    "model {key:?}: input_cost_per_million must be finite and non-negative, got {}",
                    entry.input_cost_per_million
                )));
            }
            if !entry.output_cost_per_million.is_finite() || entry.output_cost_per_million < 0.0 {
                return Err(ModelRegistryError::ParseError(format!(
                    "model {key:?}: output_cost_per_million must be finite and non-negative, got {}",
                    entry.output_cost_per_million
                )));
            }
        }

        // Build the registry. Each entry is registered under both its TOML
        // key and the canonical `provider/model_id`, so a single model is
        // typically reachable via two lookup keys. Two TOML entries that
        // reduce to the same canonical key are tolerated only when their
        // pricing matches — otherwise the silent last-write-wins behavior
        // of `HashMap::collect` would non-deterministically pick one.
        let mut models: HashMap<String, ModelRegistration> = HashMap::new();
        for (key, entry) in config.models {
            let id = format!("{}/{}", entry.provider, entry.model_id);
            let info = ModelRegistration {
                id: id.clone(),
                provider: entry.provider,
                model_id: entry.model_id,
                display_name: entry.display_name,
                input_cost_per_million: entry.input_cost_per_million,
                output_cost_per_million: entry.output_cost_per_million,
                context_window: entry.context_window,
                supports_streaming: entry.supports_streaming,
                supports_tools: entry.supports_tools,
                supports_vision: entry.supports_vision,
                reasoning: entry.reasoning,
                supports_structured_output: entry.supports_structured_output,
                supports_batch: entry.supports_batch,
                max_output_tokens: entry.max_output_tokens,
            };

            // Reject canonical-key collisions only when they would change
            // the resolved pricing.
            if let Some(existing) = models.get(&id) {
                let pricing_matches =
                    (existing.input_cost_per_million - info.input_cost_per_million).abs()
                        < f64::EPSILON
                        && (existing.output_cost_per_million - info.output_cost_per_million).abs()
                            < f64::EPSILON;
                if !pricing_matches {
                    return Err(ModelRegistryError::ParseError(format!(
                        "duplicate canonical key {id:?} with conflicting pricing: \
                         entry {key:?} has input={}/output={} but a previous entry \
                         registered input={}/output={}",
                        info.input_cost_per_million,
                        info.output_cost_per_million,
                        existing.input_cost_per_million,
                        existing.output_cost_per_million
                    )));
                }
            }

            // TOML key gets its own entry too (legacy). It's expected to be
            // unique per TOML — `toml::from_str` into HashMap already
            // enforces that — so no collision check is needed here.
            models.insert(key, info.clone());
            models.insert(id, info);
        }

        Ok(Self { models })
    }

    /// Look up a model by its ID (e.g., "openai/gpt-4o" or "openai-gpt-4o").
    pub fn get(&self, model_id: &str) -> Option<&ModelRegistration> {
        self.models.get(model_id)
    }

    /// Resolve a BARE Anthropic model id (the `model_id` field, e.g.
    /// `"claude-sonnet-4-6"` or `"claude-haiku-4-5-20251001"`) to its registered
    /// canonical entry.
    ///
    /// This is the inbound-id contract for `POST /v1/messages`: Claude Code (and
    /// every native `api.anthropic.com` client) addresses models by their bare
    /// Anthropic id, NOT by the gateway-canonical `anthropic/<id>` form used on
    /// `/v1/models` and `/v1/chat/completions`. The bare id is never a registry
    /// key (the registry is keyed by the TOML key and the canonical
    /// `provider/model_id`), so a bare-id lookup via [`get`](Self::get) misses.
    /// This walks the registered models and matches an `anthropic`-provider entry
    /// whose `model_id` equals the bare id.
    ///
    /// Provider-scoped to `anthropic` on purpose: a bare id is meaningful as an
    /// Anthropic-native address only, and scoping prevents an accidental
    /// cross-provider match if two providers ever share a bare `model_id`.
    /// Returns `None` for an unknown bare id (the caller then surfaces
    /// `ModelNotFound` — fail closed, never default-route).
    pub fn resolve_anthropic_model_id(&self, bare_model_id: &str) -> Option<&ModelRegistration> {
        self.models
            .values()
            .find(|m| m.provider == "anthropic" && m.model_id == bare_model_id)
    }

    /// Return all registered models.
    pub fn all(&self) -> Vec<&ModelRegistration> {
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
        let provider_cost = input_cost + output_cost;
        let total_with_fee = provider_cost * PLATFORM_FEE_MULTIPLIER;
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
    fn test_registry_stores_raw_provider_rates() {
        // ModelRegistry stores raw provider rates as-loaded from TOML.
        // The 5% platform fee is applied at the boundary by estimate_cost
        // and compute_actual_atomic_cost — never baked into ModelRegistration.
        let registry = ModelRegistry::from_toml(TEST_TOML).unwrap();
        let model = registry.get("openai/gpt-4o").unwrap();
        assert!(
            (model.input_cost_per_million - 2.50).abs() < 0.001,
            "got {}",
            model.input_cost_per_million
        );
        assert!(
            (model.output_cost_per_million - 10.00).abs() < 0.001,
            "got {}",
            model.output_cost_per_million
        );
    }

    #[test]
    fn test_estimate_cost_applies_exactly_one_5_percent_fee() {
        // Pin the effective fee at exactly 5% (not 10.25%). This is the
        // regression test for the double-application bug fixed in Option A.
        let registry = ModelRegistry::from_toml(TEST_TOML).unwrap();
        // 1M input tokens @ $2.50/M = $2.50 provider; 0 output tokens.
        let cost = registry
            .estimate_cost("openai/gpt-4o", 1_000_000, 0)
            .unwrap();
        let provider: f64 = cost.provider_cost.parse().unwrap();
        let fee: f64 = cost.platform_fee.parse().unwrap();
        let total: f64 = cost.total.parse().unwrap();
        assert!(
            (provider - 2.50).abs() < 1e-6,
            "provider_cost should be 2.50, got {provider}"
        );
        assert!(
            (fee - 0.125).abs() < 1e-6,
            "platform_fee should be 0.125 (5% of 2.50), got {fee}"
        );
        assert!(
            (total - 2.625).abs() < 1e-6,
            "total should be 2.625 (2.50 * 1.05), got {total}"
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

    /// `/v1/messages` inbound-id contract: a BARE Anthropic id (the form Claude
    /// Code sends, e.g. `claude-sonnet-4-6`) is NOT a registry key (only the TOML
    /// key and the canonical `anthropic/<id>` are), so `get()` misses — but
    /// `resolve_anthropic_model_id` resolves it to the canonical entry. This is
    /// the fix for the native-passthrough non-engagement: without it a bare id
    /// fails closed with `ModelNotFound`.
    #[test]
    fn resolve_anthropic_model_id_maps_bare_ids_to_canonical_entries() {
        const ANTHROPIC_TOML: &str = r#"
[models.anthropic-claude-sonnet-4-6]
provider = "anthropic"
model_id = "claude-sonnet-4-6"
display_name = "Claude Sonnet 4.6"
input_cost_per_million = 3.00
output_cost_per_million = 15.00
context_window = 200000

[models.anthropic-claude-haiku-4-5]
provider = "anthropic"
model_id = "claude-haiku-4-5-20251001"
display_name = "Claude Haiku 4.5"
input_cost_per_million = 1.00
output_cost_per_million = 5.00
context_window = 200000

[models.openai-gpt-4o]
provider = "openai"
model_id = "gpt-4o"
display_name = "GPT-4o"
input_cost_per_million = 2.50
output_cost_per_million = 10.00
context_window = 128000
"#;
        let registry = ModelRegistry::from_toml(ANTHROPIC_TOML).unwrap();

        // The bare id is NOT a plain key — this is the gap the helper closes.
        assert!(
            registry.get("claude-sonnet-4-6").is_none(),
            "bare id must not be a direct registry key"
        );

        // The helper resolves the bare id to the canonical entry, and the
        // canonical entry's `model_id` is the bare id the relay must forward
        // upstream to api.anthropic.com.
        let sonnet = registry
            .resolve_anthropic_model_id("claude-sonnet-4-6")
            .expect("bare sonnet id must resolve");
        assert_eq!(sonnet.id, "anthropic/claude-sonnet-4-6");
        assert_eq!(sonnet.model_id, "claude-sonnet-4-6");
        assert_eq!(sonnet.provider, "anthropic");

        // A dated bare id (ANTHROPIC_SMALL_FAST_MODEL / haiku) also resolves.
        let haiku = registry
            .resolve_anthropic_model_id("claude-haiku-4-5-20251001")
            .expect("bare haiku id must resolve");
        assert_eq!(haiku.id, "anthropic/claude-haiku-4-5-20251001");
        assert_eq!(haiku.model_id, "claude-haiku-4-5-20251001");

        // Provider-scoped: a bare OpenAI model_id does NOT resolve via the
        // Anthropic-only helper.
        assert!(
            registry.resolve_anthropic_model_id("gpt-4o").is_none(),
            "non-Anthropic bare id must not resolve via the Anthropic helper"
        );

        // Unknown bare id → None (caller fails closed with ModelNotFound).
        assert!(registry
            .resolve_anthropic_model_id("claude-does-not-exist")
            .is_none());
    }

    /// R1 regression: NaN/Infinity/negative pricing must be rejected at
    /// load time, not silently propagated through `estimate_cost` to the
    /// chat path's f64-as-u64 saturating cast.
    #[test]
    fn from_toml_rejects_nan_negative_and_infinite_pricing() {
        for (label, body) in [
            (
                "NaN input cost",
                r#"
[models.bad]
provider = "test"
model_id = "bad"
display_name = "Bad"
input_cost_per_million = nan
output_cost_per_million = 1.0
context_window = 4096
"#,
            ),
            (
                "Infinity output cost",
                r#"
[models.bad]
provider = "test"
model_id = "bad"
display_name = "Bad"
input_cost_per_million = 1.0
output_cost_per_million = inf
context_window = 4096
"#,
            ),
            (
                "negative input cost",
                r#"
[models.bad]
provider = "test"
model_id = "bad"
display_name = "Bad"
input_cost_per_million = -0.50
output_cost_per_million = 1.0
context_window = 4096
"#,
            ),
        ] {
            let err =
                ModelRegistry::from_toml(body).expect_err(&format!("{label} must be rejected"));
            match err {
                ModelRegistryError::ParseError(msg) => {
                    assert!(
                        msg.contains("input_cost_per_million")
                            || msg.contains("output_cost_per_million"),
                        "{label}: error must name the offending field, got: {msg}"
                    );
                }
                other => panic!("{label}: expected ParseError, got {other:?}"),
            }
        }
    }

    /// R1 regression: an unknown TOML field (e.g. typo `input_cost_per_milion`)
    /// must error at load, not silently default the real field to zero.
    #[test]
    fn from_toml_rejects_unknown_field_typo() {
        let body = r#"
[models.typo]
provider = "test"
model_id = "typo"
display_name = "Typo"
input_cost_per_milion = 2.50
output_cost_per_million = 5.00
context_window = 4096
"#;
        let err = ModelRegistry::from_toml(body)
            .expect_err("unknown field must be rejected by deny_unknown_fields");
        let msg = match err {
            ModelRegistryError::ParseError(m) => m,
            other => panic!("expected ParseError, got {other:?}"),
        };
        assert!(
            msg.contains("unknown") || msg.contains("input_cost_per_milion"),
            "error must surface the unknown field, got: {msg}"
        );
    }

    /// R1 regression: two entries with the same canonical key but
    /// **different** pricing must be rejected. Equal-pricing duplicates
    /// are allowed (this is what the production `claude-sonnet-4-6` /
    /// `claude-sonnet-4-5` pair was relying on; either entry is safe to
    /// land in the registry first because the resolved pricing is
    /// identical).
    #[test]
    fn from_toml_rejects_canonical_collision_with_conflicting_pricing() {
        let body = r#"
[models.first]
provider = "test"
model_id = "shared"
display_name = "First"
input_cost_per_million = 1.00
output_cost_per_million = 2.00
context_window = 4096

[models.second]
provider = "test"
model_id = "shared"
display_name = "Second"
input_cost_per_million = 9.99
output_cost_per_million = 2.00
context_window = 4096
"#;
        let err = ModelRegistry::from_toml(body)
            .expect_err("canonical-key collision with conflicting pricing must error");
        let msg = match err {
            ModelRegistryError::ParseError(m) => m,
            other => panic!("expected ParseError, got {other:?}"),
        };
        assert!(
            msg.contains("duplicate canonical key") && msg.contains("test/shared"),
            "error must mention the canonical key, got: {msg}"
        );
    }

    #[test]
    fn from_toml_allows_canonical_collision_with_identical_pricing() {
        let body = r#"
[models.first]
provider = "test"
model_id = "shared"
display_name = "First"
input_cost_per_million = 3.00
output_cost_per_million = 15.00
context_window = 4096

[models.second]
provider = "test"
model_id = "shared"
display_name = "Second"
input_cost_per_million = 3.00
output_cost_per_million = 15.00
context_window = 4096
"#;
        let registry = ModelRegistry::from_toml(body)
            .expect("equal-pricing duplicates should load successfully");
        // Both TOML keys and the shared canonical key all resolve.
        assert!(registry.get("first").is_some());
        assert!(registry.get("second").is_some());
        assert!(registry.get("test/shared").is_some());
    }
}
