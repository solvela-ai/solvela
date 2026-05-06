/**
 * Redaction and sanitization utilities for Solvela gateway error handling.
 * All functions are pure — no side effects, no mutation.
 */

/** Matches 64+ hex characters (private keys, tx signatures in hex encoding). */
const HEX_RE = /[0-9a-fA-F]{64,}/g;

/** Matches 44–88 base58 characters (Solana public keys, wallet addresses, base58 tx sigs). */
const BASE58_RE = /[1-9A-HJ-NP-Za-km-z]{44,88}/g;

/**
 * Replaces 64+ character hex sequences in `s` with `[REDACTED]`.
 * Must be called BEFORE redactBase58 because the hex alphabet is a subset
 * of the base58 alphabet.
 *
 * Used internally by `sanitizeGatewayError`; also exported so downstream
 * SDKs that build their own error formatters can compose the same primitives.
 */
export function redactHex(s: string): string {
  return s.replace(HEX_RE, '[REDACTED]');
}

/**
 * Replaces 44–88 character base58 sequences in `s` with `[REDACTED]`.
 * Call redactHex first if the input may contain hex-encoded keys.
 *
 * Used internally by `sanitizeGatewayError`; also exported so downstream
 * SDKs that build their own error formatters can compose the same primitives.
 */
export function redactBase58(s: string): string {
  return s.replace(BASE58_RE, '[REDACTED]');
}

/**
 * Sanitize a gateway error body string for safe inclusion in error messages.
 *
 * Pipeline:
 *   1. Slice to `maxLen` (avoids spending regex work on bytes that get discarded).
 *   2. Replace `payment-signature` header fragments with `[redacted]`.
 *   3. Apply `redactHex` (catches private keys + hex tx signatures, ≥ 64 chars).
 *   4. Apply `redactBase58` (catches wallet addresses + base58 tx signatures,
 *      44–88 chars).
 *
 * Hex must run before base58 because the hex alphabet is a strict subset of
 * the base58 alphabet — a 64-char hex private key would otherwise be matched
 * (and replaced) by the base58 pass first, losing the more specific signal.
 *
 * @param text - Raw gateway response body text.
 * @param maxLen - Maximum length to slice to before redaction (default: 500).
 * @returns Sanitized string safe for error messages.
 */
export function sanitizeGatewayError(text: string, maxLen = 500): string {
  const sliced = text
    .slice(0, maxLen)
    .replace(/payment-signature[^\s,}"]+/gi, '[redacted]');
  return redactBase58(redactHex(sliced));
}
