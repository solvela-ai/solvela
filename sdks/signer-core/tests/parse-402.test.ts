/**
 * Tests for parse402 — unified 402 gateway envelope parser.
 */

import { describe, it } from 'node:test';
import assert from 'node:assert/strict';

import { parse402 } from '../src/parse-402.ts';

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const validDirectBody = {
  x402_version: 2,
  accepts: [
    {
      scheme: 'exact',
      network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
      amount: '2500',
      asset: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
      pay_to: '11111111111111111111111111111111',
      max_timeout_seconds: 300,
    },
  ],
  cost_breakdown: {
    provider_cost: '0.002375',
    platform_fee: '0.000125',
    total: '0.002500',
    currency: 'USDC',
    fee_percent: 5,
  },
  error: 'Payment required',
};

const validEnvelopeBody = {
  error: { message: JSON.stringify(validDirectBody) },
};

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe('parse402', () => {
  describe('direct shape', () => {
    it('parses a valid direct-shape body', () => {
      const result = parse402(JSON.stringify(validDirectBody));
      assert.equal(result.x402_version, 2);
      assert.equal(result.accepts.length, 1);
      assert.equal(result.accepts[0].scheme, 'exact');
      assert.equal(result.cost_breakdown.total, '0.002500');
    });

    it('preserves all fields in the direct shape', () => {
      const result = parse402(JSON.stringify(validDirectBody));
      assert.equal(result.error, 'Payment required');
      assert.equal(result.accepts[0].network, 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp');
      assert.equal(result.accepts[0].pay_to, '11111111111111111111111111111111');
    });
  });

  describe('envelope shape', () => {
    it('parses a valid envelope-shape body', () => {
      const result = parse402(JSON.stringify(validEnvelopeBody));
      assert.equal(result.x402_version, 2);
      assert.equal(result.accepts.length, 1);
      assert.equal(result.cost_breakdown.total, '0.002500');
    });

    it('throws on invalid inner JSON in error.message', () => {
      const body = { error: { message: '{"broken json' } };
      assert.throws(
        () => parse402(JSON.stringify(body)),
        /error\.message is not valid JSON/,
      );
    });
  });

  describe('error cases', () => {
    it('throws on non-JSON body', () => {
      assert.throws(
        () => parse402('not json at all'),
        /non-JSON body/,
      );
    });

    // Issue #129: validation messages now name the failing JSON path
    // (e.g. `x402_version`, `accepts[1].scheme`, `cost_breakdown.total`),
    // emitted by the valibot schema in `src/schema.ts`. Tests assert on
    // the path token rather than the legacy hand-rolled wording so
    // valibot's natural message format flows through unchanged.
    it('throws on missing x402_version', () => {
      const body = {
        accepts: [validDirectBody.accepts[0]],
        cost_breakdown: validDirectBody.cost_breakdown,
        error: 'Payment required',
      };
      assert.throws(
        () => parse402(JSON.stringify(body)),
        /x402_version/,
      );
    });

    it('throws on missing accepts', () => {
      const body = {
        x402_version: 2,
        cost_breakdown: validDirectBody.cost_breakdown,
        error: 'Payment required',
      };
      assert.throws(
        () => parse402(JSON.stringify(body)),
        /accepts/,
      );
    });

    it('throws on empty accepts array', () => {
      const body = { ...validDirectBody, accepts: [] };
      assert.throws(
        () => parse402(JSON.stringify(body)),
        /accepts must be non-empty/,
      );
    });

    // Per-element validation: without this, accepts: [null] / [{}] would
    // parse cleanly and then crash downstream in scheme-filter.ts when
    // a.scheme is dereferenced on a non-object.
    it('throws on accepts element that is not a JSON object', () => {
      for (const bad of [null, 'exact', 42, true, []]) {
        const body = { ...validDirectBody, accepts: [bad] };
        assert.throws(
          () => parse402(JSON.stringify(body)),
          /accepts\[0\]/,
          `expected ${JSON.stringify(bad)} to be rejected`,
        );
      }
    });

    it('throws on accepts element missing or non-string scheme field', () => {
      for (const bad of [{}, { scheme: 42 }, { scheme: null }, { network: 'solana' }]) {
        const body = { ...validDirectBody, accepts: [bad] };
        assert.throws(
          () => parse402(JSON.stringify(body)),
          /accepts\[0\]/,
          `expected ${JSON.stringify(bad)} to be rejected`,
        );
      }
    });

    it('reports the index of the first invalid accepts element', () => {
      const body = {
        ...validDirectBody,
        accepts: [validDirectBody.accepts[0], null, validDirectBody.accepts[0]],
      };
      assert.throws(
        () => parse402(JSON.stringify(body)),
        /accepts\[1\]/,
      );
    });

    it('throws on non-finite cost_breakdown.total', () => {
      const body = {
        ...validDirectBody,
        cost_breakdown: { ...validDirectBody.cost_breakdown, total: 'NaN' },
      };
      assert.throws(
        () => parse402(JSON.stringify(body)),
        /cost_breakdown\.total/,
      );
    });

    it('throws on negative cost_breakdown.total', () => {
      const body = {
        ...validDirectBody,
        cost_breakdown: { ...validDirectBody.cost_breakdown, total: '-1.0' },
      };
      assert.throws(
        () => parse402(JSON.stringify(body)),
        /cost_breakdown\.total/,
      );
    });

    // Number() is stricter than parseFloat for trailing garbage:
    //   parseFloat("1.5USDC") → 1.5  (silently strips the suffix — bug)
    //   Number("1.5USDC")    → NaN   (correctly rejects)
    // The `decimalAmount` schema preserves this stricter check.
    it('throws on cost_breakdown.total with trailing currency suffix', () => {
      for (const total of ['1.5USDC', '0.001SOL', '0.5 USDC', '1.5,']) {
        const body = {
          ...validDirectBody,
          cost_breakdown: { ...validDirectBody.cost_breakdown, total },
        };
        assert.throws(
          () => parse402(JSON.stringify(body)),
          /cost_breakdown\.total/,
          `expected "${total}" to be rejected`,
        );
      }
    });

    // Number() coerces empty strings, null, booleans, and arrays to 0 / 1
    // (unlike parseFloat which yields NaN). The schema's `string()` step
    // means those non-string inputs never reach the numeric check.
    // Numeric-typed totals are also rejected — the wire format keeps
    // USDC amounts as strings to avoid float precision drift.
    it('throws on cost_breakdown.total that is empty or non-string', () => {
      const malformed: unknown[] = ['', null, true, false, [], 0.001];
      for (const total of malformed) {
        const body = {
          ...validDirectBody,
          cost_breakdown: { ...validDirectBody.cost_breakdown, total },
        };
        assert.throws(
          () => parse402(JSON.stringify(body)),
          /cost_breakdown\.total/,
          `expected ${JSON.stringify(total)} to be rejected`,
        );
      }
    });

    it('throws on non-object body (array)', () => {
      assert.throws(
        () => parse402('[]'),
        /is not a JSON object/,
      );
    });

    // Issue #129: the schema now validates every accept field, not just
    // `scheme`. Pre-schema parsers would have happily passed an accept
    // with missing/empty `pay_to`, `network`, or `asset` — the signer
    // would then build a tx against a malformed wire payload. Lock the
    // new per-field coverage so future schema regressions are loud.
    it('throws when accept fields beyond scheme are malformed', () => {
      const baseAccept = validDirectBody.accepts[0];
      const cases: Array<{ label: string; mut: Record<string, unknown>; path: RegExp }> = [
        { label: 'missing network', mut: { network: undefined }, path: /accepts\[0\]\.network/ },
        { label: 'empty pay_to', mut: { pay_to: '' }, path: /accepts\[0\]\.pay_to/ },
        { label: 'numeric asset', mut: { asset: 42 }, path: /accepts\[0\]\.asset/ },
        {
          label: 'string max_timeout_seconds',
          mut: { max_timeout_seconds: '300' },
          path: /accepts\[0\]\.max_timeout_seconds/,
        },
        {
          label: 'amount with trailing junk',
          mut: { amount: '2500abc' },
          path: /accepts\[0\]\.amount/,
        },
      ];
      for (const { label, mut, path } of cases) {
        const body = { ...validDirectBody, accepts: [{ ...baseAccept, ...mut }] };
        assert.throws(
          () => parse402(JSON.stringify(body)),
          path,
          `expected ${label} to be rejected at ${path}`,
        );
      }
    });
  });

  describe('envelope with escrow scheme', () => {
    it('parses envelope with escrow accept scheme', () => {
      const body = {
        ...validDirectBody,
        accepts: [
          { ...validDirectBody.accepts[0], scheme: 'escrow', escrow_program_id: 'EscrowProg1234' },
        ],
      };
      const result = parse402(JSON.stringify({ error: { message: JSON.stringify(body) } }));
      assert.equal(result.accepts[0].scheme, 'escrow');
    });
  });
});
