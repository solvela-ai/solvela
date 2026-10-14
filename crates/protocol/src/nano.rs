//! Nano (XNO) settlement constants and the micro-USD → raw conversion math.
//!
//! Nano is a feeless, block-lattice currency: every account has its own chain
//! and a `send` block is final once the network confirms it (sub-second, no
//! mempool). Solvela accepts a confirmed send block as a payment *receipt* —
//! the client publishes the block itself and presents its hash; the gateway
//! verifies the block against a Nano node. See
//! `solvela_x402::nano::NanoVerifier` for the verification contract.
//!
//! Everything here is pure integer math with no I/O so the gateway, the SDKs,
//! and the tests all share one definition of "how many raw does a quote cost".
//!
//! # Units
//!
//! - Solvela quotes every price in **micro-USD** (atomic USDC, 6 decimals).
//! - Nano's atomic unit is **raw**; `1 XNO = 10^30 raw`.
//! - The operator supplies the exchange rate as **USD per XNO**, parsed to an
//!   integer number of micro-USD per XNO ([`parse_usd_per_xno`]) so the
//!   conversion never touches floating point on the money path.
//!
//! # Rounding
//!
//! [`usdc_atomic_to_nano_raw`] always rounds **up** to the next whole raw: a
//! quote converted to XNO can never collect less than the micro-USD figure the
//! spend ledger records. Because raw is 24 orders of magnitude finer than
//! micro-USD, the rounding is immaterial to the payer (at most 1 raw).

/// x402 network identifier for Nano mainnet.
///
/// Nano has no chain-id convention (it is not EVM, not CAIP-2 registered), so
/// the identifier is the plain `nano:mainnet` the issue proposed. A deployment
/// never advertises this unless `[nano]` is configured.
pub const NANO_NETWORK: &str = "nano:mainnet";

/// The `asset` string advertised in a Nano `accepts[]` entry.
///
/// Nano has a single native asset and no token contracts, so the ticker is the
/// asset identifier (there is no mint / contract address to quote).
pub const NANO_ASSET: &str = "XNO";

/// Raw per XNO (Nano's atomic unit has 30 decimals).
pub const RAW_PER_XNO: u128 = 1_000_000_000_000_000_000_000_000_000_000;

/// Micro-USD per USD (atomic USDC has 6 decimals).
const MICRO_PER_USD: u64 = 1_000_000;

/// `RAW_PER_XNO / MICRO_PER_USD` — raw per (XNO · micro-USD⁻¹), used to keep
/// the intermediate product inside `u128` (see [`usdc_atomic_to_nano_raw`]).
const RAW_PER_XNO_OVER_MICRO: u128 = RAW_PER_XNO / MICRO_PER_USD as u128;

/// Receipt freshness window, in seconds, measured from the moment the
/// gateway's Nano node first saw the block (`block_info.local_timestamp`).
///
/// Deliberately the SAME order of magnitude as a Solana recent-blockhash
/// transaction's ~90 s validity so the existing gateway replay cache
/// (120 s TTL on the Redis and in-memory paths alike) covers a Nano receipt
/// exactly the way it covers a Solana signature. A block hash never expires
/// on-chain, so this window — not the chain — is what bounds replay.
pub const NANO_RECEIPT_MAX_AGE_SECS: u64 = 90;

/// Exact length of a Nano block hash rendered as uppercase hex (32 bytes).
pub const NANO_BLOCK_HASH_HEX_LEN: usize = 64;

/// Length of a `nano_` address: 5-char prefix + 52-char public key + 8-char
/// checksum, all in Nano's base32 alphabet.
pub const NANO_ADDRESS_LEN: usize = 65;

/// Nano's base32 alphabet (no `0`, `2`, `l`, `v`).
const NANO_BASE32_ALPHABET: &str = "13456789abcdefghijkmnopqrstuwxyz";

/// Why a Nano amount or rate could not be produced. Every variant is a
/// fail-closed condition — the caller must refuse to quote, never fall back.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum NanoAmountError {
    #[error("USD-per-XNO rate is empty")]
    EmptyRate,
    #[error("USD-per-XNO rate '{0}' is not a plain decimal (digits with an optional '.')")]
    MalformedRate(String),
    #[error("USD-per-XNO rate '{0}' has more than 6 fractional digits")]
    TooPrecise(String),
    #[error("USD-per-XNO rate must be greater than zero")]
    ZeroRate,
    #[error("USD-per-XNO rate '{0}' overflows the supported range")]
    RateOverflow(String),
    #[error("micro-USD amount {0} is too large to convert to raw without overflow")]
    AmountOverflow(u64),
}

/// Parse an operator-supplied `USD per XNO` rate (e.g. `"1.25"`) into an
/// integer number of **micro-USD per XNO**.
///
/// Accepts plain decimals only: digits, at most one `.`, at most six
/// fractional digits (micro-USD resolution). Rejects empty, zero, negative,
/// scientific notation, separators, and anything beyond `u64` — a mistyped
/// rate must refuse to boot, never silently quote at the wrong price.
pub fn parse_usd_per_xno(rate: &str) -> Result<u64, NanoAmountError> {
    let s = rate.trim();
    if s.is_empty() {
        return Err(NanoAmountError::EmptyRate);
    }
    let (int_part, frac_part) = match s.split_once('.') {
        Some((i, f)) => (i, f),
        None => (s, ""),
    };
    let is_digits = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
    // A '.' must be followed by at least one digit ("5." is malformed).
    if !is_digits(int_part) || (s.contains('.') && !is_digits(frac_part)) {
        return Err(NanoAmountError::MalformedRate(rate.to_string()));
    }
    if frac_part.len() > 6 {
        return Err(NanoAmountError::TooPrecise(rate.to_string()));
    }
    let integer: u64 = int_part
        .parse()
        .map_err(|_| NanoAmountError::RateOverflow(rate.to_string()))?;
    // Right-pad the fraction to exactly six digits ("25" → "250000").
    let frac_padded = format!("{frac_part:0<6}");
    let fractional: u64 = frac_padded
        .parse()
        .map_err(|_| NanoAmountError::MalformedRate(rate.to_string()))?;
    let micro = integer
        .checked_mul(MICRO_PER_USD)
        .and_then(|v| v.checked_add(fractional))
        .ok_or_else(|| NanoAmountError::RateOverflow(rate.to_string()))?;
    if micro == 0 {
        return Err(NanoAmountError::ZeroRate);
    }
    Ok(micro)
}

/// Convert a micro-USD quote into the number of **raw** the payer must send,
/// given the rate as micro-USD per XNO (from [`parse_usd_per_xno`]).
///
/// `raw = ceil(micro_usd × 10^30 / micro_usd_per_xno)`, computed as
/// `ceil(micro_usd × 10^24 / rate) × 10^6` so the intermediate product stays
/// within `u128` for every quote below ~10^14 micro-USD (100 million USD).
/// Any step that would overflow — including a result above `u128::MAX` raw
/// (≈3.4 × 10^8 XNO, more than Nano's entire supply) — fails closed with
/// [`NanoAmountError::AmountOverflow`] rather than wrapping. The second factor
/// is exact (10^6 raw = 10^-24 XNO), so the only rounding is the single
/// ceiling, which can over-collect by at most 10^6 raw
/// = 0.000000000000000000000001 XNO.
pub fn usdc_atomic_to_nano_raw(
    micro_usd: u64,
    micro_usd_per_xno: u64,
) -> Result<u128, NanoAmountError> {
    if micro_usd_per_xno == 0 {
        return Err(NanoAmountError::ZeroRate);
    }
    let numerator = (micro_usd as u128)
        .checked_mul(RAW_PER_XNO_OVER_MICRO)
        .ok_or(NanoAmountError::AmountOverflow(micro_usd))?;
    let rate = micro_usd_per_xno as u128;
    let quotient = numerator / rate;
    let rounded_up = if numerator % rate == 0 {
        quotient
    } else {
        quotient + 1
    };
    rounded_up
        .checked_mul(MICRO_PER_USD as u128)
        .ok_or(NanoAmountError::AmountOverflow(micro_usd))
}

/// Syntactic check of a `nano_` address: prefix, exact length, and alphabet.
///
/// This does NOT verify the 8-character Blake2b checksum (the protocol crate
/// has no hash dependency). That is safe for the gateway's purposes: the
/// verifier compares the node-rendered `link_as_account` (which always carries
/// a correct checksum) for string equality, so a mistyped checksum in config
/// can only make every Nano payment fail closed — never misdirect one.
pub fn is_well_formed_nano_address(addr: &str) -> bool {
    let Some(body) = addr.strip_prefix("nano_") else {
        return false;
    };
    addr.len() == NANO_ADDRESS_LEN && body.chars().all(|c| NANO_BASE32_ALPHABET.contains(c))
}

/// Syntactic check of a block hash: exactly 64 **uppercase** hex characters.
///
/// Uppercase is required (not merely accepted) so one block has exactly one
/// wire spelling — the gateway's replay cache is keyed on the string, and a
/// case-variant of a used hash must not look like a fresh receipt.
pub fn is_well_formed_block_hash(hash: &str) -> bool {
    hash.len() == NANO_BLOCK_HASH_HEX_LEN
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_integer_and_decimal_rates() {
        assert_eq!(parse_usd_per_xno("1"), Ok(1_000_000));
        assert_eq!(parse_usd_per_xno("1.25"), Ok(1_250_000));
        assert_eq!(parse_usd_per_xno("0.5"), Ok(500_000));
        assert_eq!(parse_usd_per_xno("  2.000001 "), Ok(2_000_001));
        assert_eq!(parse_usd_per_xno("0.000001"), Ok(1));
    }

    #[test]
    fn rejects_malformed_rates_fail_closed() {
        assert_eq!(parse_usd_per_xno(""), Err(NanoAmountError::EmptyRate));
        assert_eq!(parse_usd_per_xno("   "), Err(NanoAmountError::EmptyRate));
        assert_eq!(parse_usd_per_xno("0"), Err(NanoAmountError::ZeroRate));
        assert_eq!(parse_usd_per_xno("0.0"), Err(NanoAmountError::ZeroRate));
        assert_eq!(
            parse_usd_per_xno("0.0000001"),
            Err(NanoAmountError::TooPrecise("0.0000001".into()))
        );
        for bad in [
            "-1", "1e3", "1_000", "1.2.3", ".5", "5.", "abc", "1,5", "+1",
        ] {
            assert!(
                matches!(
                    parse_usd_per_xno(bad),
                    Err(NanoAmountError::MalformedRate(_))
                ),
                "{bad:?} must be rejected as malformed"
            );
        }
        assert!(matches!(
            parse_usd_per_xno("18446744073709551616"),
            Err(NanoAmountError::RateOverflow(_))
        ));
    }

    #[test]
    fn one_usd_at_one_usd_per_xno_is_exactly_one_xno() {
        assert_eq!(
            usdc_atomic_to_nano_raw(1_000_000, 1_000_000),
            Ok(RAW_PER_XNO)
        );
    }

    #[test]
    fn converts_at_a_fractional_rate() {
        // $0.002625 at $1.25/XNO = 0.0021 XNO = 2.1 × 10^27 raw (exact).
        assert_eq!(
            usdc_atomic_to_nano_raw(2_625, 1_250_000),
            Ok(2_100_000_000_000_000_000_000_000_000)
        );
        // $1 at $3/XNO = 0.333… XNO — must round UP, never down.
        let raw = usdc_atomic_to_nano_raw(1_000_000, 3_000_000).unwrap();
        assert!(
            raw * 3 >= RAW_PER_XNO,
            "rounded-up raw must cover the quote"
        );
        assert!(
            raw * 3 - RAW_PER_XNO <= 3_000_000,
            "over-collection is at most one rounding step"
        );
    }

    #[test]
    fn zero_quote_is_zero_raw() {
        assert_eq!(usdc_atomic_to_nano_raw(0, 1_250_000), Ok(0));
    }

    #[test]
    fn conversion_fails_closed_on_overflow_and_zero_rate() {
        assert_eq!(
            usdc_atomic_to_nano_raw(1, 0),
            Err(NanoAmountError::ZeroRate)
        );
        assert_eq!(
            usdc_atomic_to_nano_raw(u64::MAX, 1),
            Err(NanoAmountError::AmountOverflow(u64::MAX))
        );
        // A $100M quote (10^14 micro-USD) at $1/XNO is 10^8 XNO = 10^38 raw —
        // still inside u128; the guard only trips beyond Nano's whole supply.
        assert_eq!(
            usdc_atomic_to_nano_raw(100_000_000_000_000, 1_000_000),
            Ok(100_000_000 * RAW_PER_XNO)
        );
        // The same quote at an absurd 1 micro-USD/XNO rate would exceed u128 raw.
        assert_eq!(
            usdc_atomic_to_nano_raw(100_000_000_000_000, 1),
            Err(NanoAmountError::AmountOverflow(100_000_000_000_000))
        );
    }

    #[test]
    fn nano_address_syntax() {
        let good = "nano_3t6k35gi95xu6tergt6p69ck76ogmitsa8mnijtpxm9fkcm736xtoncuohr3";
        assert!(is_well_formed_nano_address(good));
        assert!(!is_well_formed_nano_address(
            "xrb_3t6k35gi95xu6tergt6p69ck76ogmitsa8mnijtpxm9fkcm736xtoncuohr3"
        ));
        assert!(!is_well_formed_nano_address(
            "nano_3t6k35gi95xu6tergt6p69ck76ogmitsa8mnijtpxm9fkcm736xtoncuohr"
        )); // short
        assert!(!is_well_formed_nano_address(
            "nano_0t6k35gi95xu6tergt6p69ck76ogmitsa8mnijtpxm9fkcm736xtoncuohr3"
        )); // '0'
        assert!(!is_well_formed_nano_address(""));
    }

    #[test]
    fn block_hash_syntax_requires_uppercase_hex() {
        let good = "991CF190094C00F0B68E2E5F75F6BEE95A2E0BD93CEAA4A6734DB9F19B728948";
        assert!(is_well_formed_block_hash(good));
        assert!(!is_well_formed_block_hash(&good.to_ascii_lowercase()));
        assert!(!is_well_formed_block_hash(&good[..63]));
        assert!(!is_well_formed_block_hash(&format!("{good}0")));
        assert!(!is_well_formed_block_hash(""));
    }
}
