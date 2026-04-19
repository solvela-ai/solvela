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
 */
export function redactHex(s: string): string {
  return s.replace(HEX_RE, '[REDACTED]');
}

/**
 * Replaces 44–88 character base58 sequences in `s` with `[REDACTED]`.
 * Call redactHex first if the input may contain hex-encoded keys.
 */
export function redactBase58(s: string): string {
  return s.replace(BASE58_RE, '[REDACTED]');
}

/**
 * Sanitize a gateway error body string for safe inclusion in error messages.
 *
 * Slices to maxLen characters then redacts payment-signature header fragments.
 *
 * @param text - Raw gateway response body text.
 * @param maxLen - Maximum length to slice to before redaction (default: 500).
 * @returns Sanitized string safe for error messages.
 */
export function sanitizeGatewayError(text: string, maxLen = 500): string {
  return text.slice(0, maxLen).replace(/payment-signature[^\s,}"]+/gi, '[redacted]');
}
