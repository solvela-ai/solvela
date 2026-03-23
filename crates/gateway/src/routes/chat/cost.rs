//! USDC cost computation and token estimation.
//!
//! All financial calculations use integer arithmetic to avoid f64 precision
//! loss on monetary amounts. USDC has 6 decimal places (atomic units).

use tracing::warn;

use rustyclaw_protocol::{ChatRequest, ModelInfo};

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

/// Estimate cost in atomic USDC units using the model registry's cost breakdown.
///
/// Used as a fallback when actual token usage is unavailable (e.g., streaming).
/// Returns `None` when the estimate cannot be computed (model not found, parse
/// failure, etc.) so callers can distinguish "failed to compute" from "genuinely
/// zero cost".
pub(crate) fn estimated_atomic_cost(
    registry: &router::models::ModelRegistry,
    model: &str,
    req: &ChatRequest,
) -> Option<u64> {
    match registry
        .estimate_cost(
            model,
            estimate_input_tokens(req),
            req.max_tokens.unwrap_or(1000),
        )
        .and_then(|c| {
            c.total
                .parse::<f64>()
                .map_err(|e| router::models::ModelRegistryError::ParseError(e.to_string()))
        }) {
        Ok(f) => Some((f * 1_000_000.0) as u64),
        Err(e) => {
            warn!(error = %e, model, "failed to estimate atomic cost for escrow claim");
            None
        }
    }
}

/// Convert a USDC decimal string to atomic units (6 decimals).
///
/// Uses integer arithmetic to avoid f64 precision loss on financial amounts.
/// Splits on the decimal point and pads/truncates the fractional part to 6 digits.
///
/// Returns an error if the input is empty or contains non-numeric characters,
/// preventing silent fallback to 0 on malformed financial amounts.
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

    let atomic = integer * 1_000_000 + fractional;
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
    use rustyclaw_protocol::{ChatMessage, ModelInfo, Role};

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
}
