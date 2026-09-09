use std::sync::atomic::{AtomicU8, Ordering};

/// x402 protocol version.
pub const X402_VERSION: u8 = 2;

/// USDC-SPL mint address on Solana mainnet.
pub const USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

/// Solana mainnet network identifier for x402.
pub const SOLANA_NETWORK: &str = "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp";

/// Solvela escrow program ID deployed on Solana mainnet.
///
/// Pinned alongside [`USDC_MINT`] and [`SOLANA_NETWORK`] so clients can enforce
/// the expected escrow program by default and reject a malicious gateway that
/// advertises a different (attacker-controlled) escrow program.
pub const MAINNET_ESCROW_PROGRAM_ID: &str = "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU";

/// Maximum timeout for payment authorization (5 minutes).
pub const MAX_TIMEOUT_SECONDS: u64 = 300;

/// The platform fee multiplier (1.05 = provider cost + 5%).
pub const PLATFORM_FEE_MULTIPLIER: f64 = 1.05;

/// Platform fee percentage.
pub const PLATFORM_FEE_PERCENT: u8 = 5;

/// Live, process-global platform-fee percentage.
///
/// Seeded from [`PLATFORM_FEE_PERCENT`] and overwritten exactly once at gateway
/// startup from `SOLVELA_PLATFORM_FEE_PERCENT` (see `crates/gateway/src/main.rs`).
/// Every fee-emitting surface MUST read [`platform_fee_percent`] rather than the
/// constant, so a configured fee can never be reported by one surface and
/// silently ignored by another.
static LIVE_PLATFORM_FEE_PERCENT: AtomicU8 = AtomicU8::new(PLATFORM_FEE_PERCENT);

/// A rejected [`set_platform_fee_percent`] call. The live value is untouched.
#[derive(Debug, thiserror::Error)]
pub enum PlatformFeeError {
    #[error("platform fee percent {0} is out of range (expected 0..=100)")]
    OutOfRange(u8),
}

/// The live platform-fee percentage (`0..=100`).
///
/// Defaults to [`PLATFORM_FEE_PERCENT`] until the gateway sets it at boot.
#[inline]
pub fn platform_fee_percent() -> u8 {
    LIVE_PLATFORM_FEE_PERCENT.load(Ordering::Relaxed)
}

/// Set the live platform-fee percentage.
///
/// Rejects anything above 100 — it never clamps, and a rejected call leaves the
/// live value unchanged (a partially-applied money knob is worse than a refused
/// one). Intended to be called ONCE, at process startup, before serving.
pub fn set_platform_fee_percent(percent: u8) -> Result<(), PlatformFeeError> {
    if percent > 100 {
        return Err(PlatformFeeError::OutOfRange(percent));
    }
    LIVE_PLATFORM_FEE_PERCENT.store(percent, Ordering::Relaxed);
    Ok(())
}

/// The live platform-fee multiplier for the (non-money-path) f64 estimate
/// surfaces: `1.0 + pct/100`. At the default 5 this is bit-exactly
/// [`PLATFORM_FEE_MULTIPLIER`] (1.05), so existing float pins hold.
#[inline]
pub fn platform_fee_multiplier() -> f64 {
    1.0 + platform_fee_percent() as f64 / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The knob is process-global; the mutating spec lives in
    /// `tests/fee_knob.rs` (its own test binary). This only pins the
    /// non-mutating identity that every float pin relies on.
    #[test]
    fn default_multiplier_matches_the_constant_bit_exactly() {
        assert_eq!(platform_fee_multiplier(), PLATFORM_FEE_MULTIPLIER);
    }
}
