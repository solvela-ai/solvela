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
    // Use spaces so neither the hex nor base58 redactor matches —
    // we're asserting the slice happens at maxLen, not the redaction.
    const long = ' '.repeat(1000);
    const result = sanitizeGatewayError(long);
    assert.equal(result.length, 500);
  });

  it('respects custom maxLen', () => {
    const long = ' '.repeat(1000);
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

  it('redacts long hex sequences (e.g. hex-encoded keys / tx signatures)', () => {
    // 64 hex chars — minimum match for redactHex.
    const hex = 'a'.repeat(64);
    const text = `signing failed for key ${hex}: invalid`;
    const result = sanitizeGatewayError(text);
    assert.equal(result, 'signing failed for key [REDACTED]: invalid');
  });

  it('redacts base58 wallet-address-shaped substrings', () => {
    // 44-char base58 (typical Solana pubkey length).
    const wallet = 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v';
    const text = `payee ${wallet} rejected the deposit`;
    const result = sanitizeGatewayError(text);
    assert.equal(result, 'payee [REDACTED] rejected the deposit');
  });

  it('redacts hex first, then base58 — order matters for correctness', () => {
    // 64+ hex string is also valid base58 (hex alphabet ⊂ base58 alphabet).
    // If base58 ran first, it would mask the hex with [REDACTED] before the
    // hex pass saw it — same visual outcome but the documented-invariant
    // order is hex → base58.
    const hex = 'abcdef1234567890'.repeat(4); // 64 chars, all hex
    const text = `key=${hex}`;
    const result = sanitizeGatewayError(text);
    assert.equal(result, 'key=[REDACTED]');
  });

  it('still redacts payment-signature alongside hex/base58', () => {
    const wallet = 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v';
    const text = `error: payment-signatureXYZ from ${wallet} was rejected`;
    const result = sanitizeGatewayError(text);
    assert.equal(result, 'error: [redacted] from [REDACTED] was rejected');
  });
});
