use serde::de::Deserializer;
use serde::ser::Serializer;
use serde::{Deserialize, Serialize};

use crate::PLATFORM_FEE_PERCENT;

/// Full, gateway-internal description of a supported model.
///
/// This is the registry-side struct: the model registry
/// (`crates/router/src/models.rs`) builds one per `config/models.toml`
/// entry, providers return them from `supported_models()`, and the cost
/// path (`crates/gateway/src/routes/chat/cost.rs`) reads them to enforce
/// pricing and token caps. It deliberately has **no** `Serialize` /
/// `Deserialize` impl — it is never put on the wire directly.
///
/// To emit a model on the `GET /v1/models` wire, convert to [`ModelInfo`]
/// via `ModelInfo::from(&registration)`. That conversion drops the
/// internal-only fields ([`Self::model_id`], [`Self::supports_structured_output`],
/// [`Self::supports_batch`], [`Self::max_output_tokens`]) that the gateway
/// never emits.
///
/// # Why a separate type (the #229 follow-up)
///
/// Before the split, a single `ModelInfo` played both roles: registry
/// truth *and* wire shape. Deserializing a wire payload back into it
/// silently left the four internal-only fields at their zero/`false`/`None`
/// defaults, so any code that parsed `ModelInfo` from JSON and then read
/// `max_output_tokens` (a cost-cap input) got a wrong-but-plausible value.
/// Splitting the registry struct ([`ModelRegistration`]) from the wire
/// struct ([`ModelInfo`]) makes that lossy round-trip *structurally
/// impossible*: the wire type does not carry those fields at all, so there
/// is nothing to silently zero-fill and no internal field to misread.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelRegistration {
    /// Canonical gateway-facing id, e.g. `"openai/gpt-4o"`. Emitted on the wire.
    pub id: String,
    /// Provider name, e.g. `"openai"`. Emitted on the wire.
    pub provider: String,
    /// Provider-side model identifier, e.g. `"gpt-4o"`. **Internal-only** —
    /// used to address the upstream provider; never emitted on the wire.
    pub model_id: String,
    /// Human-friendly name. Emitted on the wire.
    pub display_name: String,
    /// Input price per million tokens (USDC). Emitted on the wire (nested
    /// under `pricing`).
    pub input_cost_per_million: f64,
    /// Output price per million tokens (USDC). Emitted on the wire (nested
    /// under `pricing`).
    pub output_cost_per_million: f64,
    /// Context window in tokens. Emitted on the wire.
    pub context_window: u32,
    /// Emitted on the wire (nested under `capabilities`).
    pub supports_streaming: bool,
    /// Emitted on the wire (nested under `capabilities`).
    pub supports_tools: bool,
    /// Emitted on the wire (nested under `capabilities`).
    pub supports_vision: bool,
    /// Emitted on the wire (nested under `capabilities`).
    pub reasoning: bool,
    /// **Internal-only** — never emitted on the wire.
    pub supports_structured_output: bool,
    /// **Internal-only** — never emitted on the wire.
    pub supports_batch: bool,
    /// Priceable completion-token ceiling. **Internal-only** — consumed by
    /// gateway cost-cap enforcement (`crates/gateway/src/routes/chat/cost.rs`);
    /// never emitted on the wire.
    pub max_output_tokens: Option<u32>,
}

/// Wire representation of a model, as emitted by `GET /v1/models` and parsed
/// by the sibling SDKs (Python, Go, TypeScript, Rust).
///
/// The on-the-wire JSON is nested (`pricing.input_per_million`,
/// `capabilities.streaming`); this struct keeps **flat** Rust fields so SDK
/// consumers read `info.input_cost_per_million` directly, with the nesting
/// handled by the hand-written [`Serialize`]/[`Deserialize`] impls below via
/// the private [`ModelInfoWire`]. Pre-0.3.0 versions derived Serde directly
/// on flat top-level fields that did not match the gateway, so SDK consumers
/// silently parsed every response with all-zero pricing and all-false
/// capabilities (#229).
///
/// This type carries **only** wire-visible fields. The gateway-internal
/// fields (`model_id`, `supports_structured_output`, `supports_batch`,
/// `max_output_tokens`) live on [`ModelRegistration`] and are intentionally
/// absent here — there is no lossy round-trip to misread.
///
/// # Wire shape is hand-written on purpose
///
/// The nested wire shape is produced by the manual `Serialize`/`Deserialize`
/// impls, **not** by `#[derive]`. Re-introducing a derive on this struct
/// would flip the wire back to flat top-level fields — the exact #229 drift.
/// Because the manual impl already occupies the trait slot, adding a derived
/// `Serialize` is a hard `E0119` "conflicting implementations" compile error:
///
/// ```compile_fail
/// use serde::Serialize;
///
/// // Mirrors `ModelInfo`: a hand-written `Serialize` impl. Adding a derived
/// // one on the same type is rejected at compile time — which is what stops
/// // an accidental re-derive from silently changing the wire shape.
/// #[derive(Serialize)]
/// struct ModelInfo {
///     id: String,
/// }
///
/// impl Serialize for ModelInfo {
///     fn serialize<S: serde::Serializer>(&self, _s: S) -> Result<S::Ok, S::Error> {
///         unimplemented!()
///     }
/// }
/// ```
///
/// (The orphan rule prevents a doctest from re-impl'ing `Serialize` on the
/// real `ModelInfo`, so the guard above mirrors the structure. The behavioral
/// counterpart — "wire JSON must stay nested" — is pinned by the runtime
/// `serializes_nested_wire_shape` test in this module, which catches the
/// other regression path: deleting the manual impl and re-deriving flat.)
#[derive(Debug, Clone, PartialEq)]
pub struct ModelInfo {
    pub id: String,
    pub provider: String,
    pub display_name: String,
    pub input_cost_per_million: f64,
    pub output_cost_per_million: f64,
    pub context_window: u32,
    pub supports_streaming: bool,
    pub supports_tools: bool,
    pub supports_vision: bool,
    pub reasoning: bool,
}

impl From<&ModelRegistration> for ModelInfo {
    /// Project a registry entry onto its wire shape, dropping the four
    /// internal-only fields the gateway never emits.
    fn from(r: &ModelRegistration) -> Self {
        Self {
            id: r.id.clone(),
            provider: r.provider.clone(),
            display_name: r.display_name.clone(),
            input_cost_per_million: r.input_cost_per_million,
            output_cost_per_million: r.output_cost_per_million,
            context_window: r.context_window,
            supports_streaming: r.supports_streaming,
            supports_tools: r.supports_tools,
            supports_vision: r.supports_vision,
            reasoning: r.reasoning,
        }
    }
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

/// Deserialize from the gateway's nested wire shape into a flat [`ModelInfo`].
///
/// Only wire-visible fields are parsed. There are no internal-only fields on
/// [`ModelInfo`] to silently zero-fill — that hazard moved to
/// [`ModelRegistration`], which is never deserialized from the wire. If you
/// need a registry-grade record, parse [`ModelInfo`] for wire truth and merge
/// with the registry (`crates/router/src/models.rs`) for the internal fields.
impl<'de> Deserialize<'de> for ModelInfo {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let w = ModelInfoWire::deserialize(deserializer)?;
        Ok(Self {
            id: w.id,
            provider: w.provider,
            display_name: w.display_name,
            input_cost_per_million: w.pricing.input_per_million,
            output_cost_per_million: w.pricing.output_per_million,
            context_window: w.context_window,
            supports_streaming: w.capabilities.streaming,
            supports_tools: w.capabilities.tools,
            supports_vision: w.capabilities.vision,
            reasoning: w.capabilities.reasoning,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn registration() -> ModelRegistration {
        ModelRegistration {
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

    fn fixture() -> ModelInfo {
        ModelInfo::from(&registration())
    }

    #[test]
    fn registration_to_wire_drops_internal_only_fields() {
        // The conversion is the single chokepoint where internal-only fields
        // are shed. Everything wire-visible survives; nothing else exists on
        // the wire type to leak.
        let reg = registration();
        let info = ModelInfo::from(&reg);

        assert_eq!(info.id, reg.id);
        assert_eq!(info.provider, reg.provider);
        assert_eq!(info.display_name, reg.display_name);
        assert_eq!(info.input_cost_per_million, reg.input_cost_per_million);
        assert_eq!(info.output_cost_per_million, reg.output_cost_per_million);
        assert_eq!(info.context_window, reg.context_window);
        assert_eq!(info.supports_streaming, reg.supports_streaming);
        assert_eq!(info.supports_tools, reg.supports_tools);
        assert_eq!(info.supports_vision, reg.supports_vision);
        assert_eq!(info.reasoning, reg.reasoning);
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

        // Internal-only fields are not even present on the wire type, so they
        // categorically cannot appear in the JSON.
        assert!(v.get("model_id").is_none());
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
        // Exact shape from crates/gateway/src/routes/models.rs.
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
    fn round_trip_through_wire_is_identity() {
        // With internal-only fields off the wire type, a wire round trip is a
        // true identity — there is nothing left to lose. This is the property
        // the type split buys: the old `ModelInfo` reset four fields on every
        // round trip; the new wire type has none to reset.
        let original = fixture();
        let json = serde_json::to_string(&original).unwrap();
        let parsed: ModelInfo = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed, original);
    }
}
