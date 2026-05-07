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
 * Pipeline (in order):
 *   1. Slice to `maxLen` characters.
 *   2. Redact `payment-signature[…]` header fragments.
 *   3. Redact 64+ hex sequences (e.g. hex-encoded keys / tx signatures).
 *   4. Redact 44–88 base58 sequences (Solana wallet addresses, base58 tx
 *      signatures). MUST run after the hex pass — the hex alphabet is a
 *      subset of base58, so running base58 first would mask hex keys with
 *      the same `[REDACTED]` token before the hex check sees them.
 *
 * Without steps 3 + 4 the function still leaked wallet addresses, full
 * tx signatures, and hex private-key fragments into downstream error
 * surfaces (mcp client.ts, openclaw-provider index.ts).
 *
 * @param text - Raw gateway response body text.
 * @param maxLen - Maximum length to slice to before redaction (default: 500).
 * @returns Sanitized string safe for error messages.
 */
export function sanitizeGatewayError(text: string, maxLen = 500): string {
  const sliced = text.slice(0, maxLen).replace(/payment-signature[^\s,}"]+/gi, '[redacted]');
  return redactBase58(redactHex(sliced));
}
