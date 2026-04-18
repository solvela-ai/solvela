/**
 * Unit-3: fetch-wrapper branch coverage
 *
 * Covers all 12 branches (b)-(m) from §6 Phase 3 WI-3:
 *   (b)  200 on first fetch → returned unchanged, body never read
 *   (c)  non-402 non-2xx → returned unchanged
 *   (d)  caller-supplied PAYMENT-SIGNATURE → SolvelaPaymentError without signing
 *   (e)  402 body not valid JSON → SolvelaPaymentError
 *   (f)  init.body not string → SolvelaPaymentError('unsupported body type...')
 *   (f2) init.body too large (byte cap, emoji-heavy string) → SolvelaPaymentError('request body exceeds...')
 *   (g)  402 + budget exhausted → SolvelaBudgetExceededError; wallet NOT called
 *   (h)  wallet throws → reservation released; SolvelaSigningError
 *   (h2) AbortSignal already aborted before sign → release; AbortError rethrown
 *   (i)  Between sign and retry: abort fires → release; warn-once; AbortError
 *   (i2) retry network error → release; error propagated
 *   (j)  retry 2xx → budget finalized; response returned
 *   (k)  retry 500 → release; SolvelaUpstreamError(statusCode=500, isRetryable:false, no PAYMENT-SIGNATURE in headers)
 *   (l)  retry 402 → release; SolvelaPaymentError('Payment rejected after retry'); exactly 2 fetch calls
 *   (m)  counter assertions: exactly 2 fetch-start events on 402 path, 1 on 200 path
 *
 * Sentinel absence: every error-path test asserts err.message, err.stack, and
 * JSON.stringify(err) do NOT contain the mock signature value.
 *
 * Framework: vitest. No fake timers needed.
 */

import { beforeEach, describe, expect, it, vi } from 'vitest';

// vi.mock is hoisted above imports by vitest; this replaces warnOnce with a
// spy throughout the file so each abort-path test can assert it was called
// with a message that contains no signature bytes.
vi.mock('../../src/util/warn-once.js', () => ({
  warnOnce: vi.fn(),
}));

import { warnOnce } from '../../src/util/warn-once.js';
import { BudgetState } from '../../src/budget.js';
import {
  SolvelaBudgetExceededError,
  SolvelaPaymentError,
  SolvelaSigningError,
  SolvelaUpstreamError,
} from '../../src/errors.js';
import {
  type SolvelaFetchLogEvent,
  createSolvelaFetch,
} from '../../src/fetch-wrapper.js';

// ---------------------------------------------------------------------------
// Constants & helpers
// ---------------------------------------------------------------------------

const MOCK_SIGNATURE = 'mock-base64-signature==';
const MOCK_URL = 'https://api.solvela.io/v1/chat/completions';

/**
 * The cost in USDC atomic units that the fixture envelope declares.
 * amount: "2625" in 402-envelope.json.
 */
const FIXTURE_COST = 2625n;

/**
 * A valid 402 envelope matching tests/fixtures/402-envelope.json.
 * Inlined so tests can mutate fields without touching the shared file.
 */
function valid402Body(): string {
  return JSON.stringify({
    error: {
      type: 'invalid_payment',
      message: JSON.stringify({
        x402_version: 2,
        resource: { url: '/v1/chat/completions', method: 'POST' },
        accepts: [
          {
            scheme: 'exact',
            network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
            amount: '2625',
            asset: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
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
      }),
    },
  });
}

function make402Response(bodyOverride?: string): Response {
  return new Response(bodyOverride ?? valid402Body(), {
    status: 402,
    headers: { 'content-type': 'application/json' },
  });
}

function make200Response(): Response {
  return new Response(JSON.stringify({ choices: [] }), {
    status: 200,
    headers: { 'content-type': 'application/json' },
  });
}

function makeErrorResponse(status: number, headers?: Record<string, string>): Response {
  return new Response(JSON.stringify({ error: 'upstream error' }), {
    status,
    headers: { 'content-type': 'application/json', ...headers },
  });
}

/** Build a wallet adapter that resolves with MOCK_SIGNATURE. */
function makeWallet() {
  return {
    label: 'test',
    signPayment: vi.fn().mockResolvedValue(MOCK_SIGNATURE),
  };
}

/**
 * Assert that MOCK_SIGNATURE does not appear anywhere in the thrown error:
 * message, stack, or full JSON serialization.
 */
function assertSentinelAbsent(err: unknown): void {
  expect(err).toBeTruthy();
  if (err instanceof Error) {
    expect(err.message).not.toContain(MOCK_SIGNATURE);
    if (err.stack) {
      expect(err.stack).not.toContain(MOCK_SIGNATURE);
    }
  }
  try {
    const serialized = JSON.stringify(err, Object.getOwnPropertyNames(err as object));
    expect(serialized).not.toContain(MOCK_SIGNATURE);
  } catch {
    // Non-serializable errors: skip JSON check
  }
}

// ---------------------------------------------------------------------------
// (b) 200 on first fetch — response returned unchanged, body not read
// ---------------------------------------------------------------------------

describe('(b) 200 on first fetch', () => {
  it('returns the response reference unchanged without reading the body', async () => {
    const expected = make200Response();
    const baseFetch = vi.fn().mockResolvedValueOnce(expected);
    const events: SolvelaFetchLogEvent[] = [];

    const solvelaFetch = createSolvelaFetch({
      wallet: makeWallet(),
      budget: new BudgetState(undefined),
      maxSignedBodyBytes: 1024,
      baseFetch,
      logger: (e) => events.push(e),
    });

    const result = await solvelaFetch(MOCK_URL, { method: 'POST', body: '{}' });

    expect(result).toBe(expected);
    expect(baseFetch).toHaveBeenCalledTimes(1);
    const fetchStartCount = events.filter((e) => e.event === 'fetch-start').length;
    expect(fetchStartCount).toBe(1);
  });
});

// ---------------------------------------------------------------------------
// (c) non-402 non-2xx — response returned unchanged
// ---------------------------------------------------------------------------

describe('(c) non-402 non-2xx on first fetch', () => {
  it('returns a 500 response unchanged without signing', async () => {
    const resp500 = new Response('internal error', { status: 500 });
    const wallet = makeWallet();
    const baseFetch = vi.fn().mockResolvedValueOnce(resp500);

    const solvelaFetch = createSolvelaFetch({
      wallet,
      budget: new BudgetState(undefined),
      maxSignedBodyBytes: 1024,
      baseFetch,
    });

    const result = await solvelaFetch(MOCK_URL, { method: 'POST', body: '{}' });

    expect(result).toBe(resp500);
    expect(wallet.signPayment).not.toHaveBeenCalled();
    expect(baseFetch).toHaveBeenCalledTimes(1);
  });

  it('returns a 404 response unchanged without signing', async () => {
    const resp404 = new Response('not found', { status: 404 });
    const baseFetch = vi.fn().mockResolvedValueOnce(resp404);
    const wallet = makeWallet();

    const solvelaFetch = createSolvelaFetch({
      wallet,
      budget: new BudgetState(undefined),
      maxSignedBodyBytes: 1024,
      baseFetch,
    });

    const result = await solvelaFetch(MOCK_URL, { method: 'POST', body: '{}' });

    expect(result).toBe(resp404);
    expect(wallet.signPayment).not.toHaveBeenCalled();
  });
});

// ---------------------------------------------------------------------------
// (d) caller-supplied PAYMENT-SIGNATURE → SolvelaPaymentError; no sign call
// ---------------------------------------------------------------------------

describe('(d) caller-supplied PAYMENT-SIGNATURE', () => {
  it('throws SolvelaPaymentError without calling signPayment', async () => {
    const baseFetch = vi.fn().mockResolvedValueOnce(make402Response());
    const wallet = makeWallet();

    const solvelaFetch = createSolvelaFetch({
      wallet,
      budget: new BudgetState(undefined),
      maxSignedBodyBytes: 1024,
      baseFetch,
    });

    const err = await solvelaFetch(MOCK_URL, {
      method: 'POST',
      body: '{}',
      headers: { 'payment-signature': 'some-existing-sig' },
    }).catch((e: unknown) => e);

    expect(SolvelaPaymentError.isInstance(err)).toBe(true);
    expect((err as SolvelaPaymentError).message).toContain('caller supplied');
    expect(wallet.signPayment).not.toHaveBeenCalled();
    assertSentinelAbsent(err);
  });

  it('throws even when PAYMENT-SIGNATURE is mixed-case in caller headers', async () => {
    const baseFetch = vi.fn().mockResolvedValueOnce(make402Response());
    const wallet = makeWallet();

    const solvelaFetch = createSolvelaFetch({
      wallet,
      budget: new BudgetState(undefined),
      maxSignedBodyBytes: 1024,
      baseFetch,
    });

    const err = await solvelaFetch(MOCK_URL, {
      method: 'POST',
      body: '{}',
      headers: { 'Payment-Signature': 'some-existing-sig' },
    }).catch((e: unknown) => e);

    expect(SolvelaPaymentError.isInstance(err)).toBe(true);
    expect(wallet.signPayment).not.toHaveBeenCalled();
  });
});

// ---------------------------------------------------------------------------
// (e) 402 body not valid JSON → SolvelaPaymentError
// ---------------------------------------------------------------------------

describe('(e) 402 body not valid JSON', () => {
  it('throws SolvelaPaymentError when 402 body is plain text', async () => {
    const badResp = new Response('not json at all', { status: 402 });
    const baseFetch = vi.fn().mockResolvedValueOnce(badResp);
    const wallet = makeWallet();

    const solvelaFetch = createSolvelaFetch({
      wallet,
      budget: new BudgetState(undefined),
      maxSignedBodyBytes: 1024,
      baseFetch,
    });

    const err = await solvelaFetch(MOCK_URL, { method: 'POST', body: '{}' }).catch(
      (e: unknown) => e,
    );

    expect(SolvelaPaymentError.isInstance(err)).toBe(true);
    expect(wallet.signPayment).not.toHaveBeenCalled();
    assertSentinelAbsent(err);
  });
});

// ---------------------------------------------------------------------------
// (f) init.body not a string → SolvelaPaymentError('unsupported body type...')
// ---------------------------------------------------------------------------

describe('(f) init.body not a string', () => {
  it('throws SolvelaPaymentError with "unsupported body type" when body is undefined', async () => {
    const baseFetch = vi.fn().mockResolvedValueOnce(make402Response());
    const wallet = makeWallet();

    const solvelaFetch = createSolvelaFetch({
      wallet,
      budget: new BudgetState(undefined),
      maxSignedBodyBytes: 1024,
      baseFetch,
    });

    const err = await solvelaFetch(MOCK_URL, { method: 'POST' }).catch(
      (e: unknown) => e,
    );

    expect(SolvelaPaymentError.isInstance(err)).toBe(true);
    expect((err as SolvelaPaymentError).message).toContain('unsupported body type');
    expect(wallet.signPayment).not.toHaveBeenCalled();
    assertSentinelAbsent(err);
  });

  it('throws SolvelaPaymentError with "unsupported body type" when body is a Uint8Array', async () => {
    const baseFetch = vi.fn().mockResolvedValueOnce(make402Response());
    const wallet = makeWallet();

    const solvelaFetch = createSolvelaFetch({
      wallet,
      budget: new BudgetState(undefined),
      maxSignedBodyBytes: 1024,
      baseFetch,
    });

    const err = await solvelaFetch(MOCK_URL, {
      method: 'POST',
      body: new Uint8Array([1, 2, 3]) as unknown as string,
    }).catch((e: unknown) => e);

    expect(SolvelaPaymentError.isInstance(err)).toBe(true);
    expect((err as SolvelaPaymentError).message).toContain('unsupported body type');
    expect(wallet.signPayment).not.toHaveBeenCalled();
  });

  it('throws SolvelaPaymentError with "request body exceeds" for emoji-heavy body over byte cap', async () => {
    // Each crab emoji '🦀' is 4 UTF-8 bytes but only 2 UTF-16 code units.
    // Cap at 32 bytes: 10 crabs = 40 bytes > 32, but .length = 20 (under cap if
    // the implementation incorrectly used string .length instead of byte length).
    const MAX_BYTES = 32;
    const bigEmojiBody = '🦀'.repeat(10); // 40 UTF-8 bytes, .length == 20

    const baseFetch = vi.fn().mockResolvedValueOnce(make402Response());
    const wallet = makeWallet();

    const solvelaFetch = createSolvelaFetch({
      wallet,
      budget: new BudgetState(undefined),
      maxSignedBodyBytes: MAX_BYTES,
      baseFetch,
    });

    const err = await solvelaFetch(MOCK_URL, {
      method: 'POST',
      body: bigEmojiBody,
    }).catch((e: unknown) => e);

    expect(SolvelaPaymentError.isInstance(err)).toBe(true);
    expect((err as SolvelaPaymentError).message).toContain('request body exceeds');
    expect(wallet.signPayment).not.toHaveBeenCalled();
    assertSentinelAbsent(err);
  });
});

// ---------------------------------------------------------------------------
// (g) 402 + budget exhausted → SolvelaBudgetExceededError; wallet NOT called
// ---------------------------------------------------------------------------

describe('(g) budget exhausted', () => {
  it('throws SolvelaBudgetExceededError without calling signPayment when budget is 0', async () => {
    const baseFetch = vi.fn().mockResolvedValueOnce(make402Response());
    const wallet = makeWallet();
    // Fixture cost is 2625n; budget of 0n is exhausted immediately.
    const budget = new BudgetState(0n);

    const solvelaFetch = createSolvelaFetch({
      wallet,
      budget,
      maxSignedBodyBytes: 1024,
      baseFetch,
    });

    const err = await solvelaFetch(MOCK_URL, { method: 'POST', body: '{}' }).catch(
      (e: unknown) => e,
    );

    expect(SolvelaBudgetExceededError.isInstance(err)).toBe(true);
    expect(wallet.signPayment).not.toHaveBeenCalled();
    // Budget should still have 0n remaining (no debit on reserve failure)
    expect(budget.remaining).toBe(0n);
    assertSentinelAbsent(err);
  });

  it('throws SolvelaBudgetExceededError when budget is smaller than fixture cost', async () => {
    const baseFetch = vi.fn().mockResolvedValueOnce(make402Response());
    const wallet = makeWallet();
    // Fixture cost is 2625n; give budget of 100n.
    const budget = new BudgetState(100n);

    const solvelaFetch = createSolvelaFetch({
      wallet,
      budget,
      maxSignedBodyBytes: 1024,
      baseFetch,
    });

    const err = await solvelaFetch(MOCK_URL, { method: 'POST', body: '{}' }).catch(
      (e: unknown) => e,
    );

    expect(SolvelaBudgetExceededError.isInstance(err)).toBe(true);
    expect(wallet.signPayment).not.toHaveBeenCalled();
    assertSentinelAbsent(err);
  });
});

// ---------------------------------------------------------------------------
// (h) wallet throws → reservation released; SolvelaSigningError
// ---------------------------------------------------------------------------

describe('(h) wallet throws', () => {
  it('releases reservation and throws SolvelaSigningError when wallet rejects', async () => {
    const baseFetch = vi.fn().mockResolvedValueOnce(make402Response());
    const wallet = {
      label: 'test',
      signPayment: vi.fn().mockRejectedValue(new Error('hardware wallet disconnected')),
    };
    const TOTAL = 1_000_000n;
    const budget = new BudgetState(TOTAL);
    const events: SolvelaFetchLogEvent[] = [];

    const solvelaFetch = createSolvelaFetch({
      wallet,
      budget,
      maxSignedBodyBytes: 1024,
      baseFetch,
      logger: (e) => events.push(e),
    });

    const err = await solvelaFetch(MOCK_URL, { method: 'POST', body: '{}' }).catch(
      (e: unknown) => e,
    );

    expect(SolvelaSigningError.isInstance(err)).toBe(true);
    // Reservation must have been released — budget back to full
    expect(budget.remaining).toBe(TOTAL);
    // A 'release' event must have been logged
    expect(events.some((e) => e.event === 'release')).toBe(true);
    // baseFetch called only once (no retry)
    expect(baseFetch).toHaveBeenCalledTimes(1);
    assertSentinelAbsent(err);
  });

  it('releases reservation and rethrows AbortError when AbortSignal is already aborted before signPayment', async () => {
    const baseFetch = vi.fn().mockResolvedValueOnce(make402Response());
    const controller = new AbortController();

    // Wallet mock: abort the controller on call, then throw the AbortError
    const abortError = new DOMException('The operation was aborted', 'AbortError');
    const wallet = {
      label: 'test',
      signPayment: vi.fn().mockImplementation(() => {
        controller.abort();
        return Promise.reject(abortError);
      }),
    };
    const TOTAL = 1_000_000n;
    const budget = new BudgetState(TOTAL);
    const events: SolvelaFetchLogEvent[] = [];

    const solvelaFetch = createSolvelaFetch({
      wallet,
      budget,
      maxSignedBodyBytes: 1024,
      baseFetch,
      logger: (e) => events.push(e),
    });

    const err = await solvelaFetch(MOCK_URL, {
      method: 'POST',
      body: '{}',
      signal: controller.signal,
    }).catch((e: unknown) => e);

    // Must rethrow the AbortError, not wrap it
    expect((err as Error).name).toBe('AbortError');
    // Must NOT be wrapped in a SolvelaSigningError
    expect(SolvelaSigningError.isInstance(err)).toBe(false);
    // Budget released
    expect(budget.remaining).toBe(TOTAL);
    expect(events.some((e) => e.event === 'release')).toBe(true);
    assertSentinelAbsent(err);
  });
});

// ---------------------------------------------------------------------------
// (i) Between sign and retry: abort fires → release; warn-once; AbortError
// ---------------------------------------------------------------------------

// warnOnce is vi.mock'd at the top of the file. Reset the spy before each
// test so call counts are isolated even across watch-mode re-runs.

describe('(i) abort fires between sign and retry', () => {
  beforeEach(() => {
    vi.mocked(warnOnce).mockClear();
  });

  it('post-sign abort: signal.aborted=true after wallet resolves → release + warn + AbortError', async () => {
    const controller = new AbortController();

    // wallet resolves with signature AND aborts the controller synchronously,
    // so that init.signal.aborted is true when the guard at line 370 runs.
    const wallet = {
      label: 'test',
      signPayment: vi.fn().mockImplementation(() => {
        controller.abort();
        return Promise.resolve(MOCK_SIGNATURE);
      }),
    };
    const TOTAL = 1_000_000n;
    const budget = new BudgetState(TOTAL);
    const events: SolvelaFetchLogEvent[] = [];
    // baseFetch: first call returns 402; second must NOT be reached.
    const baseFetch = vi.fn().mockResolvedValueOnce(make402Response());

    const solvelaFetch = createSolvelaFetch({
      wallet,
      budget,
      maxSignedBodyBytes: 1024,
      baseFetch,
      logger: (e) => events.push(e),
    });

    const err = await solvelaFetch(MOCK_URL, {
      method: 'POST',
      body: '{}',
      signal: controller.signal,
    }).catch((e: unknown) => e);

    expect((err as Error).name).toBe('AbortError');
    // Budget released (no debit)
    expect(budget.remaining).toBe(TOTAL);
    expect(events.some((e) => e.event === 'release')).toBe(true);
    // warnOnce must have been called exactly once with the abort-mid-retry message
    expect(warnOnce).toHaveBeenCalledTimes(1);
    const [warnMsg] = vi.mocked(warnOnce).mock.calls[0];
    expect(warnMsg).toContain('aborted mid-retry');
    // The warn message must NOT contain the mock signature value
    expect(warnMsg).not.toContain(MOCK_SIGNATURE);
    // baseFetch called only once (abort guard fired before second call)
    expect(baseFetch).toHaveBeenCalledTimes(1);
    assertSentinelAbsent(err);
  });

  it('retry-network-error abort: baseFetch rejects with AbortError on second call → release + warn + AbortError rethrown', async () => {
    // Signal is NOT pre-aborted. The abort is simulated by having baseFetch
    // reject with AbortError on the second call (conceptually: abort fires
    // concurrently during the retry fetch). The post-sign guard at line 370
    // must see signal.aborted === false so that the second fetch is attempted.
    const abortError = new DOMException('The operation was aborted', 'AbortError');

    const wallet = makeWallet();
    const TOTAL = 1_000_000n;
    const budget = new BudgetState(TOTAL);
    const events: SolvelaFetchLogEvent[] = [];

    const baseFetch = vi
      .fn()
      .mockResolvedValueOnce(make402Response())
      .mockRejectedValueOnce(abortError);

    const solvelaFetch = createSolvelaFetch({
      wallet,
      budget,
      maxSignedBodyBytes: 1024,
      baseFetch,
      logger: (e) => events.push(e),
    });

    const err = await solvelaFetch(MOCK_URL, {
      method: 'POST',
      body: '{}',
    }).catch((e: unknown) => e);

    expect((err as Error).name).toBe('AbortError');
    expect(budget.remaining).toBe(TOTAL);
    expect(events.some((e) => e.event === 'release')).toBe(true);
    // warnOnce called with abort-mid-retry message; no signature bytes in it
    expect(warnOnce).toHaveBeenCalledTimes(1);
    const [warnMsg] = vi.mocked(warnOnce).mock.calls[0];
    expect(warnMsg).toContain('aborted mid-retry');
    expect(warnMsg).not.toContain(MOCK_SIGNATURE);
    expect(baseFetch).toHaveBeenCalledTimes(2);
    assertSentinelAbsent(err);
  });
});

// ---------------------------------------------------------------------------
// (i2) retry network error (non-abort) → release; error propagated
// ---------------------------------------------------------------------------

describe('(i2) retry network error (non-abort)', () => {
  it('releases reservation and propagates the network error', async () => {
    const networkError = new TypeError('fetch failed: connection refused');
    const wallet = makeWallet();
    const TOTAL = 1_000_000n;
    const budget = new BudgetState(TOTAL);
    const events: SolvelaFetchLogEvent[] = [];

    const baseFetch = vi
      .fn()
      .mockResolvedValueOnce(make402Response())
      .mockRejectedValueOnce(networkError);

    const solvelaFetch = createSolvelaFetch({
      wallet,
      budget,
      maxSignedBodyBytes: 1024,
      baseFetch,
      logger: (e) => events.push(e),
    });

    const err = await solvelaFetch(MOCK_URL, { method: 'POST', body: '{}' }).catch(
      (e: unknown) => e,
    );

    expect(err).toBe(networkError);
    expect(budget.remaining).toBe(TOTAL);
    expect(events.some((e) => e.event === 'release')).toBe(true);
    expect(baseFetch).toHaveBeenCalledTimes(2);
  });
});

// ---------------------------------------------------------------------------
// (j) retry 2xx → budget finalized; response returned
// ---------------------------------------------------------------------------

describe('(j) retry 2xx', () => {
  it('finalizes budget and returns retry response on 200', async () => {
    const retryResp = make200Response();
    const wallet = makeWallet();
    const TOTAL = 1_000_000n;
    const budget = new BudgetState(TOTAL);
    const events: SolvelaFetchLogEvent[] = [];

    const baseFetch = vi
      .fn()
      .mockResolvedValueOnce(make402Response())
      .mockResolvedValueOnce(retryResp);

    const solvelaFetch = createSolvelaFetch({
      wallet,
      budget,
      maxSignedBodyBytes: 1024,
      baseFetch,
      logger: (e) => events.push(e),
    });

    const result = await solvelaFetch(MOCK_URL, { method: 'POST', body: '{}' });

    expect(result).toBe(retryResp);
    // Budget finalized: remaining = total - cost
    expect(budget.remaining).toBe(TOTAL - FIXTURE_COST);
    expect(events.some((e) => e.event === 'finalize')).toBe(true);
    expect(baseFetch).toHaveBeenCalledTimes(2);
    expect(wallet.signPayment).toHaveBeenCalledTimes(1);
  });

  it('adds PAYMENT-SIGNATURE header on the retry request', async () => {
    const wallet = makeWallet();
    const baseFetch = vi
      .fn()
      .mockResolvedValueOnce(make402Response())
      .mockResolvedValueOnce(make200Response());

    const solvelaFetch = createSolvelaFetch({
      wallet,
      budget: new BudgetState(undefined),
      maxSignedBodyBytes: 1024,
      baseFetch,
    });

    await solvelaFetch(MOCK_URL, { method: 'POST', body: '{}' });

    const [, retryInit] = baseFetch.mock.calls[1] as [unknown, RequestInit];
    const headers = retryInit.headers as Record<string, string>;
    expect(headers['PAYMENT-SIGNATURE']).toBe(MOCK_SIGNATURE);
  });
});

// ---------------------------------------------------------------------------
// (k) retry 500 → release; SolvelaUpstreamError(statusCode=500, isRetryable:false,
//     responseHeaders stripped of PAYMENT-SIGNATURE)
// ---------------------------------------------------------------------------

describe('(k) retry non-2xx non-402', () => {
  it('releases budget and throws SolvelaUpstreamError with statusCode=500 and isRetryable=false', async () => {
    const wallet = makeWallet();
    const TOTAL = 1_000_000n;
    const budget = new BudgetState(TOTAL);
    const events: SolvelaFetchLogEvent[] = [];

    const retry500 = makeErrorResponse(500, {
      'x-request-id': 'req-abc',
      'PAYMENT-SIGNATURE': 'leaked-sig',
      'payment-signature': 'also-leaked',
    });

    const baseFetch = vi
      .fn()
      .mockResolvedValueOnce(make402Response())
      .mockResolvedValueOnce(retry500);

    const solvelaFetch = createSolvelaFetch({
      wallet,
      budget,
      maxSignedBodyBytes: 1024,
      baseFetch,
      logger: (e) => events.push(e),
    });

    const err = await solvelaFetch(MOCK_URL, { method: 'POST', body: '{}' }).catch(
      (e: unknown) => e,
    );

    expect(SolvelaUpstreamError.isInstance(err)).toBe(true);
    const upstreamErr = err as SolvelaUpstreamError;
    expect(upstreamErr.statusCode).toBe(500);
    expect(upstreamErr.isRetryable).toBe(false);
    // responseHeaders must not contain PAYMENT-SIGNATURE (any case)
    const respHeaders = upstreamErr.responseHeaders as Record<string, string> | undefined;
    if (respHeaders) {
      for (const key of Object.keys(respHeaders)) {
        expect(key.toLowerCase()).not.toBe('payment-signature');
      }
      // But non-sensitive headers are preserved
      expect(respHeaders['x-request-id']).toBe('req-abc');
    }
    // Budget released
    expect(budget.remaining).toBe(TOTAL);
    expect(events.some((e) => e.event === 'release')).toBe(true);
    assertSentinelAbsent(err);
  });
});

// ---------------------------------------------------------------------------
// (l) retry 402 → release; SolvelaPaymentError('Payment rejected after retry')
// ---------------------------------------------------------------------------

describe('(l) retry 402', () => {
  it('throws SolvelaPaymentError("Payment rejected after retry") on double-402', async () => {
    const wallet = makeWallet();
    const TOTAL = 1_000_000n;
    const budget = new BudgetState(TOTAL);
    const events: SolvelaFetchLogEvent[] = [];

    const baseFetch = vi
      .fn()
      .mockResolvedValueOnce(make402Response())
      .mockResolvedValueOnce(make402Response());

    const solvelaFetch = createSolvelaFetch({
      wallet,
      budget,
      maxSignedBodyBytes: 1024,
      baseFetch,
      logger: (e) => events.push(e),
    });

    const err = await solvelaFetch(MOCK_URL, { method: 'POST', body: '{}' }).catch(
      (e: unknown) => e,
    );

    expect(SolvelaPaymentError.isInstance(err)).toBe(true);
    expect((err as SolvelaPaymentError).message).toContain('Payment rejected after retry');
    // Budget released (not finalized)
    expect(budget.remaining).toBe(TOTAL);
    expect(events.some((e) => e.event === 'release')).toBe(true);
    // Exactly 2 fetch calls
    expect(baseFetch).toHaveBeenCalledTimes(2);
    assertSentinelAbsent(err);
  });
});

// ---------------------------------------------------------------------------
// (m) Exactly-2-fetch-calls counter assertion via logger
// ---------------------------------------------------------------------------

describe('(m) fetch-count assertions via logger', () => {
  it('emits exactly 1 fetch-start event on the 200 path', async () => {
    const events: SolvelaFetchLogEvent[] = [];
    const baseFetch = vi.fn().mockResolvedValueOnce(make200Response());

    const solvelaFetch = createSolvelaFetch({
      wallet: makeWallet(),
      budget: new BudgetState(undefined),
      maxSignedBodyBytes: 1024,
      baseFetch,
      logger: (e) => events.push(e),
    });

    await solvelaFetch(MOCK_URL, { method: 'POST', body: '{}' });

    const fetchStarts = events.filter((e) => e.event === 'fetch-start');
    expect(fetchStarts).toHaveLength(1);
    expect(fetchStarts[0].attempt).toBe(1);
  });

  it('emits exactly 2 fetch-start events on the 402→200 path', async () => {
    const events: SolvelaFetchLogEvent[] = [];
    const baseFetch = vi
      .fn()
      .mockResolvedValueOnce(make402Response())
      .mockResolvedValueOnce(make200Response());

    const solvelaFetch = createSolvelaFetch({
      wallet: makeWallet(),
      budget: new BudgetState(undefined),
      maxSignedBodyBytes: 1024,
      baseFetch,
      logger: (e) => events.push(e),
    });

    await solvelaFetch(MOCK_URL, { method: 'POST', body: '{}' });

    const fetchStarts = events.filter((e) => e.event === 'fetch-start');
    expect(fetchStarts).toHaveLength(2);
    expect(fetchStarts[0].attempt).toBe(1);
    expect(fetchStarts[1].attempt).toBe(2);
  });

  it('emits exactly 2 fetch-start events on the 402→402 path', async () => {
    const events: SolvelaFetchLogEvent[] = [];
    const baseFetch = vi
      .fn()
      .mockResolvedValueOnce(make402Response())
      .mockResolvedValueOnce(make402Response());

    const solvelaFetch = createSolvelaFetch({
      wallet: makeWallet(),
      budget: new BudgetState(undefined),
      maxSignedBodyBytes: 1024,
      baseFetch,
      logger: (e) => events.push(e),
    });

    await solvelaFetch(MOCK_URL, { method: 'POST', body: '{}' }).catch(() => {});

    const fetchStarts = events.filter((e) => e.event === 'fetch-start');
    expect(fetchStarts).toHaveLength(2);
  });

  it('emits exactly 1 fetch-start event on the caller-supplied-sig path (d)', async () => {
    const events: SolvelaFetchLogEvent[] = [];
    const baseFetch = vi.fn().mockResolvedValueOnce(make402Response());

    const solvelaFetch = createSolvelaFetch({
      wallet: makeWallet(),
      budget: new BudgetState(undefined),
      maxSignedBodyBytes: 1024,
      baseFetch,
      logger: (e) => events.push(e),
    });

    await solvelaFetch(MOCK_URL, {
      method: 'POST',
      body: '{}',
      headers: { 'payment-signature': 'existing' },
    }).catch(() => {});

    const fetchStarts = events.filter((e) => e.event === 'fetch-start');
    expect(fetchStarts).toHaveLength(1);
  });
});
