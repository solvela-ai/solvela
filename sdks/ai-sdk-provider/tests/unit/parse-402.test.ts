/**
 * Unit-2: parse-402.ts
 *
 * Covers:
 *   - parseGateway402: valid fixture, allowlist enforcement, wrong shape,
 *     missing fields, unknown error.type
 *   - selectAccept: scheme selection priority, zero-match, negative and
 *     non-integer amounts
 */

import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

import { SolvelaPaymentError } from '../../src/errors.js';
import {
  USDC_MINT_MAINNET,
  parseGateway402,
  selectAccept,
} from '../../src/util/parse-402.js';
import type { ParsedPaymentRequired } from '../../src/util/parse-402.js';

// ---------------------------------------------------------------------------
// Fixture helpers
// ---------------------------------------------------------------------------

// ESM-safe fixture loading (no __dirname).
const FIXTURE_PATH = new URL('../fixtures/402-envelope.json', import.meta.url);
const FIXTURE_RAW = JSON.parse(readFileSync(FIXTURE_PATH, 'utf8'));

/**
 * The inner PaymentRequired object that lives inside the fixture's
 * `error.message` JSON string. Parsed once so tests can compare against it.
 */
const FIXTURE_INNER = JSON.parse(FIXTURE_RAW.error.message) as Record<
  string,
  unknown
>;

// ---------------------------------------------------------------------------
// Allowlisted key sets (used in multiple tests)
// ---------------------------------------------------------------------------

const TOP_LEVEL_KEYS = [
  'accepts',
  'cost_breakdown',
  'error',
  'resource',
  'x402_version',
];
const ACCEPT_KEYS = [
  'amount',
  'asset',
  'max_timeout_seconds',
  'network',
  'pay_to',
  'scheme',
];
const COST_BREAKDOWN_KEYS = [
  'currency',
  'fee_percent',
  'platform_fee',
  'provider_cost',
  'total',
];
const RESOURCE_KEYS = ['method', 'url'];

// ---------------------------------------------------------------------------
// Test factories
// ---------------------------------------------------------------------------

/**
 * Build a minimal valid gateway envelope. Callers can override individual
 * fields inside the inner PaymentRequired.
 */
function buildEnvelope(overrideInner: Record<string, unknown> = {}): unknown {
  const inner = {
    x402_version: 2,
    resource: { url: '/v1/chat/completions', method: 'POST' },
    accepts: [
      {
        scheme: 'exact',
        network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
        amount: '2625',
        asset: USDC_MINT_MAINNET,
        pay_to: 'RecipientWalletPubkeyHere',
        max_timeout_seconds: 300,
      },
    ],
    cost_breakdown: {
      provider_cost: '0.002500',
      platform_fee: '0.000125',
      total: '0.002625',
      currency: 'USDC',
      fee_percent: 5,
    },
    error: 'Payment required',
    ...overrideInner,
  };
  return { error: { type: 'invalid_payment', message: JSON.stringify(inner) } };
}

/**
 * Build a minimal `ParsedPaymentRequired` for use in `selectAccept` tests
 * without going through `parseGateway402`.
 */
function buildParsed(
  accepts: Array<{
    scheme: string;
    network: string;
    amount: string;
    asset: string;
    pay_to?: string;
    max_timeout_seconds?: number;
  }>,
): ParsedPaymentRequired {
  return {
    x402_version: 2,
    resource: { url: '/v1/chat/completions', method: 'POST' },
    accepts: accepts.map((a) => ({
      scheme: a.scheme,
      network: a.network,
      amount: a.amount,
      asset: a.asset,
      pay_to: a.pay_to ?? 'RecipientWallet',
      max_timeout_seconds: a.max_timeout_seconds ?? 300,
    })),
    cost_breakdown: {
      provider_cost: '0.002500',
      platform_fee: '0.000125',
      total: '0.002625',
      currency: 'USDC',
      fee_percent: 5,
    },
    error: 'Payment required',
  };
}

// ---------------------------------------------------------------------------
// parseGateway402
// ---------------------------------------------------------------------------

describe('parseGateway402', () => {
  it('returns ParsedPaymentRequired from the fixture verbatim', () => {
    const result = parseGateway402(FIXTURE_RAW);

    // Top-level scalar
    expect(result.x402_version).toBe(FIXTURE_INNER['x402_version']);

    // resource
    const fixtureResource = FIXTURE_INNER['resource'] as {
      url: string;
      method: string;
    };
    expect(result.resource.url).toBe(fixtureResource.url);
    expect(result.resource.method).toBe(fixtureResource.method);

    // accepts[0]
    const fixtureAccept = (
      FIXTURE_INNER['accepts'] as Array<Record<string, unknown>>
    )[0];
    expect(result.accepts).toHaveLength(1);
    expect(result.accepts[0].scheme).toBe(fixtureAccept['scheme']);
    expect(result.accepts[0].network).toBe(fixtureAccept['network']);
    expect(result.accepts[0].amount).toBe(fixtureAccept['amount']);
    expect(result.accepts[0].asset).toBe(fixtureAccept['asset']);
    expect(result.accepts[0].pay_to).toBe(fixtureAccept['pay_to']);
    expect(result.accepts[0].max_timeout_seconds).toBe(
      fixtureAccept['max_timeout_seconds'],
    );

    // cost_breakdown
    const fixtureCb = FIXTURE_INNER['cost_breakdown'] as Record<
      string,
      unknown
    >;
    expect(result.cost_breakdown.provider_cost).toBe(
      fixtureCb['provider_cost'],
    );
    expect(result.cost_breakdown.platform_fee).toBe(fixtureCb['platform_fee']);
    expect(result.cost_breakdown.total).toBe(fixtureCb['total']);
    expect(result.cost_breakdown.currency).toBe(fixtureCb['currency']);
    expect(result.cost_breakdown.fee_percent).toBe(fixtureCb['fee_percent']);

    // error string
    expect(result.error).toBe(FIXTURE_INNER['error']);
  });

  it('strips extra top-level, accept, cost_breakdown and resource fields — only allowlisted keys survive', () => {
    const innerWithExtras = {
      x402_version: 2,
      // extra top-level field
      internal_trace_id: 'trace-abc-123',
      accepts: [
        {
          scheme: 'exact',
          network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
          amount: '2625',
          asset: USDC_MINT_MAINNET,
          pay_to: 'RecipientWallet',
          max_timeout_seconds: 300,
          // extra accept field
          debug_hash: 'sha256-abc',
        },
      ],
      cost_breakdown: {
        provider_cost: '0.002500',
        platform_fee: '0.000125',
        total: '0.002625',
        currency: 'USDC',
        fee_percent: 5,
        // extra cost_breakdown field
        trace_id: 'cb-trace-xyz',
      },
      resource: {
        url: '/v1/chat/completions',
        method: 'POST',
        // extra resource field
        region: 'us-east-1',
      },
      error: 'Payment required',
    };
    const envelope = {
      error: {
        type: 'invalid_payment',
        message: JSON.stringify(innerWithExtras),
      },
    };

    const result = parseGateway402(envelope);

    // Top-level: only allowlisted keys
    expect(Object.keys(result).sort()).toEqual(TOP_LEVEL_KEYS);

    // accepts[0]: only allowlisted keys
    expect(Object.keys(result.accepts[0]).sort()).toEqual(ACCEPT_KEYS);

    // cost_breakdown: only allowlisted keys
    expect(Object.keys(result.cost_breakdown).sort()).toEqual(
      COST_BREAKDOWN_KEYS,
    );

    // resource: only allowlisted keys
    expect(Object.keys(result.resource).sort()).toEqual(RESOURCE_KEYS);

    // Extra fields are absent
    expect(result).not.toHaveProperty('internal_trace_id');
    expect(result.accepts[0]).not.toHaveProperty('debug_hash');
    expect(result.cost_breakdown).not.toHaveProperty('trace_id');
    expect(result.resource).not.toHaveProperty('region');
  });

  it('throws SolvelaPaymentError when body is the direct unwrapped x402 shape (not wrapped in envelope)', () => {
    // The direct { x402_version, accepts, … } shape is NOT supported.
    // The parser checks for { error: { type, message } } and rejects anything else.
    const directShape = {
      x402_version: 2,
      resource: { url: '/v1/chat/completions', method: 'POST' },
      accepts: [
        {
          scheme: 'exact',
          network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
          amount: '2625',
          asset: USDC_MINT_MAINNET,
          pay_to: 'RecipientWallet',
          max_timeout_seconds: 300,
        },
      ],
      cost_breakdown: {
        provider_cost: '0.002500',
        platform_fee: '0.000125',
        total: '0.002625',
        currency: 'USDC',
        fee_percent: 5,
      },
      error: 'Payment required',
    };

    expect(() => parseGateway402(directShape)).toThrow(SolvelaPaymentError);
  });

  it('throws SolvelaPaymentError when error.type is missing', () => {
    const envelope = {
      error: {
        // type is absent
        message: JSON.stringify({
          x402_version: 2,
          resource: { url: '/v1/chat/completions', method: 'POST' },
          accepts: [],
          cost_breakdown: {
            provider_cost: '0.002500',
            platform_fee: '0.000125',
            total: '0.002625',
            currency: 'USDC',
            fee_percent: 5,
          },
          error: 'Payment required',
        }),
      },
    };

    expect(() => parseGateway402(envelope)).toThrow(SolvelaPaymentError);
  });

  it('throws SolvelaPaymentError when error.message is missing', () => {
    const envelope = {
      error: {
        type: 'invalid_payment',
        // message is absent
      },
    };

    expect(() => parseGateway402(envelope)).toThrow(SolvelaPaymentError);
  });

  it('throws SolvelaPaymentError when error.message is not valid JSON — does not echo raw body', () => {
    // The sentinel must not appear in err.message so we can prove no body echo.
    const SENTINEL = 'SOLVELA_SENTINEL_9f3d';
    const envelope = {
      error: {
        type: 'invalid_payment',
        message: `{"${SENTINEL}":"truncated broken json`,
      },
    };

    let caught: unknown;
    try {
      parseGateway402(envelope);
    } catch (err) {
      caught = err;
    }

    expect(SolvelaPaymentError.isInstance(caught)).toBe(true);
    const err = caught as SolvelaPaymentError;

    // responseBody must not be set (parser never passes it to the constructor)
    expect(err.responseBody).toBeUndefined();

    // Sentinel must not appear in the error message
    expect(err.message).not.toContain(SENTINEL);
  });

  it('throws SolvelaPaymentError when error.type is an unknown value', () => {
    const envelope = {
      error: {
        type: 'rate_limited',
        message: '{}',
      },
    };

    expect(() => parseGateway402(envelope)).toThrow(SolvelaPaymentError);
  });
});

// ---------------------------------------------------------------------------
// selectAccept
// ---------------------------------------------------------------------------

describe('selectAccept', () => {
  it('selects the first exact+USDC entry when multiple accepts are present', () => {
    // Three entries: [exact+USDC mainnet, escrow+USDC, exact+USDC different network]
    const parsed = buildParsed([
      {
        scheme: 'exact',
        network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
        amount: '2625',
        asset: USDC_MINT_MAINNET,
      },
      {
        scheme: 'escrow',
        network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
        amount: '9000',
        asset: USDC_MINT_MAINNET,
      },
      {
        scheme: 'exact',
        network: 'solana:devnet-1234',
        amount: '5000',
        asset: USDC_MINT_MAINNET,
      },
    ]);

    const result = selectAccept(parsed);

    // Referential identity to the first entry
    expect(result.accept).toBe(parsed.accepts[0]);

    // cost is a bigint equal to the first entry's amount
    expect(result.cost).toBe(BigInt('2625'));
    expect(typeof result.cost).toBe('bigint');
  });

  it('throws SolvelaPaymentError with "no supported payment scheme" when zero entries match', () => {
    // All entries are escrow — none qualify under v1 rules
    const parsed = buildParsed([
      {
        scheme: 'escrow',
        network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
        amount: '2625',
        asset: USDC_MINT_MAINNET,
      },
      {
        scheme: 'escrow',
        network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
        amount: '3000',
        asset: USDC_MINT_MAINNET,
      },
    ]);

    expect(() => selectAccept(parsed)).toThrow(
      /no supported payment scheme/,
    );
    expect(() => selectAccept(parsed)).toThrow(SolvelaPaymentError);
  });

  it('throws SolvelaPaymentError with "no supported payment scheme" when all entries have a non-USDC asset', () => {
    const NON_USDC = 'So11111111111111111111111111111111111111112'; // wSOL mint

    const parsed = buildParsed([
      {
        scheme: 'exact',
        network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
        amount: '2625',
        asset: NON_USDC,
      },
    ]);

    expect(() => selectAccept(parsed)).toThrow(SolvelaPaymentError);
    expect(() => selectAccept(parsed)).toThrow(
      /no supported payment scheme/,
    );
  });

  it('throws SolvelaPaymentError when the selected accept has a negative amount', () => {
    // "-1000" parses as a bigint but the < 0n guard fires
    const parsed = buildParsed([
      {
        scheme: 'exact',
        network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
        amount: '-1000',
        asset: USDC_MINT_MAINNET,
      },
    ]);

    expect(() => selectAccept(parsed)).toThrow(SolvelaPaymentError);
    expect(() => selectAccept(parsed)).toThrow(/negative/);
  });

  it('throws SolvelaPaymentError when the selected accept amount is a non-integer string', () => {
    // "1.5" causes BigInt("1.5") to throw a SyntaxError internally
    const parsed = buildParsed([
      {
        scheme: 'exact',
        network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
        amount: '1.5',
        asset: USDC_MINT_MAINNET,
      },
    ]);

    expect(() => selectAccept(parsed)).toThrow(SolvelaPaymentError);
  });

  it('throws SolvelaPaymentError when the selected accept amount is a non-numeric string', () => {
    const parsed = buildParsed([
      {
        scheme: 'exact',
        network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
        amount: 'abc',
        asset: USDC_MINT_MAINNET,
      },
    ]);

    expect(() => selectAccept(parsed)).toThrow(SolvelaPaymentError);
  });
});
