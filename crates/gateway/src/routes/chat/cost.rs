//! USDC cost computation and token estimation.
//!
//! All financial calculations use integer arithmetic to avoid f64 precision
//! loss on monetary amounts. USDC has 6 decimal places (atomic units).

use tracing::warn;

use solvela_protocol::{ChatRequest, ModelInfo, Usage};

/// Hard ceiling on `completion_tokens` when neither the request nor the model
/// registry declares a tighter cap. Prevents a misbehaving / compromised
/// provider from inflating `usage.completion_tokens` to something the
/// gateway cannot have priced for.
const DEFAULT_COMPLETION_TOKENS_CAP: u32 = 8192;

/// Cap a provider-reported `Usage` to limits the gateway has actually priced
/// for. Provider responses are trusted today (the gateway speaks
/// OpenAI-compat to the provider and parses the JSON back), but a compromised
/// or misbehaving provider could inflate `completion_tokens` to over-bill the
/// agent, or deflate `prompt_tokens` to under-bill the gateway. Both flow
/// directly into `compute_actual_atomic_cost` and `state.usage.log_spend`,
/// so both need bounding before they reach billing.
///
/// Caps:
/// - `completion_tokens` ≤ min(`req.max_tokens`, `model_info.max_output_tokens`,
///   `DEFAULT_COMPLETION_TOKENS_CAP`). Whichever the gateway treated as the
///   priceable ceiling at quote time is the right cap; we take the tightest.
/// - `prompt_tokens` ≤ `model_info.context_window`. The provider cannot have
///   processed more than the context window without errors, so anything
///   above is a sign of a corrupt response.
/// - `total_tokens` is recomputed from the capped components — provider-supplied
///   totals are not trusted independently.
pub(crate) fn cap_usage_to_request_limits(
    usage: &Usage,
    req: &ChatRequest,
    model_info: &ModelInfo,
) -> Usage {
    let completion_cap = [
        req.max_tokens,
        model_info.max_output_tokens,
        Some(DEFAULT_COMPLETION_TOKENS_CAP),
    ]
    .into_iter()
    .flatten()
    .min()
    .unwrap_or(DEFAULT_COMPLETION_TOKENS_CAP);

    let prompt = usage.prompt_tokens.min(model_info.context_window);
    let completion = usage.completion_tokens.min(completion_cap);

    if completion < usage.completion_tokens {
        warn!(
            model = %model_info.model_id,
            reported = usage.completion_tokens,
            cap = completion,
            "provider reported completion_tokens above gateway cap — clamping for billing"
        );
    }
    if prompt < usage.prompt_tokens {
        warn!(
            model = %model_info.model_id,
            reported = usage.prompt_tokens,
            cap = prompt,
            "provider reported prompt_tokens above context window — clamping for billing"
        );
    }

    let total = prompt.saturating_add(completion);
    Usage {
        prompt_tokens: prompt,
        completion_tokens: completion,
        total_tokens: total,
    }
}

/// Rough token estimate: ~4 chars per token.
pub(crate) fn estimate_input_tokens(req: &ChatRequest) -> u32 {
    let chars: usize = req.messages.iter().map(|m| m.content.len()).sum();
    (chars / 4).max(1) as u32
}

/// Compute the actual cost in atomic USDC units from token usage.
///
/// Uses integer arithmetic to avoid f64 precision loss on financial amounts.
/// Cost per million tokens is converted to micro-USDC (atomic units) early,
/// then all math stays in u128 to prevent overflow on large token counts.
///
/// Returns `None` when the model registry's per-million pricing is non-finite
/// (NaN/Inf) or negative, or when the final atomic total would overflow `u64`.
/// Rust's `f64 as u128` cast is saturating: `NaN` → 0 and `INFINITY` →
/// `u128::MAX`. Without this guard, a corrupt `models.toml` entry with `NaN`
/// pricing would silently quote a $0 cost (escrow claim then no-ops on the
/// `claim_amount == 0` guard), and an `INFINITY` entry would wrap on the
/// `* 105 / 100` step. Both are silent revenue gaps.
pub(crate) fn compute_actual_atomic_cost(
    prompt_tokens: u32,
    completion_tokens: u32,
    model_info: &ModelInfo,
) -> Option<u64> {
    let input = model_info.input_cost_per_million;
    let output = model_info.output_cost_per_million;
    if !input.is_finite() || !output.is_finite() || input < 0.0 || output < 0.0 {
        warn!(
            model = %model_info.model_id,
            input_cost = input,
            output_cost = output,
            "model pricing is non-finite or negative — refusing to compute cost"
        );
        return None;
    }

    // Convert cost-per-million-tokens from USDC (f64) to atomic micro-USDC (u128)
    // by multiplying by 1_000_000. This is the only f64->int conversion; the
    // `is_finite` guard above ensures the cast is well-defined.
    let input_cost_atomic_per_million = (input * 1_000_000.0) as u128;
    let output_cost_atomic_per_million = (output * 1_000_000.0) as u128;

    // tokens * atomic_cost_per_million / 1_000_000 = atomic cost
    let input_atomic = (prompt_tokens as u128) * input_cost_atomic_per_million / 1_000_000;
    let output_atomic = (completion_tokens as u128) * output_cost_atomic_per_million / 1_000_000;
    let provider_atomic = input_atomic + output_atomic;

    // 5% platform fee: total = provider * 105 / 100
    let total_atomic = provider_atomic * 105 / 100;

    // Final overflow guard: u128 → u64. With sane pricing this never trips,
    // but guarding here means a pathological combination of huge tokens ×
    // huge pricing surfaces as None instead of silent wrap.
    u64::try_from(total_atomic).ok()
}

/// Upper bound for the `total` USDC cost that `estimated_atomic_cost` will accept.
///
/// Multiplying by 1_000_000 (to convert USDC → atomic units) must not overflow
/// u64 (max ~1.84 × 10¹⁹).  We cap at u64::MAX / 1_000_000 ≈ $18.4 trillion,
/// which is far beyond any realistic LLM request cost.
///
/// TODO(GHSA-86cr-h3rx-vj6j): remove this guard once cost.rs and usage.rs are
/// fully migrated to integer atomic-USDC arithmetic so f64 is never used for
/// financial values.
const ESTIMATED_COST_MAX_USDC: f64 = (u64::MAX / 1_000_000) as f64;

/// Estimate cost in atomic USDC units using the model registry's cost breakdown.
///
/// Used as a fallback when actual token usage is unavailable (e.g., streaming).
///
/// Returns `Err` (not `Ok(0)`) when the estimate cannot be computed so that
/// callers fail-closed: treating a failed estimate as zero cost would bypass
/// budget enforcement and under-claim from escrow.
///
/// Range checks applied to the parsed f64 before casting to u64:
/// - Non-finite (NaN, ±∞): indicates a corrupt model registry entry.
/// - Negative: cost cannot be negative; a negative cast wraps to a huge u64.
/// - Overflow (> u64::MAX / 1_000_000): the ×1e6 multiplier would exceed u64.
pub(crate) fn estimated_atomic_cost(
    registry: &solvela_router::models::ModelRegistry,
    model: &str,
    req: &ChatRequest,
) -> Result<u64, String> {
    let f = registry
        .estimate_cost(
            model,
            estimate_input_tokens(req),
            req.max_tokens.unwrap_or(1000),
        )
        .and_then(|c| {
            c.total
                .parse::<f64>()
                .map_err(|e| solvela_router::models::ModelRegistryError::ParseError(e.to_string()))
        })
        .map_err(|e| {
            warn!(error = %e, model, "failed to estimate atomic cost for escrow claim");
            e.to_string()
        })?;

    if !f.is_finite() {
        return Err(format!(
            "cost estimate for model '{model}' is non-finite ({f}); \
             refusing to cast NaN/∞ to u64"
        ));
    }
    if f < 0.0 {
        return Err(format!(
            "cost estimate for model '{model}' is negative ({f}); \
             negative USDC amounts are invalid"
        ));
    }
    if f > ESTIMATED_COST_MAX_USDC {
        return Err(format!(
            "cost estimate for model '{model}' ({f} USDC) exceeds u64 range \
             after ×1_000_000 conversion; possible overflow in model pricing config"
        ));
    }

    Ok((f * 1_000_000.0) as u64)
}

/// Convert a USDC decimal string to atomic units (6 decimals).
///
/// Uses integer arithmetic to avoid f64 precision loss on financial amounts.
/// Splits on the decimal point and pads/truncates the fractional part to 6 digits.
///
/// Returns an error if the input is empty, contains non-numeric characters, or
/// would overflow `u64` after multiplication by 1_000_000. This prevents silent
/// fallback to 0 (rejects malformed input) and silent wraparound to a tiny
/// positive number (rejects whole-USDC values above ~$18.4 trillion).
pub(crate) fn usdc_atomic_amount_checked(decimal_str: &str) -> Result<String, String> {
    let s = decimal_str.trim();
    if s.is_empty() {
        return Err("empty USDC amount string".to_string());
    }

    let (integer_part, fractional_part) = if let Some(dot) = s.find('.') {
        (&s[..dot], &s[dot + 1..])
    } else {
        (s, "")
    };

    let integer: u64 = integer_part
        .parse()
        .map_err(|e| format!("invalid USDC integer part '{}': {}", integer_part, e))?;

    // Pad or truncate fractional part to exactly 6 digits
    let frac_padded = format!("{:0<6}", fractional_part);
    let frac_6 = &frac_padded[..6.min(frac_padded.len())];
    let fractional: u64 = frac_6
        .parse()
        .map_err(|e| format!("invalid USDC fractional part '{}': {}", frac_6, e))?;

    // Use checked arithmetic: `integer * 1_000_000` wraps for any whole-USDC
    // value above `u64::MAX / 1e6` ≈ 1.84e13 USDC, silently turning very large
    // amounts into very small ones — fail-closed instead.
    let atomic = integer
        .checked_mul(1_000_000)
        .and_then(|scaled| scaled.checked_add(fractional))
        .ok_or_else(|| {
            format!(
                "USDC amount '{decimal_str}' overflows u64 atomic units \
                 (max ≈ {} USDC)",
                u64::MAX / 1_000_000
            )
        })?;
    Ok(atomic.to_string())
}

/// Unchecked convenience wrapper -- used only in tests to verify round-trip behavior.
#[cfg(test)]
fn usdc_atomic_amount(decimal_str: &str) -> String {
    usdc_atomic_amount_checked(decimal_str).unwrap_or_else(|_| "0".to_string())
}

/// Pricing for a semantic-cache hit, in atomic USDC units.
///
/// The normal path (cache miss / exact-match hit) is represented by the absence
/// of this value (`Option::None`) — the agent pays the full computed cost. When
/// present, `full_atomic` is what a miss would have cost (the **all-in** price:
/// provider cost + 5% platform fee; retained for receipts and metrics) and
/// `discounted_atomic` is what's actually billed.
///
/// The discount is a flat fraction of that all-in price, so the 5% fee scales
/// down with it — this mirrors how provider prompt caching prices a hit (a flat
/// fraction of the all-in rate, not "compute discount + full margin"). It is not
/// a fee loss: a cache hit pays the upstream provider nothing, so the entire
/// `discounted_atomic` is gateway revenue minus the sub-cent embedding cost.
///
/// The discount is realised on the **escrow scheme** by claiming only
/// `discounted_atomic`; the remainder refunds to the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SemanticDiscount {
    // Private so the only construction path is `new`, which guarantees the
    // invariant `discounted_atomic <= full_atomic`. A struct literal could
    // otherwise set `discounted > full` and over-claim from escrow.
    full_atomic: u64,
    discounted_atomic: u64,
}

impl SemanticDiscount {
    /// Build a semantic discount, computing the billed price from the full
    /// price and the percent-of-full to bill (e.g. `30` → bill 30%).
    pub(crate) fn new(full_atomic: u64, hit_price_percent: u8) -> Self {
        SemanticDiscount {
            full_atomic,
            discounted_atomic: apply_hit_price(full_atomic, hit_price_percent),
        }
    }

    /// Amount to actually charge / claim, in atomic USDC.
    pub(crate) fn billable_atomic(&self) -> u64 {
        self.discounted_atomic
    }

    /// The full (undiscounted) all-in price a miss would have cost, in atomic
    /// USDC. Used for the discount-ratio metric.
    pub(crate) fn full_atomic(&self) -> u64 {
        self.full_atomic
    }
}

/// Apply a semantic-hit price to a full atomic cost using integer arithmetic
/// (no f64 — financial math stays exact).
///
/// `hit_price_percent` is the percentage of the full price the agent still pays
/// on a hit (e.g. `30` = pay 30%, a 70% discount). Clamped to `0..=100`, so the
/// result is always `floor(full_atomic * pct / 100)` and never exceeds
/// `full_atomic`.
pub(crate) fn apply_hit_price(full_atomic: u64, hit_price_percent: u8) -> u64 {
    let pct = hit_price_percent.min(100) as u128;
    // u128 intermediate so `full_atomic * pct` cannot overflow u64; the result
    // is always ≤ full_atomic (pct ≤ 100), so the cast back is lossless.
    ((full_atomic as u128) * pct / 100) as u64
}

/// The USDC amount to record in the spend log / budget ledger for this request.
///
/// On a semantic-cache hit the agent is billed the **discounted** price, so the
/// ledger must reflect that — not the full computed cost. Logging the full cost
/// makes per-wallet budget counters consume at 100% while the escrow claim only
/// took the discount, denying agents service before their real spend warrants
/// it. `None` (normal path) records the full computed cost as before.
pub(crate) fn spend_cost_usdc(cost_outcome: Option<SemanticDiscount>, full_cost_usdc: f64) -> f64 {
    match cost_outcome {
        Some(discount) => discount.billable_atomic() as f64 / 1_000_000.0,
        None => full_cost_usdc,
    }
}

/// The full (undiscounted) all-in price to charge for a semantic-cache hit, in
/// atomic USDC, or `None` if no positive price can be derived.
///
/// `ChatResponse.usage` is optional and some providers omit it. We prefer the
/// cached response's actual token cost, fall back to the current request's
/// estimate, and **reject zero** so a usage-less hit can never settle free
/// (the C2 bug): a `None` here signals the caller to fall through to a normal
/// upstream call rather than serve a discounted-from-nothing response.
pub(crate) fn semantic_hit_full_atomic(
    usage: Option<&Usage>,
    registry: &solvela_router::models::ModelRegistry,
    model: &str,
    req: &ChatRequest,
    model_info: &ModelInfo,
) -> Option<u64> {
    usage
        .and_then(|u| compute_actual_atomic_cost(u.prompt_tokens, u.completion_tokens, model_info))
        .or_else(|| estimated_atomic_cost(registry, model, req).ok())
        .filter(|&c| c > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use solvela_protocol::{ChatMessage, ModelInfo, Role};

    fn user_msg(content: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: content.to_string(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    fn make_request(model: &str, messages: Vec<ChatMessage>) -> ChatRequest {
        ChatRequest {
            model: model.to_string(),
            messages,
            max_tokens: None,
            temperature: None,
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
        }
    }

    // =========================================================================
    // usdc_atomic_amount -- Financial calculation (100% coverage required)
    // =========================================================================

    #[test]
    fn test_usdc_atomic_basic_decimal() {
        assert_eq!(usdc_atomic_amount("1.50"), "1500000");
    }

    #[test]
    fn test_usdc_atomic_small_amount() {
        assert_eq!(usdc_atomic_amount("0.002625"), "2625");
    }

    #[test]
    fn test_usdc_atomic_whole_number() {
        assert_eq!(usdc_atomic_amount("5"), "5000000");
    }

    #[test]
    fn test_usdc_atomic_zero() {
        assert_eq!(usdc_atomic_amount("0"), "0");
        assert_eq!(usdc_atomic_amount("0.0"), "0");
        assert_eq!(usdc_atomic_amount("0.000000"), "0");
    }

    #[test]
    fn test_usdc_atomic_max_precision() {
        assert_eq!(usdc_atomic_amount("0.000001"), "1");
    }

    #[test]
    fn test_usdc_atomic_truncates_beyond_6_decimals() {
        assert_eq!(usdc_atomic_amount("0.0000019"), "1");
    }

    #[test]
    fn test_usdc_atomic_large_amount() {
        assert_eq!(usdc_atomic_amount("1000.000000"), "1000000000");
    }

    #[test]
    fn test_usdc_atomic_whitespace_trimmed() {
        assert_eq!(usdc_atomic_amount("  1.50  "), "1500000");
    }

    #[test]
    fn test_usdc_atomic_exact_six_decimals() {
        assert_eq!(usdc_atomic_amount("0.123456"), "123456");
    }

    #[test]
    fn test_usdc_atomic_fewer_decimals_pads() {
        assert_eq!(usdc_atomic_amount("0.5"), "500000");
    }

    #[test]
    fn test_usdc_atomic_empty_string() {
        assert_eq!(usdc_atomic_amount(""), "0");
    }

    #[test]
    fn test_usdc_atomic_typical_llm_costs() {
        assert_eq!(usdc_atomic_amount("0.002625"), "2625");
        assert_eq!(usdc_atomic_amount("0.000042"), "42");
        assert_eq!(usdc_atomic_amount("0.015750"), "15750");
    }

    // =========================================================================
    // estimate_input_tokens
    // =========================================================================

    #[test]
    fn test_estimate_input_tokens_simple() {
        let req = make_request("m", vec![user_msg("Hello")]);
        assert_eq!(estimate_input_tokens(&req), 1);
    }

    #[test]
    fn test_estimate_input_tokens_longer_message() {
        let msg = "a".repeat(100);
        let req = make_request("m", vec![user_msg(&msg)]);
        assert_eq!(estimate_input_tokens(&req), 25);
    }

    #[test]
    fn test_estimate_input_tokens_multiple_messages() {
        let req = make_request(
            "m",
            vec![user_msg(&"b".repeat(50)), user_msg(&"c".repeat(50))],
        );
        assert_eq!(estimate_input_tokens(&req), 25);
    }

    #[test]
    fn test_estimate_input_tokens_empty_message_returns_at_least_one() {
        let req = make_request("m", vec![user_msg("")]);
        assert_eq!(estimate_input_tokens(&req), 1, "minimum should be 1 token");
    }

    // =========================================================================
    // compute_actual_atomic_cost -- Financial calculation (100% coverage)
    // =========================================================================

    #[test]
    fn test_compute_actual_atomic_cost_basic() {
        let model_info = ModelInfo {
            id: "openai/gpt-4o".to_string(),
            provider: "openai".to_string(),
            model_id: "gpt-4o".to_string(),
            display_name: "GPT-4o".to_string(),
            input_cost_per_million: 2.50,
            output_cost_per_million: 10.00,
            context_window: 128_000,
            supports_streaming: true,
            supports_tools: true,
            supports_vision: true,
            reasoning: false,
            supports_structured_output: false,
            supports_batch: false,
            max_output_tokens: None,
        };

        let atomic = compute_actual_atomic_cost(1000, 500, &model_info);
        assert_eq!(atomic, Some(7875));
    }

    #[test]
    fn test_compute_actual_atomic_cost_zero_tokens() {
        let model_info = ModelInfo {
            id: "test/model".to_string(),
            provider: "test".to_string(),
            model_id: "model".to_string(),
            display_name: "Test".to_string(),
            input_cost_per_million: 2.50,
            output_cost_per_million: 10.00,
            context_window: 128_000,
            supports_streaming: true,
            supports_tools: false,
            supports_vision: false,
            reasoning: false,
            supports_structured_output: false,
            supports_batch: false,
            max_output_tokens: None,
        };

        assert_eq!(compute_actual_atomic_cost(0, 0, &model_info), Some(0));
    }

    #[test]
    fn test_compute_actual_atomic_cost_applies_single_fee_via_registry() {
        // Regression: prevents double-application of the 5% platform fee.
        // Exercises the production wiring TOML → ModelRegistry → compute_actual_atomic_cost.
        // A hand-crafted ModelInfo cannot catch a registry-side bake-in; this test must.
        let toml = r#"
[models.test-model]
provider = "test"
model_id = "test-model"
display_name = "Test"
input_cost_per_million = 1.00
output_cost_per_million = 1.00
context_window = 4096
supports_streaming = false
supports_tools = false
supports_vision = false
"#;
        let registry = solvela_router::models::ModelRegistry::from_toml(toml)
            .expect("valid test registry TOML");
        let model_info = registry
            .get("test/test-model")
            .expect("model registered under canonical id");

        // 1M input tokens @ $1.00/M = $1.00 provider cost = 1_000_000 atomic.
        // Single 5% fee → 1_050_000. A double-applied fee would yield 1_102_500.
        let atomic = compute_actual_atomic_cost(1_000_000, 0, model_info);
        assert_eq!(
            atomic,
            Some(1_050_000),
            "expected exactly 1.05x provider cost (single 5% fee); got {atomic:?}. \
             Some(1_102_500) here means the fee is being applied twice."
        );
    }

    #[test]
    fn test_compute_actual_atomic_cost_includes_5_percent_fee() {
        let model_info = ModelInfo {
            id: "test/model".to_string(),
            provider: "test".to_string(),
            model_id: "model".to_string(),
            display_name: "Test".to_string(),
            input_cost_per_million: 1_000_000.0,
            output_cost_per_million: 0.0,
            context_window: 128_000,
            supports_streaming: true,
            supports_tools: false,
            supports_vision: false,
            reasoning: false,
            supports_structured_output: false,
            supports_batch: false,
            max_output_tokens: None,
        };

        let atomic = compute_actual_atomic_cost(1, 0, &model_info);
        assert_eq!(atomic, Some(1_050_000));
    }

    // =========================================================================
    // cap_usage_to_request_limits — provider-trust bounds (H7)
    // =========================================================================

    fn test_model_info() -> ModelInfo {
        ModelInfo {
            id: "test/model".to_string(),
            provider: "test".to_string(),
            model_id: "test-model".to_string(),
            display_name: "Test".to_string(),
            input_cost_per_million: 1.0,
            output_cost_per_million: 1.0,
            context_window: 4096,
            supports_streaming: false,
            supports_tools: false,
            supports_vision: false,
            reasoning: false,
            supports_structured_output: false,
            supports_batch: false,
            max_output_tokens: Some(2048),
        }
    }

    fn req_with_max_tokens(max: Option<u32>) -> ChatRequest {
        use solvela_protocol::ChatRequest;
        ChatRequest {
            model: "test/model".to_string(),
            messages: vec![],
            max_tokens: max,
            temperature: None,
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
        }
    }

    #[test]
    fn cap_usage_clamps_completion_to_req_max_tokens() {
        let model = test_model_info();
        let req = req_with_max_tokens(Some(500));
        // Provider claims 9999 completion tokens. Gateway only priced for ≤500.
        let inflated = solvela_protocol::Usage {
            prompt_tokens: 100,
            completion_tokens: 9999,
            total_tokens: 10099,
        };
        let capped = cap_usage_to_request_limits(&inflated, &req, &model);
        assert_eq!(capped.completion_tokens, 500);
        assert_eq!(capped.prompt_tokens, 100);
        assert_eq!(capped.total_tokens, 600);
    }

    #[test]
    fn cap_usage_clamps_prompt_to_context_window() {
        let model = test_model_info(); // context_window = 4096
        let req = req_with_max_tokens(Some(500));
        // Provider claims a wildly inflated prompt token count.
        let inflated = solvela_protocol::Usage {
            prompt_tokens: 1_000_000,
            completion_tokens: 100,
            total_tokens: 1_000_100,
        };
        let capped = cap_usage_to_request_limits(&inflated, &req, &model);
        assert_eq!(capped.prompt_tokens, 4096);
        assert_eq!(capped.completion_tokens, 100);
        assert_eq!(capped.total_tokens, 4196);
    }

    #[test]
    fn cap_usage_falls_back_to_model_max_output_when_request_uncapped() {
        let model = test_model_info(); // max_output_tokens = Some(2048)
        let req = req_with_max_tokens(None); // no req cap
        let inflated = solvela_protocol::Usage {
            prompt_tokens: 10,
            completion_tokens: 9999,
            total_tokens: 10009,
        };
        let capped = cap_usage_to_request_limits(&inflated, &req, &model);
        // Falls back to model.max_output_tokens (2048), which is tighter
        // than DEFAULT_COMPLETION_TOKENS_CAP (8192).
        assert_eq!(capped.completion_tokens, 2048);
    }

    #[test]
    fn cap_usage_passes_through_when_within_limits() {
        let model = test_model_info();
        let req = req_with_max_tokens(Some(1000));
        let normal = solvela_protocol::Usage {
            prompt_tokens: 100,
            completion_tokens: 500,
            total_tokens: 600,
        };
        let capped = cap_usage_to_request_limits(&normal, &req, &model);
        assert_eq!(capped.prompt_tokens, 100);
        assert_eq!(capped.completion_tokens, 500);
        assert_eq!(capped.total_tokens, 600);
    }

    /// Regression for the H3 finding: NaN/Inf/negative pricing must return
    /// None, not silently saturate to 0 or u64::MAX. A corrupt models.toml
    /// entry must not be a silent revenue gap.
    #[test]
    fn test_compute_actual_atomic_cost_rejects_non_finite_pricing() {
        let mut model_info = ModelInfo {
            id: "test/model".to_string(),
            provider: "test".to_string(),
            model_id: "model".to_string(),
            display_name: "Test".to_string(),
            input_cost_per_million: f64::NAN,
            output_cost_per_million: 1.00,
            context_window: 128_000,
            supports_streaming: false,
            supports_tools: false,
            supports_vision: false,
            reasoning: false,
            supports_structured_output: false,
            supports_batch: false,
            max_output_tokens: None,
        };
        // NaN input cost
        assert_eq!(compute_actual_atomic_cost(1000, 500, &model_info), None);
        // Infinity input cost
        model_info.input_cost_per_million = f64::INFINITY;
        assert_eq!(compute_actual_atomic_cost(1000, 500, &model_info), None);
        // Negative output cost
        model_info.input_cost_per_million = 1.00;
        model_info.output_cost_per_million = -0.001;
        assert_eq!(compute_actual_atomic_cost(1000, 500, &model_info), None);
        // NaN output cost
        model_info.output_cost_per_million = f64::NAN;
        assert_eq!(compute_actual_atomic_cost(1000, 500, &model_info), None);
    }

    // =========================================================================
    // Security: usdc_atomic_amount_checked rejects malformed input
    // =========================================================================

    #[test]
    fn test_usdc_atomic_checked_valid_amounts() {
        assert_eq!(usdc_atomic_amount_checked("1.50").unwrap(), "1500000");
        assert_eq!(usdc_atomic_amount_checked("0.002625").unwrap(), "2625");
        assert_eq!(usdc_atomic_amount_checked("5").unwrap(), "5000000");
        assert_eq!(usdc_atomic_amount_checked("0").unwrap(), "0");
    }

    #[test]
    fn test_usdc_atomic_checked_rejects_empty() {
        assert!(
            usdc_atomic_amount_checked("").is_err(),
            "empty string must be rejected"
        );
    }

    #[test]
    fn test_usdc_atomic_checked_rejects_non_numeric() {
        assert!(
            usdc_atomic_amount_checked("abc").is_err(),
            "non-numeric string must be rejected"
        );
        assert!(
            usdc_atomic_amount_checked("1.2.3").is_err(),
            "double-dot string must be rejected"
        );
        assert!(
            usdc_atomic_amount_checked("-1.50").is_err(),
            "negative amounts must be rejected"
        );
    }

    #[test]
    fn test_usdc_atomic_checked_rejects_negative() {
        assert!(usdc_atomic_amount_checked("-5").is_err());
    }

    #[test]
    fn test_usdc_atomic_unchecked_defaults_to_zero_on_malformed() {
        assert_eq!(usdc_atomic_amount(""), "0");
        assert_eq!(usdc_atomic_amount("not-a-number"), "0");
    }

    #[test]
    fn test_usdc_atomic_checked_rejects_overflow_in_integer_part() {
        // u64::MAX / 1_000_000 ≈ 1.844e13. Anything above must reject so the
        // ×1_000_000 multiplication cannot wrap to a small positive number.
        let just_over_cap = (u64::MAX / 1_000_000) + 1;
        let amount = format!("{just_over_cap}");
        let result = usdc_atomic_amount_checked(&amount);
        assert!(
            result.is_err(),
            "amount above u64/1e6 must overflow-reject, got: {result:?}"
        );
        assert!(
            result.unwrap_err().contains("overflow"),
            "error must mention overflow"
        );
    }

    #[test]
    fn test_usdc_atomic_checked_rejects_u64_max() {
        let amount = format!("{}", u64::MAX);
        let result = usdc_atomic_amount_checked(&amount);
        assert!(
            result.is_err(),
            "u64::MAX integer part must overflow-reject, got: {result:?}"
        );
    }

    #[test]
    fn test_usdc_atomic_checked_accepts_at_overflow_boundary() {
        // The largest integer that does NOT overflow when ×1_000_000 must
        // still succeed — boundary check ensures we didn't off-by-one the cap.
        let max_safe_integer = u64::MAX / 1_000_000;
        let amount = format!("{max_safe_integer}");
        let result = usdc_atomic_amount_checked(&amount);
        assert!(
            result.is_ok(),
            "amount at u64/1e6 boundary must still succeed, got: {result:?}"
        );
    }

    #[test]
    fn test_usdc_atomic_checked_rejects_overflow_via_fractional_add() {
        // Construct a value where `integer * 1_000_000` is exactly u64::MAX
        // minus a small remainder, then add a fractional part that pushes it
        // past u64::MAX. Using max_safe_integer with all-9s fractional triggers
        // the `checked_add` failure path specifically.
        let max_safe_integer = u64::MAX / 1_000_000;
        let amount = format!("{max_safe_integer}.999999");
        let result = usdc_atomic_amount_checked(&amount);
        assert!(
            result.is_err(),
            "fractional addition that overflows u64 must reject, got: {result:?}"
        );
    }

    // =========================================================================
    // estimated_atomic_cost — GHSA-86cr-h3rx-vj6j input-validation guards
    // =========================================================================

    /// Format an `f64` as a TOML float literal.
    ///
    /// TOML 1.0 spec accepts only lowercase `nan`, `inf`, `+inf`, `-inf` for
    /// special floats; Rust's `Display` impl prints `NaN` and `inf`. We map
    /// non-finite values to their TOML-compatible spellings so the registry
    /// loader can round-trip `f64::NAN` / `f64::INFINITY` for these tests.
    fn format_f64_for_toml(v: f64) -> String {
        if v.is_nan() {
            "nan".to_string()
        } else if v.is_infinite() {
            if v.is_sign_positive() {
                "inf".to_string()
            } else {
                "-inf".to_string()
            }
        } else if v.abs() < 1e15 {
            // Finite values in moderate range: emit with one decimal so TOML
            // always parses as float (bare `1` would deserialize as integer).
            format!("{v:.1}")
        } else {
            // Scientific notation handles very large/small magnitudes (e.g. 1e18
            // for the overflow test) without precision loss.
            format!("{v:e}")
        }
    }

    /// Build a minimal `ModelRegistry` with a single model whose cost fields
    /// are set to `cost_per_million` USDC, for use in estimated_atomic_cost tests.
    fn registry_with_cost(cost_per_million: f64) -> solvela_router::models::ModelRegistry {
        let cost_str = format_f64_for_toml(cost_per_million);
        let toml = format!(
            r#"
[models.test-model]
provider = "test"
model_id = "test-model"
display_name = "Test"
input_cost_per_million = {cost_str}
output_cost_per_million = {cost_str}
context_window = 4096
supports_streaming = false
supports_tools = false
supports_vision = false
"#
        );
        solvela_router::models::ModelRegistry::from_toml(&toml).expect("valid test registry TOML")
        // safe: known-good template
    }

    fn simple_req() -> ChatRequest {
        make_request("test-model", vec![user_msg("hello")])
    }

    #[test]
    fn test_estimated_atomic_cost_valid_small_amount() {
        let reg = registry_with_cost(1.0); // $1 per million tokens
        let result = estimated_atomic_cost(&reg, "test-model", &simple_req());
        assert!(result.is_ok(), "valid cost should succeed, got: {result:?}");
        assert!(result.unwrap() > 0, "valid cost should be positive");
    }

    // R1 note: tests asserting NaN/+Inf/-Inf input through `registry_with_cost`
    // were removed because PR-R1 added a load-time validator in
    // `solvela_router::models::from_toml` that rejects non-finite and
    // negative pricing at the source — `registry_with_cost(f64::NAN)` now
    // panics at the `from_toml(...)` step, so those test inputs are
    // unreachable at this layer. The router crate carries equivalent
    // coverage in `from_toml_rejects_nan_negative_and_infinite_pricing`.
    //
    // The downstream `estimated_atomic_cost` guard is retained as
    // defense-in-depth: it still catches the case where finite inputs
    // produce a non-finite intermediate via arithmetic overflow (the
    // surviving overflow test below).

    #[test]
    fn test_estimated_atomic_cost_rejects_overflow() {
        // ESTIMATED_COST_MAX_USDC = u64::MAX / 1e6 ≈ 1.844e13 USDC. The estimate
        // is (input/1e6 + output/1e6) * cost_per_million * PLATFORM_FEE_MULTIPLIER,
        // which for simple_req() (~1 input token, 1000 output tokens) reduces to
        // roughly cost_per_million * 1.05e-3. Use 1e18 so the resulting total
        // (~1.05e15 USDC) exceeds the cap but stays finite.
        let huge_cost = 1.0e18_f64;
        let reg = registry_with_cost(huge_cost);
        let result = estimated_atomic_cost(&reg, "test-model", &simple_req());
        match result {
            Err(e) => assert!(
                e.contains("overflow") || e.contains("range") || e.contains("exceeds"),
                "error should mention overflow/range, got: {e}"
            ),
            Ok(v) => panic!("overflowing cost must not produce Ok({v})"),
        }
    }

    #[test]
    fn test_estimated_atomic_cost_unknown_model_returns_err() {
        let reg = registry_with_cost(1.0);
        let req = make_request("unknown-model", vec![user_msg("hello")]);
        let result = estimated_atomic_cost(&reg, "unknown-model", &req);
        assert!(
            result.is_err(),
            "unknown model must return Err, got: {result:?}"
        );
    }

    // =========================================================================
    // CostOutcome + apply_hit_price — semantic-cache discount (integer math)
    // =========================================================================

    #[test]
    fn apply_hit_price_charges_the_given_percent() {
        assert_eq!(apply_hit_price(1000, 30), 300);
        assert_eq!(apply_hit_price(2625, 30), 787); // floor(787.5)
    }

    #[test]
    fn apply_hit_price_full_percent_is_no_discount() {
        assert_eq!(apply_hit_price(1000, 100), 1000);
    }

    #[test]
    fn apply_hit_price_zero_percent_is_free() {
        assert_eq!(apply_hit_price(1000, 0), 0);
    }

    #[test]
    fn apply_hit_price_clamps_above_100_to_full() {
        // A misconfigured >100% must never bill more than the full price.
        assert_eq!(apply_hit_price(1000, 200), 1000);
    }

    #[test]
    fn apply_hit_price_floors_sub_unit_amounts() {
        // floor(1 * 0.30) == 0; never rounds up to over-bill.
        assert_eq!(apply_hit_price(1, 30), 0);
    }

    #[test]
    fn apply_hit_price_never_exceeds_full_and_never_overflows() {
        // Large value × percent must not overflow u64 (computed in u128).
        // u64::MAX * 50 / 100 == u64::MAX / 2; a u64-domain multiply would have
        // wrapped to a tiny value, so this exact-equality is the overflow proof.
        let discounted = apply_hit_price(u64::MAX, 50);
        assert_eq!(discounted, u64::MAX / 2);
    }

    #[test]
    fn semantic_discount_bills_discounted_and_keeps_full() {
        let outcome = SemanticDiscount::new(1000, 30);
        assert_eq!(outcome.billable_atomic(), 300);
        assert_eq!(outcome.full_atomic(), 1000);
    }

    #[test]
    fn spend_cost_uses_discounted_on_semantic_hit() {
        // full $5.00 (5_000_000 atomic), 30% → discounted $1.50.
        let outcome = Some(SemanticDiscount::new(5_000_000, 30));
        assert_eq!(spend_cost_usdc(outcome, 5.0), 1.5);
    }

    #[test]
    fn spend_cost_uses_full_when_no_discount() {
        assert_eq!(spend_cost_usdc(None, 5.0), 5.0);
    }

    // -------------------------------------------------------------------------
    // semantic_hit_full_atomic — C2 guard (a usage-less hit must never bill 0)
    // -------------------------------------------------------------------------

    fn priced_registry(cost_per_million: f64) -> solvela_router::models::ModelRegistry {
        registry_with_cost(cost_per_million)
    }

    #[test]
    fn semantic_full_price_uses_actual_usage_when_present() {
        let reg = priced_registry(2.0);
        let mi = reg.get("test/test-model").unwrap();
        let usage = Usage {
            prompt_tokens: 1000,
            completion_tokens: 500,
            total_tokens: 1500,
        };
        let req = make_request("test/test-model", vec![user_msg("hi")]);
        let got = semantic_hit_full_atomic(Some(&usage), &reg, "test/test-model", &req, mi);
        // 1000+500 tokens @ $2/M = $0.003 provider; ×1.05 fee = 3150 atomic.
        assert_eq!(got, Some(3150));
    }

    #[test]
    fn semantic_full_price_falls_back_to_estimate_when_usage_absent() {
        let reg = priced_registry(2.0);
        let mi = reg.get("test/test-model").unwrap();
        let req = make_request("test/test-model", vec![user_msg("hello there")]);
        let got = semantic_hit_full_atomic(None, &reg, "test/test-model", &req, mi);
        assert!(
            got.is_some() && got.unwrap() > 0,
            "usage-less hit on a priced model must estimate a positive cost, got {got:?}"
        );
    }

    #[test]
    fn semantic_full_price_returns_none_when_unpriceable_never_zero() {
        // Zero-priced model: actual usage prices to 0 AND estimate prices to 0.
        // The function must return None (→ caller falls through to upstream),
        // NOT Some(0) (which would settle the hit free — the C2 regression).
        let reg = priced_registry(0.0);
        let mi = reg.get("test/test-model").unwrap();
        let usage = Usage {
            prompt_tokens: 1000,
            completion_tokens: 500,
            total_tokens: 1500,
        };
        let req = make_request("test/test-model", vec![user_msg("hi")]);
        assert_eq!(
            semantic_hit_full_atomic(Some(&usage), &reg, "test/test-model", &req, mi),
            None,
            "a zero-priceable hit must be None (fall through), never Some(0)"
        );
    }
}
