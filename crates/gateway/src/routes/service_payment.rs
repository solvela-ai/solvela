//! Shared flat-price + 5%-fee money math for paid marketplace tools.
//!
//! This is the **single source of truth** for converting a service's
//! `price_per_request_usdc` into the atomic-USDC breakdown that the 402 quote
//! and the payment-amount enforcement both read — for BOTH the external service
//! proxy (`routes/proxy.rs`) and gateway-hosted internal tools like web search
//! (`routes/search.rs`). Centralising it here means the fee can never drift
//! between two call sites that must quote and charge identically.
//!
//! Money-path invariants (see the `solvela-fintech` skill):
//! - All financial math is integer atomic USDC (6 decimals). The only `f64`
//!   here is the single sanctioned `price_usdc → atomic` conversion, guarded by
//!   [`validate_price_usdc`] (fail-closed on NaN/Inf/negative/overflow).
//! - The 5% platform fee is applied EXACTLY ONCE, as the canonical integer
//!   `provider * 105 / 100`.
//! - A corrupt price returns `Err`, never `Ok` with zeros — a misconfigured
//!   tool must reject the request, never serve for free.

/// Upper bound for `price_per_request_usdc` that the integer cost path accepts.
///
/// Multiplying by 1_000_000 (USDC → atomic units) must not overflow `u64`.
/// We cap at `u64::MAX / 1_000_000` ≈ $18.4 trillion per request, far beyond
/// any realistic tool price.
const PRICE_USDC_MAX: f64 = (u64::MAX / 1_000_000) as f64;

/// Validate a `price_per_request_usdc` value before it is cast to `u64`.
///
/// A naked `as u64` cast is fail-open for adversarial values:
/// - `NaN as u64` → 0 (tool served for free)
/// - `f64::INFINITY as u64` → `u64::MAX` (panics on later arithmetic)
/// - negative `as u64` → giant positive number (also serves for free)
///
/// A price of exactly `0.0` is ALSO rejected: it produces `expected_atomic = 0`,
/// which makes the `client_amount < expected_atomic` enforcement always false —
/// any payload (even one paying 0) would satisfy the amount check, serving the
/// tool for free on a config typo. Per the x402 money-path rule ("reject zero
/// amount before building anything"), a zero/non-positive price fails closed.
pub fn validate_price_usdc(price_usdc: f64) -> Result<(), String> {
    if !price_usdc.is_finite() {
        return Err(format!(
            "price_per_request_usdc is non-finite ({price_usdc}); \
             refusing to cast NaN/∞ to u64"
        ));
    }
    if price_usdc <= 0.0 {
        return Err(format!(
            "price_per_request_usdc is non-positive ({price_usdc}); \
             a zero or negative price would serve the tool for free"
        ));
    }
    if price_usdc > PRICE_USDC_MAX {
        return Err(format!(
            "price_per_request_usdc ({price_usdc}) exceeds u64 range \
             after ×1_000_000 conversion"
        ));
    }
    Ok(())
}

/// All three atomic-USDC components of a paid service request, derived from a
/// single `price_per_request_usdc` value.
///
/// Centralising the breakdown in one struct (instead of recomputing each field
/// inline at the call site) prevents drift if the platform-fee percentage ever
/// changes — the call site only knows about `total_atomic`, `provider_atomic`,
/// and `fee_atomic` as derived values, never as independent expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceCost {
    /// Amount the upstream provider receives, in atomic USDC (6 decimals).
    pub provider_atomic: u64,
    /// Platform fee on top of the provider amount, in atomic USDC.
    pub fee_atomic: u64,
    /// Total amount the client must pay, in atomic USDC. Always equals
    /// `provider_atomic + fee_atomic`.
    pub total_atomic: u64,
}

/// Compute the full atomic-USDC cost breakdown for a flat-priced service.
///
/// Uses integer arithmetic to avoid floating-point precision loss on financial
/// amounts: the `price_usdc` is converted to atomic units (6 decimals) once via
/// the only `f64 → u64` cast in the path, then the 5% platform fee is applied
/// in pure integer math (`provider * 105 / 100`, applied exactly once).
///
/// Returns `Err` (not `Ok` with zeros) on NaN/Inf/negative/overflow input so the
/// caller fails-closed — a corrupt registry entry must reject the request, not
/// serve it for free. See [`validate_price_usdc`] for the rationale.
pub fn compute_service_cost(price_usdc: f64) -> Result<ServiceCost, String> {
    validate_price_usdc(price_usdc)?;
    let provider_atomic = (price_usdc * 1_000_000.0).round() as u64;
    // 5% platform fee: total = provider * 105 / 100. `saturating_mul` is
    // belt-and-braces — `validate_price_usdc` already capped the magnitude.
    let total_atomic = provider_atomic.saturating_mul(105) / 100;
    let fee_atomic = total_atomic.saturating_sub(provider_atomic);
    Ok(ServiceCost {
        provider_atomic,
        fee_atomic,
        total_atomic,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Convenience: total atomic cost only.
    fn total_atomic(price_usdc: f64) -> Result<u64, String> {
        compute_service_cost(price_usdc).map(|c| c.total_atomic)
    }

    #[test]
    fn test_compute_service_cost_basic() {
        // 0.01 USDC = 10_000 atomic; with 5% fee = 10_500 total, fee = 500
        let cost = compute_service_cost(0.01).unwrap();
        assert_eq!(cost.provider_atomic, 10_000);
        assert_eq!(cost.fee_atomic, 500);
        assert_eq!(cost.total_atomic, 10_500);
    }

    #[test]
    fn test_compute_service_cost_breakdown_sums_to_total() {
        // NB: 0.0 is intentionally NOT in this list — a zero price is rejected
        // (fail-closed) by `validate_price_usdc`; see
        // `test_validate_price_usdc_rejects_zero`.
        for price in [0.001, 0.01, 0.0042, 1.0, 12.345, 1_000.0] {
            let cost = compute_service_cost(price).unwrap();
            assert_eq!(
                cost.provider_atomic + cost.fee_atomic,
                cost.total_atomic,
                "breakdown invariant broken for price={price}"
            );
        }
    }

    #[test]
    fn test_compute_service_cost_applies_single_5pct_fee() {
        // 1 USDC -> 1_000_000 atomic -> +5% -> 1_050_000, NOT 1_102_500
        // (double-applied fee). Pins the fee is applied EXACTLY once.
        assert_eq!(total_atomic(1.0).unwrap(), 1_050_000);
    }

    #[test]
    fn test_compute_service_cost_small() {
        assert_eq!(total_atomic(0.001).unwrap(), 1_050);
    }

    #[test]
    fn test_compute_service_cost_uses_round_not_truncate() {
        // 0.0000015 USDC = 1.5 atomic -> rounds to 2 -> 2 * 105/100 = 2
        assert_eq!(total_atomic(0.0000015).unwrap(), 2);
    }

    #[test]
    fn test_compute_service_cost_rejects_nan() {
        let err = compute_service_cost(f64::NAN).unwrap_err();
        assert!(err.contains("non-finite"), "got: {err}");
    }

    #[test]
    fn test_compute_service_cost_rejects_infinities() {
        assert!(compute_service_cost(f64::INFINITY).is_err());
        assert!(compute_service_cost(f64::NEG_INFINITY).is_err());
    }

    #[test]
    fn test_compute_service_cost_rejects_negative() {
        let err = compute_service_cost(-0.001).unwrap_err();
        assert!(err.contains("non-positive"), "got: {err}");
    }

    #[test]
    fn test_validate_price_usdc_rejects_zero() {
        // A zero price makes `expected_atomic == 0`, so the
        // `client_amount < expected_atomic` enforcement is always false and the
        // tool would serve for free on a config typo. Must fail closed.
        let err = validate_price_usdc(0.0).unwrap_err();
        assert!(err.contains("non-positive"), "got: {err}");
        assert!(compute_service_cost(0.0).is_err(), "0.0 must reject");
        // Negative zero is also non-positive.
        assert!(validate_price_usdc(-0.0).is_err(), "-0.0 must reject");
    }

    #[test]
    fn test_compute_service_cost_rejects_overflow() {
        let err = compute_service_cost(1.0e18_f64).unwrap_err();
        assert!(
            err.contains("exceeds u64 range") || err.contains("overflow"),
            "got: {err}"
        );
    }
}
