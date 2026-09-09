//! Shared flat-price + platform-fee money math for paid marketplace tools.
//!
//! [`apply_platform_fee_atomic`] and [`split_total_atomic`] are the ONE helper
//! pair every fee site in the gateway routes through — forward (provider →
//! total) and inverse (total → provider/fee split). Both read the LIVE
//! platform-fee percent ([`solvela_protocol::platform_fee_percent`]), so a
//! configured fee can never be honoured by one surface and hard-coded away by
//! another (the inverse sites used to hard-code `* 100 / 105`, which at a 0%
//! fee would have recorded a phantom ~4.76% fee).
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
//!   `provider * (100 + pct) / 100`.
//! - A corrupt price returns `Err`, never `Ok` with zeros — a misconfigured
//!   tool must reject the request, never serve for free.

use solvela_protocol::platform_fee_percent;

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

/// Apply the live platform fee to a provider amount: `provider * (100 + pct) / 100`.
///
/// The canonical FORWARD fee formula, in integer atomic USDC. Intermediates are
/// `u128` so a large provider amount cannot wrap; the result is `None` when the
/// total would not fit `u64`.
///
/// Fail-closed by construction: callers get `None`, never a saturated ceiling.
/// A saturating cap would silently under-charge by capping the bill at
/// `u64::MAX` instead of refusing a nonsensical amount.
#[must_use]
pub fn apply_platform_fee_atomic(provider_atomic: u64) -> Option<u64> {
    let pct = u128::from(platform_fee_percent());
    u64::try_from(u128::from(provider_atomic) * (100 + pct) / 100).ok()
}

/// Split a fee-inclusive total back into `(provider, fee)` at the live percent.
///
/// The canonical INVERSE of [`apply_platform_fee_atomic`]:
/// `provider = total * 100 / (100 + pct)`, `fee = total - provider`. The two
/// components always sum to `total` exactly (any integer-rounding skew lands in
/// the fee component, which is the accepted treatment across the codebase), and
/// at a 0% fee the provider share IS the total with a zero fee — no phantom
/// ~4.76% split.
///
/// Total (not the split) is authoritative here: these sites already billed
/// `total`, so the split must reconcile to it rather than re-derive it.
#[must_use]
pub fn split_total_atomic(total_atomic: u64) -> (u64, u64) {
    let pct = u128::from(platform_fee_percent());
    // `100 + pct >= 100`, so the quotient is <= total and always fits u64.
    let provider_atomic = (u128::from(total_atomic) * 100 / (100 + pct)) as u64;
    (provider_atomic, total_atomic - provider_atomic)
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
/// the only `f64 → u64` cast in the path, then the live platform fee is applied
/// in pure integer math ([`apply_platform_fee_atomic`], applied exactly once).
///
/// Returns `Err` (not `Ok` with zeros) on NaN/Inf/negative/overflow input so the
/// caller fails-closed — a corrupt registry entry must reject the request, not
/// serve it for free. See [`validate_price_usdc`] for the rationale.
pub fn compute_service_cost(price_usdc: f64) -> Result<ServiceCost, String> {
    validate_price_usdc(price_usdc)?;
    let provider_atomic = (price_usdc * 1_000_000.0).round() as u64;
    let total_atomic = apply_platform_fee_atomic(provider_atomic).ok_or_else(|| {
        format!(
            "price_per_request_usdc ({price_usdc}) overflows u64 atomic USDC              once the platform fee is applied"
        )
    })?;
    let fee_atomic = total_atomic - provider_atomic;
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

    /// The fee knob is PROCESS-GLOBAL and these unit tests share a process
    /// with every other `gateway --lib` test, so they must NEVER call
    /// `set_platform_fee_percent` — they pin the math at the default 5%.
    /// The 0% behaviour is pinned end-to-end in `tests/fee_zero.rs`, which
    /// gets its own test binary (own process).
    #[test]
    fn apply_platform_fee_atomic_is_the_canonical_forward_formula() {
        assert_eq!(apply_platform_fee_atomic(1_000_000), Some(1_050_000));
        assert_eq!(apply_platform_fee_atomic(10_000), Some(10_500));
        assert_eq!(apply_platform_fee_atomic(0), Some(0));
        // Integer floor, never round-up: 19 × 105 / 100 = 19.95 → 19.
        assert_eq!(apply_platform_fee_atomic(19), Some(19));
    }

    #[test]
    fn apply_platform_fee_atomic_fails_closed_on_u64_overflow() {
        // The old `saturating_mul(105) / 100` silently capped here, quoting a
        // bill ~5% BELOW the real one. Overflow must refuse, not cap.
        assert_eq!(apply_platform_fee_atomic(u64::MAX), None);
        // Largest provider amount whose total still fits u64.
        let max_ok = (u64::MAX as u128 * 100 / 105) as u64;
        assert!(apply_platform_fee_atomic(max_ok).is_some());
    }

    #[test]
    fn split_total_atomic_always_reconciles_to_the_total() {
        for total in [0u64, 1, 19, 20, 9_187, 10_500, 1_050_000, u64::MAX] {
            let (provider, fee) = split_total_atomic(total);
            assert_eq!(
                provider.checked_add(fee),
                Some(total),
                "split must sum to the total exactly (total={total})"
            );
            assert!(provider <= total, "provider share cannot exceed the total");
        }
    }

    #[test]
    fn split_total_atomic_recovers_the_provider_amount_when_the_fee_divides_evenly() {
        // The inverse is lossy by one atomic unit whenever the forward
        // `× 105 / 100` floors (e.g. provider 19 → total 19 → provider 18) —
        // that skew is deliberately absorbed by the fee component. Where the
        // forward math is exact (provider a multiple of 20 at 5%), the
        // round-trip must be exact too.
        for provider in [0u64, 20, 10_000, 1_000_000, 123_456_780] {
            let total = apply_platform_fee_atomic(provider).expect("no overflow");
            let (split_provider, fee) = split_total_atomic(total);
            assert_eq!(
                split_provider, provider,
                "exact-division round-trip must recover the provider amount \
                 (provider={provider})"
            );
            assert_eq!(split_provider + fee, total);
        }
    }

    #[test]
    fn split_total_atomic_matches_the_hard_coded_idiom_it_replaced() {
        // At the default 5% the helper must be byte-identical to the
        // `* 100 / 105` expressions removed from the discovery-402 breakdown
        // and the channel-draw receipt.
        for total in [0u64, 1, 19, 9_187, 10_500, 1_050_000, u64::MAX] {
            let legacy_provider = (total as u128 * 100 / 105) as u64;
            assert_eq!(
                split_total_atomic(total),
                (legacy_provider, total - legacy_provider),
                "total={total}"
            );
        }
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
