//! Nano (XNO) settlement constants, the micro-USD → raw conversion math, and
//! the canonical bytes a receipt signature covers.
//!
//! Nano is a feeless, block-lattice currency: every account has its own chain
//! and a `send` block is final once the network confirms it (sub-second, no
//! mempool). Solvela accepts a confirmed send block as a payment *receipt* —
//! the client publishes the block itself and presents its hash plus a
//! signature by the paying account; the gateway verifies both against a Nano
//! node and the account's public key. See `solvela_x402::nano::NanoVerifier`
//! for the verification contract.
//!
//! Everything here is pure integer math and byte layout with no I/O so the
//! gateway, the SDKs, and the tests share one definition of "how many raw does
//! a quote cost" and "what exactly did the payer sign".
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
//! spend ledger records. The ceiling can over-collect by less than one
//! micro-USD, i.e. up to 10^6 raw (0.000000000000000000000001 XNO).

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
/// Kept equal to the `max_timeout_seconds` the Nano `accepts[]` entry
/// advertises (`MAX_TIMEOUT_SECONDS`, 300 s): a client that pays and then
/// presents inside the advertised window must not be refused as stale after
/// the XNO has already moved (review finding, 2026-11-11).
///
/// This is a defence-in-depth bound, not the replay guarantee: the gateway
/// records every redeemed block hash in a durable, non-expiring store (a
/// PostgreSQL primary key, or Redis `SET NX` when no database is configured),
/// and refuses any hash it has seen before. A block hash never expires
/// on-chain, so freshness alone can never bound replay — a node resync can
/// restamp `local_timestamp` on an old block. The window still rejects a
/// receipt presented long after the payment it references, keeping the normal
/// (never-resynced) path honest.
pub const NANO_RECEIPT_MAX_AGE_SECS: u64 = 300;

/// Exact length of a Nano block hash rendered as uppercase hex (32 bytes).
pub const NANO_BLOCK_HASH_HEX_LEN: usize = 64;

/// Exact length of a Nano signature rendered as hex (64 bytes).
pub const NANO_SIGNATURE_HEX_LEN: usize = 128;

/// Length of a `nano_` address: 5-char prefix + 52-char public key + 8-char
/// checksum, all in Nano's base32 alphabet.
///
/// Only the modern `nano_` prefix is accepted. The legacy `xrb_` spelling
/// encodes the same key; it is rejected (fail closed) rather than normalised,
/// so one account has exactly one wire spelling on Solvela's 402/payload
/// surfaces. Operators must configure the `nano_` form.
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
/// wire spelling — the gateway's dedupe store is keyed on the string, and a
/// case-variant of a used hash must not look like a fresh receipt.
pub fn is_well_formed_block_hash(hash: &str) -> bool {
    hash.len() == NANO_BLOCK_HASH_HEX_LEN
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'A'..=b'F').contains(&b))
}

/// Syntactic check of a receipt signature: exactly 128 hex characters (64
/// bytes), either case.
///
/// Signature bytes are not a replay key, so case does not need canonicalising;
/// both spellings decode to the same 64 bytes.
pub fn is_well_formed_nano_signature(sig: &str) -> bool {
    sig.len() == NANO_SIGNATURE_HEX_LEN && sig.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Domain-separation tag prepended to every receipt signature message.
pub const NANO_RECEIPT_SIGNING_DOMAIN: &[u8] = b"solvela-nano-receipt:v1";

/// The exact bytes a Nano receipt signature must cover.
///
/// Layout (NUL-separated, no trailing NUL):
///
/// ```text
/// solvela-nano-receipt:v1\0<block_hash>\0<amount_raw>\0<pay_to>\0<method>\0<url>
/// ```
///
/// - `block_hash` — the canonical uppercase 64-hex block hash.
/// - `amount_raw` — the **canonical decimal** raw amount the gateway quoted
///   (no leading zeros; the verifier parses `accepted.amount` to `u128` and
///   re-renders it, so a wire spelling like `"0021"` cannot produce a second
///   valid message for the same payment).
/// - `pay_to` — the `nano_` recipient exactly as advertised in `accepts[]`.
/// - `method` / `url` — the request `resource` the 402 was issued for, from
///   the client-echoed [`Resource`](crate::Resource).
///
/// NUL is not valid in any field (hex, decimal, Nano base32, HTTP method, or
/// URL), so the encoding is unambiguous without length prefixes.
///
/// The signature is Nano's Ed25519-with-BLAKE2b-512 over the **BLAKE2b-256
/// digest of these bytes** (32 bytes), hex encoded — not over the variable-
/// length message itself. Nano's signing primitive takes a 32-byte hash, so
/// signing the digest lets an ordinary Nano signer produce the receipt
/// signature: the JS `nanocurrency` library's `signBlock({hash})` and a node's
/// `sign` RPC (where hash signing is enabled) accept a 32-byte hash directly,
/// and no Solvela-specific Ed25519 code is needed. The domain tag inside the
/// preimage keeps it distinct from a real block hash, so the same signature
/// can never be a block signature.
///
/// A client computes [`receipt_signing_message`], takes its BLAKE2b-256
/// digest, signs that digest with the account's raw 32-byte Ed25519 secret key,
/// publishes its send block, and presents both in `NanoPayload`.
pub fn receipt_signing_message(
    block_hash: &str,
    amount_raw: &str,
    pay_to: &str,
    resource_method: &str,
    resource_url: &str,
) -> Vec<u8> {
    let fields = [
        block_hash,
        amount_raw,
        pay_to,
        resource_method,
        resource_url,
    ];
    let capacity =
        NANO_RECEIPT_SIGNING_DOMAIN.len() + fields.iter().map(|f| f.len() + 1).sum::<usize>();
    let mut msg = Vec::with_capacity(capacity);
    msg.extend_from_slice(NANO_RECEIPT_SIGNING_DOMAIN);
    for field in fields {
        msg.push(0);
        msg.extend_from_slice(field.as_bytes());
    }
    msg
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

    #[test]
    fn signature_syntax_is_128_hex_either_case() {
        let upper = "C3AE24D5D6B5ECC6866426A9EE9B4B74B14B7778C8F1956C9DE8EDF87C33C9705BF173A13FF0FBBD9685CD9E3DAB68BC91B3D183A928839186BF5F3282A5080F";
        assert!(is_well_formed_nano_signature(upper));
        assert!(is_well_formed_nano_signature(&upper.to_ascii_lowercase()));
        assert!(!is_well_formed_nano_signature(&upper[..127]));
        assert!(!is_well_formed_nano_signature(&format!("{upper}0")));
        assert!(!is_well_formed_nano_signature(
            "ZZ24E55B2FE2DEE521347541932C79A3AE6DFC27F80139AEB5894257BC94F460CDBF8F36192293B5E31185BC6B32101CC02FF7398AA590AC8706E57532E6BF07"
        ));
        assert!(!is_well_formed_nano_signature(""));
    }

    /// Pins the exact bytes a Nano receipt signature covers. This is a wire
    /// contract: a field, separator or domain-tag change invalidates every
    /// client signature, so it must fail here first.
    #[test]
    fn receipt_signing_message_golden_bytes() {
        let hash = "991CF190094C00F0B68E2E5F75F6BEE95A2E0BD93CEAA4A6734DB9F19B728948";
        let amount = "2100000000000000000000000000";
        let pay_to = "nano_3t6k35gi95xu6tergt6p69ck76ogmitsa8mnijtpxm9fkcm736xtoncuohr3";
        let msg = receipt_signing_message(hash, amount, pay_to, "POST", "/v1/chat/completions");
        // Generated from / cross-checked against Nano's Ed25519-BLAKE2b
        // reference construction (see the x402 golden-vector tests for a
        // signature over exactly these bytes).
        let expected_hex = "736f6c76656c612d6e616e6f2d726563656970743a763100393931434631393030393443303046304236384532453546373546364245453935413245304244393343454141344136373334444239463139423732383934380032313030303030303030303030303030303030303030303030303030006e616e6f5f3374366b333567693935787536746572677436703639636b37366f676d69747361386d6e696a7470786d39666b636d37333678746f6e63756f68723300504f5354002f76312f636861742f636f6d706c6574696f6e73";
        assert_eq!(hex_lower(&msg), expected_hex);

        // Every bound field changes the message (so a signature over one
        // receipt cannot validate a different request).
        let variants = [
            receipt_signing_message(
                "A170D51B94E00371ACE76E35AC81DC9405D5D04D4CEBC399AEACE07AE05DD293",
                amount,
                pay_to,
                "POST",
                "/v1/chat/completions",
            ),
            receipt_signing_message(
                hash,
                "2100000000000000000000000001",
                pay_to,
                "POST",
                "/v1/chat/completions",
            ),
            receipt_signing_message(
                hash,
                amount,
                "nano_3kdbxitaj7f6mrir6miiwtw4muhcc58e6tn5st6rfaxsdnb7gr4roudwn951",
                "POST",
                "/v1/chat/completions",
            ),
            receipt_signing_message(hash, amount, pay_to, "PUT", "/v1/chat/completions"),
            receipt_signing_message(hash, amount, pay_to, "POST", "/v1/messages"),
        ];
        for variant in variants {
            assert_ne!(variant, msg);
        }
    }

    fn hex_lower(bytes: &[u8]) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut out = String::with_capacity(bytes.len() * 2);
        for b in bytes {
            out.push(HEX[(b >> 4) as usize] as char);
            out.push(HEX[(b & 0x0f) as usize] as char);
        }
        out
    }
}
