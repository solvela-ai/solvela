/**
 * Tests for isStubHeader — stub payment header detection.
 */

import { describe, it } from 'node:test';
import assert from 'node:assert/strict';

import { isStubHeader } from '../src/stub-guard.ts';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function encodeHeader(payload: unknown): string {
  return Buffer.from(JSON.stringify({ payload })).toString('base64');
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe('isStubHeader', () => {
  describe('stub detection', () => {
    it('returns true for STUB_BASE64_TX in payload.transaction', () => {
      const header = encodeHeader({ transaction: 'STUB_BASE64_TX' });
      assert.equal(isStubHeader(header), true);
    });

    it('returns true for STUB_ESCROW_DEPOSIT_TX in payload.deposit_tx', () => {
      const header = encodeHeader({ deposit_tx: 'STUB_ESCROW_DEPOSIT_TX' });
      assert.equal(isStubHeader(header), true);
    });

    it('returns true when both transaction and deposit_tx are stub', () => {
      const header = encodeHeader({
        transaction: 'STUB_BASE64_TX',
        deposit_tx: 'STUB_ESCROW_DEPOSIT_TX',
      });
      assert.equal(isStubHeader(header), true);
    });

    it('returns true for any STUB_ prefix in transaction', () => {
      const header = encodeHeader({ transaction: 'STUB_SOME_OTHER_VALUE' });
      assert.equal(isStubHeader(header), true);
    });
  });

  describe('non-stub detection', () => {
    it('returns false for a real base58 transaction', () => {
      // A realistic base58-encoded Solana transaction signature (non-stub)
      const header = encodeHeader({ transaction: '4hXTCkRzt9WyecNzV1XPgCDfGAZzQKNxLXgynz5QDuWJ' });
      assert.equal(isStubHeader(header), false);
    });

    it('returns false when payload has no transaction or deposit_tx', () => {
      const header = encodeHeader({ amount: '2500' });
      assert.equal(isStubHeader(header), false);
    });

    it('returns false when transaction does not start with STUB_', () => {
      const header = encodeHeader({ transaction: 'REAL_TX_DATA_BASE64_ENCODED' });
      assert.equal(isStubHeader(header), false);
    });
  });

  describe('error handling', () => {
    it('returns false for completely malformed base64', () => {
      assert.equal(isStubHeader('not-valid-base64!!!'), false);
    });

    it('returns false for valid base64 but non-JSON content', () => {
      const header = Buffer.from('not json content').toString('base64');
      assert.equal(isStubHeader(header), false);
    });

    it('returns false for empty string', () => {
      assert.equal(isStubHeader(''), false);
    });

    it('returns false for base64-encoded null', () => {
      const header = Buffer.from('null').toString('base64');
      assert.equal(isStubHeader(header), false);
    });
  });
});
