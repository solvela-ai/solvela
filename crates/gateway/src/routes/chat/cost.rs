//! USDC cost computation and token estimation.
//!
//! All financial calculations use integer arithmetic to avoid f64 precision
//! loss on monetary amounts. USDC has 6 decimal places (atomic units).

use tracing::warn;

use solvela_protocol::{ChatRequest, ModelInfo};

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
pub(crate) fn compute_actual_atomic_cost(
    prompt_tokens: u32,
    completion_tokens: u32,
    model_info: &ModelInfo,
) -> u64 {
    // Convert cost-per-million-tokens from USDC (f64) to atomic micro-USDC (u64)
    // by multiplying by 1_000_000. This is the only f64->int conversion.
    let input_cost_atomic_per_million = (model_info.input_cost_per_million * 1_000_000.0) as u128;
    let output_cost_atomic_per_million = (model_info.output_cost_per_million * 1_000_000.0) as u128;

    // tokens * atomic_cost_per_million / 1_000_000 = atomic cost
    let input_atomic = (prompt_tokens as u128) * input_cost_atomic_per_million / 1_000_000;
    let output_atomic = (completion_tokens as u128) * output_cost_atomic_per_million / 1_000_000;
    let provider_atomic = input_atomic + output_atomic;

    // 5% platform fee: total = provider * 105 / 100
    let total_atomic = provider_atomic * 105 / 100;

    total_atomic as u64
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
        assert_eq!(atomic, 7875);
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

        assert_eq!(compute_actual_atomic_cost(0, 0, &model_info), 0);
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
            atomic, 1_050_000,
            "expected exactly 1.05x provider cost (single 5% fee); got {atomic}. \
             1_102_500 here means the fee is being applied twice."
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
        assert_eq!(atomic, 1_050_000);
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

    #[test]
    fn test_estimated_atomic_cost_rejects_nan() {
        let reg = registry_with_cost(f64::NAN);
        let result = estimated_atomic_cost(&reg, "test-model", &simple_req());
        // NaN cost_per_million propagates to a NaN total; estimated_atomic_cost
        // must reject it rather than silently cast NaN→0.
        match result {
            Err(e) => assert!(
                e.contains("non-finite") || e.contains("NaN") || e.contains("failed"),
                "error should mention non-finite or NaN, got: {e}"
            ),
            Ok(v) => panic!("NaN cost must not produce Ok({v})"),
        }
    }

    #[test]
    fn test_estimated_atomic_cost_rejects_positive_infinity() {
        let reg = registry_with_cost(f64::INFINITY);
        let result = estimated_atomic_cost(&reg, "test-model", &simple_req());
        match result {
            Err(e) => assert!(
                e.contains("non-finite") || e.contains("inf") || e.contains("failed"),
                "error should mention non-finite or infinity, got: {e}"
            ),
            Ok(v) => panic!("INFINITY cost must not produce Ok({v})"),
        }
    }

    #[test]
    fn test_estimated_atomic_cost_rejects_negative_infinity() {
        let reg = registry_with_cost(f64::NEG_INFINITY);
        let result = estimated_atomic_cost(&reg, "test-model", &simple_req());
        match result {
            Err(_) => {} // expected — non-finite or negative check
            Ok(v) => panic!("-INFINITY cost must not produce Ok({v})"),
        }
    }

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
}
