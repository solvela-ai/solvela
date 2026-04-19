/**
 * Tests for filterAccepts — payment scheme filtering.
 */

import { describe, it } from 'node:test';
import assert from 'node:assert/strict';

import { filterAccepts } from '../src/scheme-filter.ts';
import type { PaymentAccept } from '../src/types.ts';

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const exactAccept: PaymentAccept = {
  scheme: 'exact',
  network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
  amount: '2500',
  asset: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
  pay_to: '11111111111111111111111111111111',
  max_timeout_seconds: 300,
};

const escrowAccept: PaymentAccept = {
  scheme: 'escrow',
  network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
  amount: '2500',
  asset: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
  pay_to: '11111111111111111111111111111111',
  max_timeout_seconds: 300,
  escrow_program_id: 'EscrowProg1234',
};

const mixedAccepts = [exactAccept, escrowAccept];

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe('filterAccepts', () => {
  describe('auto mode', () => {
    it('returns accepts unchanged in auto mode', () => {
      const result = filterAccepts(mixedAccepts, 'auto');
      assert.strictEqual(result, mixedAccepts);
    });

    it('returns empty accepts unchanged in auto mode (no throw)', () => {
      const result = filterAccepts([], 'auto');
      assert.deepEqual(result, []);
    });
  });

  describe('escrow mode', () => {
    it('filters to only escrow accepts', () => {
      const result = filterAccepts(mixedAccepts, 'escrow');
      assert.equal(result.length, 1);
      assert.equal(result[0].scheme, 'escrow');
    });

    it('returns multiple escrow accepts if present', () => {
      const accepts = [escrowAccept, { ...escrowAccept, amount: '3000' }];
      const result = filterAccepts(accepts, 'escrow');
      assert.equal(result.length, 2);
    });

    it('throws when no escrow accepts available', () => {
      assert.throws(
        () => filterAccepts([exactAccept], 'escrow'),
        /No payment accepts match signing mode 'escrow'/,
      );
    });

    it('error message names what gateway offered', () => {
      try {
        filterAccepts([exactAccept], 'escrow');
        assert.fail('Expected throw');
      } catch (err) {
        assert.ok(err instanceof Error);
        assert.ok(err.message.includes('exact'), `Expected 'exact' in: ${err.message}`);
      }
    });
  });

  describe('direct mode', () => {
    it('filters to only exact accepts', () => {
      const result = filterAccepts(mixedAccepts, 'direct');
      assert.equal(result.length, 1);
      assert.equal(result[0].scheme, 'exact');
    });

    it('throws when no exact accepts available', () => {
      assert.throws(
        () => filterAccepts([escrowAccept], 'direct'),
        /No payment accepts match signing mode 'direct'/,
      );
    });

    it('error message names what gateway offered', () => {
      try {
        filterAccepts([escrowAccept], 'direct');
        assert.fail('Expected throw');
      } catch (err) {
        assert.ok(err instanceof Error);
        assert.ok(err.message.includes('escrow'), `Expected 'escrow' in: ${err.message}`);
      }
    });
  });
});
