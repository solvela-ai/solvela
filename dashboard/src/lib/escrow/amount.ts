// USDC atomic-unit conversion for the escrow-funding money path.
//
// USDC has 6 decimals; the atomic unit ("micro-USDC") is 1e-6 USDC. ALL math
// here is integer / BigInt — never f64 — per the Solvela money-path rules
// ([solvela-fintech] §1). Every conversion is fail-closed: any malformed,
// negative, zero, over-precise, or overflowing input throws rather than
// silently coercing to 0 or a wrong value (a silent 0 would build a deposit the
// user never intended, or under-fund the escrow).
//
// The gateway (`POST /v1/escrow/deposit-tx`) requires `amount` to be a CANONICAL
// integer string (`> 0`, no leading zeros / sign / whitespace) and rejects
// anything else, so `usdcDecimalToAtomic` emits exactly that canonical form.

/** USDC fractional precision (decimals). */
const USDC_DECIMALS = 6;

/** Zero as a BigInt (avoids `0n` literal so the code typechecks on ES2017). */
const ZERO = BigInt(0);

/** Maximum value of a Solana u64 — the on-chain amount field width. */
const U64_MAX = (BigInt(1) << BigInt(64)) - BigInt(1);

/**
 * Convert a decimal-USDC string (as a user types it) to a canonical atomic
 * micro-USDC integer string.
 *
 * Accepts only a plain decimal: an optional integer part, an optional single
 * `.`, and up to 6 fractional digits. No sign, no whitespace, no exponent, no
 * hex, no thousands separators.
 *
 * Throws on: empty / non-numeric / negative / signed / whitespace-padded /
 * `> 6` fractional digits / zero / overflow beyond u64. Returns a canonical
 * integer string with no leading zeros.
 */
export function usdcDecimalToAtomic(input: string): string {
  if (typeof input !== "string" || input.length === 0) {
    throw new Error("amount is required");
  }

  // Strict grammar: digits, optional single dot, up to 6 fractional digits.
  // Disallows sign, whitespace, exponent, hex, NaN/Infinity, "1.2.3", bare "."
  // (requires at least one digit on one side of the dot). Anchored, so any
  // surrounding whitespace fails closed.
  const match = /^(\d*)(?:\.(\d{0,6}))?$/.exec(input);
  if (!match) {
    throw new Error(
      `invalid USDC amount: "${input}" (use a plain decimal with at most ${USDC_DECIMALS} fractional digits)`,
    );
  }

  const intPart = match[1] ?? "";
  const fracPart = match[2] ?? "";

  // The regex permits "" (empty), "." (no digits at all), and ".5"/"5." — but a
  // value with no digit anywhere is not a number. Require at least one digit.
  if (intPart.length === 0 && fracPart.length === 0) {
    throw new Error(`invalid USDC amount: "${input}"`);
  }

  // Pad the fraction to exactly 6 digits, then concatenate as an integer.
  // String concatenation (not float multiply) keeps this exact.
  const paddedFrac = fracPart.padEnd(USDC_DECIMALS, "0");
  const atomic = BigInt(`${intPart === "" ? "0" : intPart}${paddedFrac}`);

  if (atomic <= ZERO) {
    throw new Error("amount must be greater than zero");
  }
  if (atomic > U64_MAX) {
    throw new Error("amount exceeds the maximum supported value");
  }

  // BigInt#toString is already canonical (no leading zeros).
  return atomic.toString();
}

/**
 * Convert an atomic micro-USDC amount to a human display decimal-USDC string
 * (e.g. `"2625"` → `"0.002625"`, `"1000000"` → `"1"`). Trailing fractional
 * zeros are trimmed. Integer / BigInt only.
 *
 * Throws on a malformed atomic input (non-integer, negative, non-numeric).
 */
export function atomicToUsdcDisplay(atomic: string | bigint): string {
  let value: bigint;
  if (typeof atomic === "bigint") {
    value = atomic;
  } else {
    if (!/^\d+$/.test(atomic)) {
      throw new Error(`invalid atomic amount: "${atomic}"`);
    }
    value = BigInt(atomic);
  }

  if (value < ZERO) {
    throw new Error("atomic amount must be non-negative");
  }

  const scale = BigInt(10) ** BigInt(USDC_DECIMALS);
  const whole = value / scale;
  const frac = value % scale;

  if (frac === ZERO) {
    return whole.toString();
  }

  // Zero-pad the fraction to 6 digits, then trim trailing zeros for display.
  const fracStr = frac.toString().padStart(USDC_DECIMALS, "0").replace(/0+$/, "");
  return `${whole.toString()}.${fracStr}`;
}
