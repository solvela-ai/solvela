use serde::de::Deserializer;
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

use crate::PLATFORM_FEE_PERCENT;

/// Information about a supported model.
///
/// Wire format is nested (`pricing.input_per_million`,
/// `capabilities.streaming`) to match what the gateway emits at
/// `GET /v1/models` and what the sibling SDKs (Python, Go, TypeScript)
/// parse. Pre-0.3.0 versions of this crate derived `Serialize`/`Deserialize`
/// directly on flat top-level fields that did not match the gateway, so
/// SDK consumers silently parsed every response with all-zero pricing and
/// all-false capabilities.
///
/// Rust-side fields stay flat (`input_cost_per_million`, `supports_streaming`,
/// …) to preserve the existing gateway-side struct API (router registry,
/// providers, cost tests). The nested wire shape lives in the custom
/// `Serialize`/`Deserialize` impls via the private `ModelInfoWire`.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelInfo {
    pub id: String,
    pub provider: String,
    pub model_id: String,
    pub display_name: String,
    pub input_cost_per_million: f64,
    pub output_cost_per_million: f64,
    pub context_window: u32,
    pub supports_streaming: bool,
    pub supports_tools: bool,
    pub supports_vision: bool,
    pub reasoning: bool,
    pub supports_structured_output: bool,
    pub supports_batch: bool,
    pub max_output_tokens: Option<u32>,
}

#[derive(Default, Serialize, Deserialize)]
struct Capabilities {
    #[serde(default)]
    streaming: bool,
    #[serde(default)]
    tools: bool,
    #[serde(default)]
    vision: bool,
    #[serde(default)]
    reasoning: bool,
}

#[derive(Serialize, Deserialize)]
struct Pricing {
    #[serde(default)]
    input_per_million: f64,
    #[serde(default)]
    output_per_million: f64,
    #[serde(default = "default_currency")]
    currency: String,
    #[serde(default = "default_fee_percent")]
    fee_percent: u8,
}

impl Default for Pricing {
    fn default() -> Self {
        Self {
            input_per_million: 0.0,
            output_per_million: 0.0,
            currency: default_currency(),
            fee_percent: default_fee_percent(),
        }
    }
}

fn default_currency() -> String {
    "USDC".to_string()
}

fn default_fee_percent() -> u8 {
    PLATFORM_FEE_PERCENT
}

#[derive(Serialize, Deserialize)]
struct ModelInfoWire {
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    object: Option<String>,
    provider: String,
    display_name: String,
    context_window: u32,
    #[serde(default)]
    capabilities: Capabilities,
    #[serde(default)]
    pricing: Pricing,
}

impl Serialize for ModelInfo {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let wire = ModelInfoWire {
            id: self.id.clone(),
            object: Some("model".to_string()),
            provider: self.provider.clone(),
            display_name: self.display_name.clone(),
            context_window: self.context_window,
            capabilities: Capabilities {
                streaming: self.supports_streaming,
                tools: self.supports_tools,
                vision: self.supports_vision,
                reasoning: self.reasoning,
            },
            pricing: Pricing {
                input_per_million: self.input_cost_per_million,
                output_per_million: self.output_cost_per_million,
                currency: default_currency(),
                fee_percent: PLATFORM_FEE_PERCENT,
            },
        };
        wire.serialize(serializer)
    }
}

/// Deserialize from the gateway's nested wire shape into a flat `ModelInfo`.
///
/// **SDK-consumer-only path.** The following internal-only fields are NOT
/// carried on the wire and default to empty / `false` / `None`:
///
/// - `model_id` — provider-side identifier (gateway emits `id` instead)
/// - `supports_structured_output`
/// - `supports_batch`
/// - `max_output_tokens` — used by gateway cost-cap enforcement
///   (`crates/gateway/src/routes/chat/cost.rs`). Never deserialize a
///   `ModelInfo` and then read this field without re-populating it from
///   the registry (`crates/router/src/models.rs`).
///
/// If a future code path caches `ModelInfo` as JSON or loads it from a
/// fixture file, treat the resulting struct as wire-truth only and merge
/// with the registry before relying on internal-only fields. To make this
/// distinction structural, consider splitting into two types
/// (`ModelRegistration` vs `ModelInfo`).
impl<'de> Deserialize<'de> for ModelInfo {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let w = ModelInfoWire::deserialize(deserializer)?;
        Ok(Self {
            id: w.id,
            provider: w.provider,
            // model_id is not carried on the wire; it's an internal-only
            // provider-side identifier. Leave empty when parsing wire input.
            model_id: String::new(),
            display_name: w.display_name,
            input_cost_per_million: w.pricing.input_per_million,
            output_cost_per_million: w.pricing.output_per_million,
            context_window: w.context_window,
            supports_streaming: w.capabilities.streaming,
            supports_tools: w.capabilities.tools,
            supports_vision: w.capabilities.vision,
            reasoning: w.capabilities.reasoning,
            // Not carried on the wire (gateway never emits them); default
            // to the historical zero/false/None values for backward compat
            // with existing internal call sites.
            supports_structured_output: false,
            supports_batch: false,
            max_output_tokens: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> ModelInfo {
        ModelInfo {
            id: "openai/gpt-4o".to_string(),
            provider: "openai".to_string(),
            model_id: "gpt-4o".to_string(),
            display_name: "GPT-4o".to_string(),
            input_cost_per_million: 2.5,
            output_cost_per_million: 10.0,
            context_window: 128_000,
            supports_streaming: true,
            supports_tools: true,
            supports_vision: true,
            reasoning: false,
            supports_structured_output: true,
            supports_batch: true,
            max_output_tokens: Some(16_384),
        }
    }

    #[test]
    fn serializes_nested_wire_shape() {
        let info = fixture();
        let v = serde_json::to_value(&info).unwrap();

        assert_eq!(v["id"], "openai/gpt-4o");
        assert_eq!(v["object"], "model");
        assert_eq!(v["provider"], "openai");
        assert_eq!(v["display_name"], "GPT-4o");
        assert_eq!(v["context_window"], 128_000);

        // Pricing is nested, NOT flat.
        assert_eq!(v["pricing"]["input_per_million"], 2.5);
        assert_eq!(v["pricing"]["output_per_million"], 10.0);
        assert_eq!(v["pricing"]["currency"], "USDC");
        assert_eq!(v["pricing"]["fee_percent"], PLATFORM_FEE_PERCENT);

        // Capabilities is nested, NOT flat.
        assert_eq!(v["capabilities"]["streaming"], true);
        assert_eq!(v["capabilities"]["tools"], true);
        assert_eq!(v["capabilities"]["vision"], true);
        assert_eq!(v["capabilities"]["reasoning"], false);

        // model_id is internal-only — never on the wire.
        assert!(v.get("model_id").is_none());
        // structured_output / batch / max_output_tokens are internal-only.
        assert!(v.get("supports_structured_output").is_none());
        assert!(v.get("supports_batch").is_none());
        assert!(v.get("max_output_tokens").is_none());
        // Old flat fields must not leak — they were the source of the drift.
        assert!(v.get("input_cost_per_million").is_none());
        assert!(v.get("output_cost_per_million").is_none());
        assert!(v.get("supports_streaming").is_none());
    }

    #[test]
    fn deserializes_actual_gateway_wire_payload() {
        // Exact shape from crates/gateway/src/routes/models.rs:10-42.
        let payload = json!({
            "id": "anthropic/claude-sonnet-4-6",
            "object": "model",
            "provider": "anthropic",
            "display_name": "Claude Sonnet 4.6",
            "context_window": 200_000,
            "pricing": {
                "input_per_million": 3.0,
                "output_per_million": 15.0,
                "currency": "USDC",
                "fee_percent": 5,
            },
            "capabilities": {
                "streaming": true,
                "tools": true,
                "vision": true,
                "reasoning": false,
            },
        });

        let info: ModelInfo = serde_json::from_value(payload).unwrap();

        // The regression class: pricing must NOT be zero, capabilities must
        // NOT be all-false. Pre-fix, derive(Deserialize) parsed every value
        // as zero/false because field paths did not match the wire.
        assert_eq!(info.input_cost_per_million, 3.0);
        assert_eq!(info.output_cost_per_million, 15.0);
        assert!(info.supports_streaming);
        assert!(info.supports_tools);
        assert!(info.supports_vision);
        assert!(!info.reasoning);

        assert_eq!(info.id, "anthropic/claude-sonnet-4-6");
        assert_eq!(info.provider, "anthropic");
        assert_eq!(info.display_name, "Claude Sonnet 4.6");
        assert_eq!(info.context_window, 200_000);

        // Wire payload does not carry these — internal-only fields default.
        assert_eq!(info.model_id, "");
        assert!(!info.supports_structured_output);
        assert!(!info.supports_batch);
        assert_eq!(info.max_output_tokens, None);
    }

    #[test]
    fn deserializes_with_missing_pricing_and_capabilities_blocks() {
        // Defensive: if the gateway ever omits a block, we should land
        // sensible zeros/false rather than panicking on parse.
        let payload = json!({
            "id": "x/y",
            "provider": "x",
            "display_name": "Y",
            "context_window": 8_000,
        });

        let info: ModelInfo = serde_json::from_value(payload).unwrap();
        assert_eq!(info.input_cost_per_million, 0.0);
        assert_eq!(info.output_cost_per_million, 0.0);
        assert!(!info.supports_streaming);
        assert!(!info.supports_tools);
        assert!(!info.supports_vision);
        assert!(!info.reasoning);
    }

    #[test]
    fn deserializes_with_partial_pricing_block_zero_fills_missing_fields() {
        // Defensive: if the response carries the `pricing` block but
        // omits a field (e.g. a buggy gateway response, a middleware
        // that strips fields, or a forward-compatible variant), the
        // missing field defaults to 0.0 / false. This is the intended
        // contract, but it overlaps with the original silent-zero-fill
        // bug class — so this test pins the behavior explicitly and
        // documents it. SDK callers seeing input == output == 0 should
        // treat pricing as "unavailable", NOT "free".
        let payload = json!({
            "id": "x/y",
            "provider": "x",
            "display_name": "Y",
            "context_window": 8_000,
            "pricing": {
                "output_per_million": 10.0,
                // input_per_million intentionally omitted
            },
            "capabilities": {
                "streaming": true,
                // tools, vision, reasoning intentionally omitted
            },
        });

        let info: ModelInfo = serde_json::from_value(payload).unwrap();
        assert_eq!(info.input_cost_per_million, 0.0);
        assert_eq!(info.output_cost_per_million, 10.0);
        assert!(info.supports_streaming);
        assert!(!info.supports_tools);
        assert!(!info.supports_vision);
        assert!(!info.reasoning);
    }

    #[test]
    fn round_trip_preserves_wire_fields() {
        // Round-tripping through the wire loses internal-only fields
        // (model_id, supports_structured_output, supports_batch,
        // max_output_tokens) but preserves everything visible to SDK
        // consumers.
        let original = fixture();
        let json = serde_json::to_string(&original).unwrap();
        let parsed: ModelInfo = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.id, original.id);
        assert_eq!(parsed.provider, original.provider);
        assert_eq!(parsed.display_name, original.display_name);
        assert_eq!(parsed.context_window, original.context_window);
        assert_eq!(
            parsed.input_cost_per_million,
            original.input_cost_per_million
        );
        assert_eq!(
            parsed.output_cost_per_million,
            original.output_cost_per_million
        );
        assert_eq!(parsed.supports_streaming, original.supports_streaming);
        assert_eq!(parsed.supports_tools, original.supports_tools);
        assert_eq!(parsed.supports_vision, original.supports_vision);
        assert_eq!(parsed.reasoning, original.reasoning);

        // Internal-only fields are reset on the wire-side round trip.
        assert_eq!(parsed.model_id, "");
        assert!(!parsed.supports_structured_output);
        assert!(!parsed.supports_batch);
        assert_eq!(parsed.max_output_tokens, None);
    }
}
