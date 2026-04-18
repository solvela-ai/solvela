/**
 * Unit-6 — BudgetState and solvelaFetch concurrency tests (§6 Phase 7)
 *
 * Tests:
 *   BudgetState direct:
 *     1. disabled (undefined total) — reserve always succeeds, remaining undefined, isDisabled true
 *     2. enabled (100n) — reserve 60n ok, reserve 50n throws SolvelaBudgetExceededError, finalize → 40n remaining
 *     3. release without finalize restores full capacity
 *     4. negative cost throws SolvelaBudgetExceededError
 *     5. two synchronous reserves (60n + 50n) against 100n budget — exactly one succeeds, one throws
 *
 *   fetch-wrapper concurrency:
 *     6. 50 concurrent solvelaFetch calls, budget affords exactly 25 — exactly 25 succeed,
 *        25 throw SolvelaBudgetExceededError, signPayment called exactly 25 times
 *     7. abort mid-retry (post-sign guard) releases reservation; sequential follow-up call
 *        sees pre-abort budget minus debited successfuls
 */

import { describe, it, expect, vi } from 'vitest';

import { BudgetState } from '../../src/budget.js';
import { createSolvelaFetch } from '../../src/fetch-wrapper.js';
import { SolvelaBudgetExceededError } from '../../src/errors.js';
import type { SolvelaWalletAdapter } from '../../src/wallet-adapter.js';
import type { CreateSolvelaFetchOptions } from '../../src/fetch-wrapper.js';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const FIXTURE_ENVELOPE = JSON.stringify({
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

/** Amount per request derived from the fixture: "2625" atomic USDC units. */
const COST_PER_REQUEST = 2625n;

/**
 * Build a mock baseFetch that returns 402 on the first call (no payment
 * signature present) and 200 on the retry (payment signature present).
 */
function makeMockFetch(): typeof globalThis.fetch {
  return async (input: RequestInfo | URL, init?: RequestInit): Promise<Response> => {
    const headers = init?.headers;
    const hasSig = hasPaymentSignatureHeader(headers);
    if (hasSig) {
      // Retry path — return 200 OK (body not read by wrapper on 200 path)
      return new Response('{}', {
        status: 200,
        headers: { 'content-type': 'application/json' },
      });
    }
    // First call — return 402 with gateway envelope
    return new Response(FIXTURE_ENVELOPE, {
      status: 402,
      headers: { 'content-type': 'application/json' },
    });
  };
}

/** Case-insensitive check for `PAYMENT-SIGNATURE` header. */
function hasPaymentSignatureHeader(headers: RequestInit['headers'] | undefined): boolean {
  if (!headers) return false;
  if (typeof Headers !== 'undefined' && headers instanceof Headers) {
    return headers.has('payment-signature');
  }
  if (Array.isArray(headers)) {
    return (headers as string[][]).some(
      ([k]) => typeof k === 'string' && k.toLowerCase() === 'payment-signature',
    );
  }
  const rec = headers as Record<string, string>;
  return Object.keys(rec).some((k) => k.toLowerCase() === 'payment-signature');
}

/** Minimal valid JSON string body for the fetch wrapper (must be a string). */
const DUMMY_BODY = JSON.stringify({ model: 'gpt-4o', messages: [{ role: 'user', content: 'hi' }] });

/** Build a simple wallet adapter whose signPayment is a vitest spy. */
function makeWalletAdapter(signFn?: () => Promise<string>): SolvelaWalletAdapter {
  return {
    signPayment: vi.fn(signFn ?? (() => Promise.resolve('fake-sig'))),
  };
}

// ---------------------------------------------------------------------------
// describe: BudgetState
// ---------------------------------------------------------------------------

describe('BudgetState', () => {
  it('new BudgetState(undefined) — reserve always succeeds, remaining is undefined, isDisabled is true', () => {
    const budget = new BudgetState(undefined);

    expect(budget.isDisabled).toBe(true);
    expect(budget.remaining).toBeUndefined();

    // Multiple reserves should not throw
    expect(() => budget.reserve('req-1', 1_000_000n)).not.toThrow();
    expect(() => budget.reserve('req-2', 999_999_999n)).not.toThrow();

    // remaining stays undefined after reserves
    expect(budget.remaining).toBeUndefined();
  });

  it('reserve(60n) succeeds, reserve(50n) throws SolvelaBudgetExceededError, finalize → remaining 40n', () => {
    const budget = new BudgetState(100n);

    // First reserve should succeed (60n ≤ 100n available)
    expect(() => budget.reserve('req-a', 60n)).not.toThrow();

    // After first reserve: 100n - 60n reserved = 40n remaining
    expect(budget.remaining).toBe(40n);

    // Second reserve of 50n should fail (50n > 40n remaining)
    expect(() => budget.reserve('req-b', 50n)).toThrow(SolvelaBudgetExceededError);

    // remaining unchanged after failed reserve
    expect(budget.remaining).toBe(40n);

    // Finalize first reservation: debit 60n from available
    budget.finalize('req-a');

    // After finalize: available is now 40n, no reservations pending → remaining 40n
    expect(budget.remaining).toBe(40n);
  });

  it('release without finalize restores full capacity', () => {
    const budget = new BudgetState(100n);

    budget.reserve('req-a', 60n);
    expect(budget.remaining).toBe(40n);

    // Release (not finalize) — no debit applied
    budget.release('req-a');

    // Full capacity restored
    expect(budget.remaining).toBe(100n);

    // Can now reserve the full 100n again
    expect(() => budget.reserve('req-b', 100n)).not.toThrow();
  });

  it('negative cost throws SolvelaBudgetExceededError', () => {
    const budget = new BudgetState(100n);

    expect(() => budget.reserve('req-neg', -1n)).toThrow(SolvelaBudgetExceededError);

    // Budget should be unaffected — no reservation was recorded
    expect(budget.remaining).toBe(100n);
  });

  it('two synchronous reserves (60n + 50n) against 100n budget — exactly one succeeds, one throws', () => {
    const budget = new BudgetState(100n);

    let firstResult: 'ok' | 'threw' = 'threw';
    let secondResult: 'ok' | 'threw' = 'threw';

    try {
      budget.reserve('req-1', 60n);
      firstResult = 'ok';
    } catch {
      firstResult = 'threw';
    }

    try {
      budget.reserve('req-2', 50n);
      secondResult = 'ok';
    } catch {
      secondResult = 'threw';
    }

    // Exactly one succeeds, one throws — the first reserve wins (synchronous critical section)
    const successes = [firstResult, secondResult].filter((r) => r === 'ok');
    const failures = [firstResult, secondResult].filter((r) => r === 'threw');

    expect(successes).toHaveLength(1);
    expect(failures).toHaveLength(1);

    // The failure must be a budget-exceeded error (checked via the thrown instance above)
    // Verify by attempting again with a fresh budget — explicitly assert error type
    const budget2 = new BudgetState(100n);
    budget2.reserve('req-a', 60n);
    expect(() => budget2.reserve('req-b', 50n)).toThrow(SolvelaBudgetExceededError);
  });
});

// ---------------------------------------------------------------------------
// describe: solvelaFetch concurrency
// ---------------------------------------------------------------------------

describe('solvelaFetch concurrency', () => {
  /**
   * Test: 50 concurrent solvelaFetch calls against a budget that affords
   * exactly 25 payments (25 × 2625n = 65625n).
   *
   * Assertions:
   *   - Exactly 25 calls resolve (fulfilled)
   *   - Exactly 25 calls reject with SolvelaBudgetExceededError
   *   - signPayment is called exactly 25 times (budget-exceeded calls never reach sign)
   */
  it('50 concurrent fetches with budget affording 25 — exactly 25 succeed, 25 throw SolvelaBudgetExceededError, signPayment called 25 times', async () => {
    const TOTAL_CALLS = 50;
    const AFFORDABLE = 25;
    const totalBudget = BigInt(AFFORDABLE) * COST_PER_REQUEST; // 65625n

    const budget = new BudgetState(totalBudget);
    const wallet = makeWalletAdapter();
    const baseFetch = makeMockFetch();

    const options: CreateSolvelaFetchOptions = {
      wallet,
      budget,
      maxSignedBodyBytes: 1_000_000,
      baseFetch,
    };

    const solvelaFetch = createSolvelaFetch(options);

    const calls = Array.from({ length: TOTAL_CALLS }, () =>
      solvelaFetch('https://gateway.solvela.io/v1/chat/completions', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: DUMMY_BODY,
      }),
    );

    const results = await Promise.allSettled(calls);

    const fulfilled = results.filter((r) => r.status === 'fulfilled');
    const rejected = results.filter((r) => r.status === 'rejected');

    // Exactly 25 succeed, 25 fail
    expect(fulfilled).toHaveLength(AFFORDABLE);
    expect(rejected).toHaveLength(TOTAL_CALLS - AFFORDABLE);

    // Every rejection is a SolvelaBudgetExceededError
    for (const r of rejected) {
      expect(r.status).toBe('rejected');
      const reason = (r as PromiseRejectedResult).reason;
      expect(SolvelaBudgetExceededError.isInstance(reason)).toBe(true);
    }

    // signPayment called exactly 25 times (budget-exceeded paths never reach sign)
    expect(wallet.signPayment).toHaveBeenCalledTimes(AFFORDABLE);
  });

  /**
   * Test: abort mid-retry releases reservation, budget visible to next call.
   *
   * Scenario:
   *   1. Fire solvelaFetch with an AbortSignal whose controller aborts after
   *      signPayment returns (post-sign guard: `if (init?.signal?.aborted)`).
   *   2. Verify the call throws an AbortError (not a budget error).
   *   3. Fire a second sequential solvelaFetch without an abort — it should
   *      succeed because the reservation from step 1 was released.
   */
  it('abort after signPayment releases reservation; follow-up call succeeds with full budget visible', async () => {
    const budget = new BudgetState(COST_PER_REQUEST); // exactly one request affordable

    const controller = new AbortController();

    // Wallet adapter that aborts the controller synchronously before returning,
    // simulating abort completing between sign and retry.
    const wallet: SolvelaWalletAdapter = {
      signPayment: vi.fn(async () => {
        controller.abort();
        return 'fake-sig';
      }),
    };

    const baseFetch = makeMockFetch();

    const options: CreateSolvelaFetchOptions = {
      wallet,
      budget,
      maxSignedBodyBytes: 1_000_000,
      baseFetch,
    };

    const solvelaFetch = createSolvelaFetch(options);

    // Step 1: fire with abortable signal — should throw AbortError (not budget error)
    let abortedCallError: unknown;
    try {
      await solvelaFetch('https://gateway.solvela.io/v1/chat/completions', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: DUMMY_BODY,
        signal: controller.signal,
      });
    } catch (err) {
      abortedCallError = err;
    }

    // Must have thrown an AbortError
    expect(abortedCallError).toBeDefined();
    const isAbort =
      abortedCallError != null &&
      typeof abortedCallError === 'object' &&
      (abortedCallError as { name?: string }).name === 'AbortError';
    expect(isAbort).toBe(true);

    // Must NOT be a budget error (reservation was released, not debited)
    expect(SolvelaBudgetExceededError.isInstance(abortedCallError)).toBe(false);

    // Step 2: budget remaining should be back to full (release, not finalize, was called)
    expect(budget.remaining).toBe(COST_PER_REQUEST);

    // Step 3: a fresh sequential call without abort should succeed (budget affords one request)
    const wallet2: SolvelaWalletAdapter = {
      signPayment: vi.fn(() => Promise.resolve('fake-sig-2')),
    };

    const solvelaFetch2 = createSolvelaFetch({
      wallet: wallet2,
      budget,
      maxSignedBodyBytes: 1_000_000,
      baseFetch,
    });

    let secondCallResult: Response | undefined;
    let secondCallError: unknown;
    try {
      secondCallResult = await solvelaFetch2('https://gateway.solvela.io/v1/chat/completions', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: DUMMY_BODY,
      });
    } catch (err) {
      secondCallError = err;
    }

    // Second call should have succeeded (no throw)
    expect(secondCallError).toBeUndefined();
    expect(secondCallResult).toBeDefined();
    expect(secondCallResult?.status).toBe(200);

    // Budget fully debited after successful second call
    expect(budget.remaining).toBe(0n);

    // signPayment called once for the aborted call + once for the follow-up
    expect(wallet.signPayment).toHaveBeenCalledTimes(1);
    expect(wallet2.signPayment).toHaveBeenCalledTimes(1);
  });
});
