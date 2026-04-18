/**
 * Unit-10: redact.ts — redactBase58, redactHex, stripPaymentSignature, sanitizeError
 *
 * Sentinel values used throughout this file:
 *
 * BASE58_SENTINEL — 44 chars, all valid base58 chars.
 *   Must contain at least one char outside the hex alphabet so it is not
 *   coincidentally matched by HEX_RE when tested in isolation.
 *   'z' (index 0) is outside [0-9a-fA-F], guaranteeing isolation.
 *
 * HEX_SENTINEL — 64 chars, all [0-9a-fA-F].
 *   Contains '0' so it does NOT match BASE58_RE (base58 excludes '0').
 */

import { describe, expect, it } from 'vitest';
import {
  redactBase58,
  redactHex,
  sanitizeError,
  stripPaymentSignature,
} from '../../src/util/redact.js';

// ---------------------------------------------------------------------------
// Sentinel constants
// ---------------------------------------------------------------------------

/** 44-char base58 sentinel. Starts with 'z' (outside hex alphabet) so
 *  redactHex leaves it untouched when called in isolation. */
const BASE58_SENTINEL_44 =
  'zABCDEFGHJKLMNPQRSTUVWXYZabcdefghjkmnpqrstuv';
//  1234567890123456789012345678901234567890 1234 — 44 chars

/** 55-char base58 sentinel. */
const BASE58_SENTINEL_55 =
  'zABCDEFGHJKLMNPQRSTUVWXYZabcdefghjkmnpqrstuvwxyzABCDEFG';
//  Trim to exactly 55:
// We'll use a slice-based build to be precise.

/** 88-char base58 sentinel. */
const BASE58_SENTINEL_88 = 'z'.repeat(88);

/** 43-char base58 string (just below minimum threshold of 44). */
const BASE58_43 = 'z'.repeat(43);

/** 89-char base58 string (one over the 88-char max). */
const BASE58_89 = 'z'.repeat(89);

/** 64-char hex sentinel. Contains '0' so BASE58_RE cannot match it. */
const HEX_SENTINEL_64 =
  '0000000000000000aabbccddee112233445566778899aabbccddee1122334455';
// Verify: 16 zeros + 48 hex chars = 64 total

/** 128-char hex sentinel (two copies of the 64-char sentinel concatenated). */
const HEX_SENTINEL_128 = HEX_SENTINEL_64 + HEX_SENTINEL_64;

/** 63-char hex string (just below 64-char minimum). */
const HEX_63 = 'a'.repeat(63);

// ---------------------------------------------------------------------------
// Helpers — build sentinels with exact lengths (validated at runtime)
// ---------------------------------------------------------------------------

function buildBase58Sentinel(len: number): string {
  // Alphabet: valid base58 chars starting with 'z' (non-hex) to guarantee isolation.
  const CHARS = 'zABCDEFGHJKLMNPQRSTUVWXYZabcdefghjkmnpqrstuvwxyz';
  let s = '';
  for (let i = 0; i < len; i++) s += CHARS[i % CHARS.length];
  return s;
}

const S44 = buildBase58Sentinel(44);
const S55 = buildBase58Sentinel(55);
const S88 = buildBase58Sentinel(88);

// Runtime sanity — fail fast if sentinel generation is wrong.
if (S44.length !== 44) throw new Error('S44 length mismatch');
if (S55.length !== 55) throw new Error('S55 length mismatch');
if (S88.length !== 88) throw new Error('S88 length mismatch');

// ---------------------------------------------------------------------------
// redactBase58
// ---------------------------------------------------------------------------

describe('redactBase58', () => {
  it('replaces a 44-char base58 string with [REDACTED]', () => {
    expect(redactBase58(S44)).toBe('[REDACTED]');
  });

  it('replaces a 55-char base58 string with [REDACTED]', () => {
    expect(redactBase58(S55)).toBe('[REDACTED]');
  });

  it('replaces an 88-char base58 string with [REDACTED]', () => {
    expect(redactBase58(S88)).toBe('[REDACTED]');
  });

  it('does NOT replace a 43-char base58 string (one below minimum)', () => {
    const s = 'z'.repeat(43);
    expect(redactBase58(s)).toBe(s);
  });

  it('replaces the first 88 chars of an 89-char base58 run, leaving the last char', () => {
    // BASE58_RE is /[1-9A-HJ-NP-Za-km-z]{44,88}/g — greedy, max 88.
    // A 89-char run of 'z': greedy match consumes 88, leaving 'z' (1 char, below min).
    const s = 'z'.repeat(89);
    expect(redactBase58(s)).toBe('[REDACTED]z');
  });

  it('preserves surrounding text while redacting the sentinel', () => {
    const input = `prefix ${S44} suffix`;
    expect(redactBase58(input)).toBe('prefix [REDACTED] suffix');
  });

  it('does NOT redact a plain English sentence of similar length', () => {
    const sentence =
      'The quick brown fox jumps over the lazy dog and keeps running fast.';
    // Contains spaces and chars not in base58 alphabet ('T', ' ', etc.)
    // No contiguous run of 44+ base58 chars, so nothing should be redacted.
    expect(redactBase58(sentence)).toBe(sentence);
  });
});

// ---------------------------------------------------------------------------
// redactHex
// ---------------------------------------------------------------------------

describe('redactHex', () => {
  it('replaces a 64-char hex string with [REDACTED]', () => {
    expect(redactHex(HEX_SENTINEL_64)).toBe('[REDACTED]');
  });

  it('replaces a 128-char hex string with [REDACTED]', () => {
    expect(redactHex(HEX_SENTINEL_128)).toBe('[REDACTED]');
  });

  it('does NOT replace a 63-char hex string (one below minimum)', () => {
    const s = HEX_63;
    expect(redactHex(s)).toBe(s);
  });

  it('replaces a mixed-case hex string with [REDACTED]', () => {
    // 64 hex chars: 32 lowercase + 32 uppercase alternating.
    const mixed = 'aAbBcCdDeEfF001122334455667788990011223344556677889900AABBCCDDEE';
    expect(mixed.length).toBe(64);
    expect(redactHex(mixed)).toBe('[REDACTED]');
  });

  it('redacts each contiguous hex block independently (space-separated blocks)', () => {
    // Two 64-char blocks separated by a space — each matched independently.
    const input = `${HEX_SENTINEL_64} ${HEX_SENTINEL_64}`;
    expect(redactHex(input)).toBe('[REDACTED] [REDACTED]');
  });
});

// ---------------------------------------------------------------------------
// stripPaymentSignature
// ---------------------------------------------------------------------------

describe('stripPaymentSignature', () => {
  it('strips PAYMENT-SIGNATURE (uppercase) and preserves other headers', () => {
    const input = { 'PAYMENT-SIGNATURE': 'x', Other: 'y' };
    const out = stripPaymentSignature(input);
    expect(out).not.toHaveProperty('PAYMENT-SIGNATURE');
    expect(out).toEqual({ Other: 'y' });
  });

  it('strips payment-signature (lowercase) and preserves other headers', () => {
    const input = { 'payment-signature': 'x', Other: 'y' };
    const out = stripPaymentSignature(input);
    expect(out).not.toHaveProperty('payment-signature');
    expect(out).toEqual({ Other: 'y' });
  });

  it('strips all case variants of payment-signature simultaneously', () => {
    const input = {
      'Payment-Signature': 'first',
      'PaYmEnT-SiGnAtUrE': 'second',
      Unrelated: 'keep',
    };
    const out = stripPaymentSignature(input);
    expect(out).not.toHaveProperty('Payment-Signature');
    expect(out).not.toHaveProperty('PaYmEnT-SiGnAtUrE');
    expect(out).toEqual({ Unrelated: 'keep' });
  });

  it('returns a new object and does NOT mutate the input', () => {
    const input = { 'PAYMENT-SIGNATURE': 'x', Other: 'y' };
    const keysBefore = Object.keys(input).sort();
    const out = stripPaymentSignature(input);
    // Input is unchanged
    expect(Object.keys(input).sort()).toEqual(keysBefore);
    expect(input).toHaveProperty('PAYMENT-SIGNATURE');
    // Output is a different reference
    expect(out).not.toBe(input);
  });
});

// ---------------------------------------------------------------------------
// sanitizeError — HIGH fix verification (WeakMap cycle-guard cache)
// ---------------------------------------------------------------------------

describe('sanitizeError', () => {
  describe('nested cause redaction', () => {
    it('redacts a sentinel in cause.message one level deep', () => {
      const input = { message: 'outer', cause: { message: S44 } };
      const out = sanitizeError(input);
      expect((out.cause as Record<string, unknown>)['message']).toBe(
        '[REDACTED]',
      );
    });

    it('redacts sentinels across a 3-level-deep cause chain', () => {
      const input = {
        message: 'top',
        cause: {
          message: S44,
          cause: {
            message: S44,
            cause: { message: S44 },
          },
        },
      };
      const out = sanitizeError(input);

      const l1 = out.cause as Record<string, unknown>;
      const l2 = l1['cause'] as Record<string, unknown>;
      const l3 = l2['cause'] as Record<string, unknown>;

      expect(l1['message']).toBe('[REDACTED]');
      expect(l2['message']).toBe('[REDACTED]');
      expect(l3['message']).toBe('[REDACTED]');
    });
  });

  describe('WeakMap cycle-guard cache — shared sub-object fix (T1-C F8)', () => {
    it('redacts a sentinel in both cause.key and requestBodyValues.key when they share the same object reference', () => {
      // This is the specific bug the WeakMap fix addresses.
      // Before fix (WeakSet only): second traversal of `shared` returned the
      // original un-redacted object because WeakSet only records *visited*, not
      // the sanitized output. The WeakMap fix caches the sanitized copy so the
      // second traversal returns the already-redacted version.
      const shared = { key: S44 };
      const input = {
        message: 'ok',
        cause: shared,
        requestBodyValues: shared,
      };

      const out = sanitizeError(input) as {
        cause: Record<string, unknown>;
        requestBodyValues: Record<string, unknown>;
      };

      expect(out.cause['key']).toBe('[REDACTED]');
      expect(out.requestBodyValues['key']).toBe('[REDACTED]');
    });

    it('returns the same sanitized object for both cause and requestBodyValues when they share a reference', () => {
      // Identity check: the WeakMap caches the sanitized copy, so both
      // out.cause and out.requestBodyValues should be the identical object.
      const shared = { key: S44 };
      const input = {
        message: 'ok',
        cause: shared,
        requestBodyValues: shared,
      };

      const out = sanitizeError(input) as {
        cause: object;
        requestBodyValues: object;
      };

      expect(out.cause).toBe(out.requestBodyValues);
    });

    it('handles a circular reference through cause without stack overflow', () => {
      // sanitizeError processes specific known fields only (message, stack,
      // cause, responseHeaders, responseBody, requestBodyValues). A circular
      // reference that flows through `cause` exercises the WeakMap cycle-guard.
      //
      // Structure: root.cause = root (cycle via the `cause` field).
      // The WeakMap records root → sanitized copy before recursing into cause,
      // so when redactUnknown encounters root again it returns the cached copy
      // rather than looping forever.
      type CyclicErr = { message: string; cause?: CyclicErr };
      const root: CyclicErr = { message: S44 };
      root.cause = root; // cycle

      // Must complete without stack overflow.
      let out!: CyclicErr;
      expect(() => {
        out = sanitizeError(root) as CyclicErr;
      }).not.toThrow();

      // The sentinel in the top-level message is redacted.
      expect(out.message).toBe('[REDACTED]');

      // The cycle through cause resolves back to the sanitized copy of root,
      // so out.cause is the same object reference as out.
      expect(out.cause).toBe(out);
    });
  });

  describe('responseHeaders — PAYMENT-SIGNATURE stripping', () => {
    it('strips PAYMENT-SIGNATURE from responseHeaders and preserves other headers', () => {
      const input = {
        message: 'err',
        responseHeaders: {
          'PAYMENT-SIGNATURE': S44,
          'x-request-id': 'ok',
        },
      };
      const out = sanitizeError(input) as {
        responseHeaders: Record<string, string>;
      };

      expect(out.responseHeaders).not.toHaveProperty('PAYMENT-SIGNATURE');
      expect(out.responseHeaders['x-request-id']).toBe('ok');

      // The raw sentinel must not appear anywhere in the stripped headers.
      expect(JSON.stringify(out.responseHeaders)).not.toContain(S44);
    });
  });

  describe('requestBodyValues redaction', () => {
    it('redacts a base58 sentinel in a string value inside requestBodyValues', () => {
      const input = {
        message: 'err',
        requestBodyValues: { prompt: S44 },
      };
      const out = sanitizeError(input) as {
        requestBodyValues: Record<string, unknown>;
      };

      expect(out.requestBodyValues['prompt']).toBe('[REDACTED]');
    });

    it('redacts a base58 sentinel nested inside a requestBodyValues object', () => {
      const input = {
        message: 'err',
        requestBodyValues: {
          prompt: { content: S44 },
        },
      };
      const out = sanitizeError(input) as {
        requestBodyValues: { prompt: Record<string, unknown> };
      };

      expect(out.requestBodyValues['prompt']['content']).toBe('[REDACTED]');
    });
  });
});
