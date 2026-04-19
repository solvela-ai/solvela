/**
 * Tests for redaction and sanitization utilities.
 */

import { describe, it } from 'node:test';
import assert from 'node:assert/strict';

import { redactHex, redactBase58, sanitizeGatewayError } from '../src/redact.ts';

// ---------------------------------------------------------------------------
// Test fixtures
// ---------------------------------------------------------------------------

// 64-char hex string (private key length)
const HEX_64 = 'a'.repeat(64);
// 44-char base58 string (Solana pubkey length)
const BASE58_44 = '4hXTCkRzt9WyecNzV1XPgCDfGAZzQKNxLXgynz5QDuWJ';

// ---------------------------------------------------------------------------
// redactHex
// ---------------------------------------------------------------------------

describe('redactHex', () => {
  it('redacts 64-char hex sequences', () => {
    const result = redactHex(`key=${HEX_64}`);
    assert.equal(result, 'key=[REDACTED]');
  });

  it('redacts multiple hex sequences in one string', () => {
    const result = redactHex(`${HEX_64} and ${HEX_64}`);
    assert.equal(result, '[REDACTED] and [REDACTED]');
  });

  it('does not redact short hex strings (< 64 chars)', () => {
    const short = 'a'.repeat(63);
    const result = redactHex(short);
    assert.equal(result, short);
  });

  it('preserves safe non-hex text', () => {
    const safe = 'Gateway error 402: Payment required';
    assert.equal(redactHex(safe), safe);
  });
});

// ---------------------------------------------------------------------------
// redactBase58
// ---------------------------------------------------------------------------

describe('redactBase58', () => {
  it('redacts 44-char base58 sequences', () => {
    const result = redactBase58(`wallet=${BASE58_44}`);
    assert.equal(result, 'wallet=[REDACTED]');
  });

  it('does not redact short base58 strings (< 44 chars)', () => {
    const short = '4hXTCkRzt9WyecNzV1XPgCDfGAZzQKNxLXgyn'; // 38 chars
    const result = redactBase58(short);
    assert.equal(result, short);
  });

  it('preserves safe non-base58 text', () => {
    const safe = 'error: payment required for model gpt-4o';
    assert.equal(redactBase58(safe), safe);
  });
});

// ---------------------------------------------------------------------------
// sanitizeGatewayError
// ---------------------------------------------------------------------------

describe('sanitizeGatewayError', () => {
  it('redacts payment-signature header fragments', () => {
    const text = 'error: payment-signatureABCDEF123 was rejected';
    const result = sanitizeGatewayError(text);
    assert.equal(result, 'error: [redacted] was rejected');
  });

  it('redacts multiple payment-signature fragments', () => {
    const text = 'err: payment-signatureABC and payment-signatureDEF';
    const result = sanitizeGatewayError(text);
    assert.equal(result, 'err: [redacted] and [redacted]');
  });

  it('is case-insensitive for payment-signature', () => {
    const text = 'Payment-SignatureXYZ123 invalid';
    const result = sanitizeGatewayError(text);
    assert.equal(result, '[redacted] invalid');
  });

  it('slices to maxLen (default 500) before redacting', () => {
    const long = 'a'.repeat(1000);
    const result = sanitizeGatewayError(long);
    assert.equal(result.length, 500);
  });

  it('respects custom maxLen', () => {
    const long = 'a'.repeat(1000);
    const result = sanitizeGatewayError(long, 100);
    assert.equal(result.length, 100);
  });

  it('preserves safe text that does not contain payment-signature', () => {
    const safe = 'Gateway error 500: Internal server error';
    assert.equal(sanitizeGatewayError(safe), safe);
  });

  it('handles empty string', () => {
    assert.equal(sanitizeGatewayError(''), '');
  });
});
