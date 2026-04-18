/**
 * IT-12: Caller-supplied PAYMENT-SIGNATURE header (T2-F).
 *
 * Plan §6 Phase 8 row IT-12:
 *   "Initial request carries header; gateway returns 402; wrapper surfaces
 *    SolvelaPaymentError directly, does NOT re-sign."
 *
 * Two sub-tests are implemented:
 *
 *   Test A — Provider-level header filter (construction-time):
 *     createSolvelaProvider receives headers: { 'PAYMENT-SIGNATURE': 'pre-existing' }.
 *     config.ts filterHeaders strips it and emits a one-time console.warn.
 *     A subsequent generateText call proceeds normally (402 → sign → 200 retry)
 *     without the pre-existing header appearing in any outbound request.
 *
 *   Test B — Fetch-wrapper direct call (integration-style, branch (d)):
 *     createSolvelaFetch is called directly with init.headers containing
 *     PAYMENT-SIGNATURE. The wrapper's branch (d) must throw SolvelaPaymentError
 *     ('caller supplied PAYMENT-SIGNATURE…') without calling signPayment or
 *     reserving budget. Exactly 1 base-fetch call is made (no retry).
 *
 * Note on warn-once deduplication:
 *   warnOnce uses a module-level Set that persists across tests in the same
 *   process. We spy on console.warn directly and clear the spy between tests
 *   rather than mocking the warnOnce module (which would break the real
 *   implementation used by all integration tests). The spy assertion in Test A
 *   verifies the warn was emitted, not that it was emitted exactly N times
 *   globally (other test files running first may have already emitted it).
 *   Therefore Test A verifies the WARN MESSAGE CONTENT by inspecting all
 *   calls and checking that at least one mentions PAYMENT-SIGNATURE — this is
 *   robust to run order.
 *
 * Transport:
 *   Test A uses undici.MockAgent (same transport as IT-1).
 *   Test B uses a vi.fn() stub baseFetch — no network.
 *
 * Framework: vitest.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { generateText } from 'ai';

import { createSolvelaProvider } from '../../src/provider.js';
import { createSolvelaFetch } from '../../src/fetch-wrapper.js';
import { BudgetState } from '../../src/budget.js';
import { SolvelaPaymentError } from '../../src/errors.js';

import {
  installMockGateway,
  make402Envelope,
  makeChatCompletionSuccess,
  makeStubWallet,
  getHeader,
  type MockGatewayHandle,
} from './mock-gateway.js';

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const BASE_URL = 'https://gateway.test';
const MOCK_SIGNATURE = 'mock-base64-signature==';
const INTERCEPT_PATH = '/v1/chat/completions';
const REPLY_TEXT = 'hello world';
const MOCK_URL = 'https://api.solvela.io/v1/chat/completions';

// The warn message emitted by config.ts filterHeaders when PAYMENT-SIGNATURE
// is found in the caller-supplied headers at construction time.
const EXPECTED_WARN_FRAGMENT = 'PAYMENT-SIGNATURE';

// ---------------------------------------------------------------------------
// Test A — Provider-level header filter (construction-time)
// ---------------------------------------------------------------------------

describe('IT-12 Test A: provider construction strips caller-supplied PAYMENT-SIGNATURE header', () => {
  let mock: MockGatewayHandle;
  let warnSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    mock = installMockGateway(BASE_URL);
    // Spy on console.warn so we can assert the filter warning is emitted.
    // We do NOT clear the warnOnce module-level Set — instead we check that
    // at least one of the calls contains the expected message fragment.
    warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});
  });

  afterEach(async () => {
    warnSpy.mockRestore();
    await mock.reset();
  });

  it('A1: console.warn is called with a message mentioning PAYMENT-SIGNATURE when the header is supplied at construction', () => {
    // Constructing the provider with a PAYMENT-SIGNATURE header should
    // trigger the warn-once via filterHeaders in config.ts.
    createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: makeStubWallet(MOCK_SIGNATURE),
      headers: { 'PAYMENT-SIGNATURE': 'pre-existing-sig' },
    });

    // At least one warn call must mention PAYMENT-SIGNATURE.
    const allWarnMessages = warnSpy.mock.calls.map(
      (call) => String(call[0]),
    );
    const hasSigWarn = allWarnMessages.some((msg) =>
      msg.includes(EXPECTED_WARN_FRAGMENT),
    );
    expect(hasSigWarn).toBe(true);
  });

  it('A2: first outbound request does NOT carry the pre-existing PAYMENT-SIGNATURE header (filtered at construction)', async () => {
    // Register: 402 first, then 200.
    mock.pool
      .intercept({ path: INTERCEPT_PATH, method: 'POST' })
      .reply(
        mock.captureReply(() => ({
          statusCode: 402,
          data: JSON.stringify(make402Envelope()),
          responseOptions: { headers: { 'content-type': 'application/json' } },
        })),
      );

    mock.pool
      .intercept({ path: INTERCEPT_PATH, method: 'POST' })
      .reply(
        mock.captureReply(() => ({
          statusCode: 200,
          data: JSON.stringify(makeChatCompletionSuccess(REPLY_TEXT)),
          responseOptions: { headers: { 'content-type': 'application/json' } },
        })),
      );

    const provider = createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: makeStubWallet(MOCK_SIGNATURE),
      headers: { 'PAYMENT-SIGNATURE': 'pre-existing-sig' },
    });

    await generateText({
      model: provider('claude-sonnet-4-5'),
      prompt: 'hello',
    });

    // The first (unsigned) request must NOT carry either the pre-existing
    // signature or any payment-signature header at all.
    expect(mock.calls[0]).toBeDefined();
    expect(getHeader(mock.calls[0], 'payment-signature')).toBeUndefined();
  });

  it('A3: normal 402→sign→200 retry proceeds after filtering (wallet signs, retry carries fresh signature)', async () => {
    mock.pool
      .intercept({ path: INTERCEPT_PATH, method: 'POST' })
      .reply(
        mock.captureReply(() => ({
          statusCode: 402,
          data: JSON.stringify(make402Envelope()),
          responseOptions: { headers: { 'content-type': 'application/json' } },
        })),
      );

    mock.pool
      .intercept({ path: INTERCEPT_PATH, method: 'POST' })
      .reply(
        mock.captureReply(() => ({
          statusCode: 200,
          data: JSON.stringify(makeChatCompletionSuccess(REPLY_TEXT)),
          responseOptions: { headers: { 'content-type': 'application/json' } },
        })),
      );

    const provider = createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: makeStubWallet(MOCK_SIGNATURE),
      headers: { 'PAYMENT-SIGNATURE': 'pre-existing-sig' },
    });

    const result = await generateText({
      model: provider('claude-sonnet-4-5'),
      prompt: 'hello',
    });

    // A. Full flow completes successfully.
    expect(result.text).toBe(REPLY_TEXT);
    // B. Exactly 2 HTTP calls (normal 402→retry).
    expect(mock.calls).toHaveLength(2);
    // C. Retry carries the FRESH wallet-signed signature, not the pre-existing one.
    expect(getHeader(mock.calls[1], 'payment-signature')).toBe(MOCK_SIGNATURE);
    // D. Retry does NOT carry the pre-existing 'pre-existing-sig' value.
    expect(getHeader(mock.calls[1], 'payment-signature')).not.toBe(
      'pre-existing-sig',
    );
  });
});

// ---------------------------------------------------------------------------
// Test B — Fetch-wrapper direct call: branch (d) rejection
//
// Tests createSolvelaFetch directly with a PAYMENT-SIGNATURE in init.headers.
// This exercises the scenario where a caller somehow gets a PAYMENT-SIGNATURE
// into the init object reaching the wrapper (bypassing the provider filter).
// Branch (d) must: throw SolvelaPaymentError, NOT call signPayment, NOT
// reserve budget, make exactly 1 base-fetch call.
// ---------------------------------------------------------------------------

describe('IT-12 Test B: fetch-wrapper branch (d) rejects caller-supplied PAYMENT-SIGNATURE without re-signing', () => {
  /**
   * Build a 402 Response that the baseFetch stub will return.
   * The wrapper reads the status (402) then hits branch (d) before parsing.
   */
  function make402Response(): Response {
    const body = JSON.stringify(make402Envelope());
    return new Response(body, {
      status: 402,
      headers: { 'content-type': 'application/json' },
    });
  }

  it('B1: throws SolvelaPaymentError with "caller supplied" message when PAYMENT-SIGNATURE is in init.headers', async () => {
    const baseFetch = vi.fn().mockResolvedValueOnce(make402Response());
    const wallet = {
      label: 'stub',
      signPayment: vi.fn().mockResolvedValue(MOCK_SIGNATURE),
    };

    const solvelaFetch = createSolvelaFetch({
      wallet,
      budget: new BudgetState(undefined),
      maxSignedBodyBytes: 1_000_000,
      baseFetch,
    });

    const err = await solvelaFetch(MOCK_URL, {
      method: 'POST',
      body: '{"model":"test","messages":[]}',
      headers: { 'PAYMENT-SIGNATURE': 'caller-injected-sig' },
    }).catch((e: unknown) => e);

    expect(SolvelaPaymentError.isInstance(err)).toBe(true);
    expect((err as SolvelaPaymentError).message).toContain('caller supplied');
    expect((err as SolvelaPaymentError).message).toContain('PAYMENT-SIGNATURE');
  });

  it('B2: signPayment is NOT called when branch (d) rejects', async () => {
    const baseFetch = vi.fn().mockResolvedValueOnce(make402Response());
    const wallet = {
      label: 'stub',
      signPayment: vi.fn().mockResolvedValue(MOCK_SIGNATURE),
    };

    const solvelaFetch = createSolvelaFetch({
      wallet,
      budget: new BudgetState(undefined),
      maxSignedBodyBytes: 1_000_000,
      baseFetch,
    });

    await solvelaFetch(MOCK_URL, {
      method: 'POST',
      body: '{"model":"test","messages":[]}',
      headers: { 'PAYMENT-SIGNATURE': 'caller-injected-sig' },
    }).catch(() => {});

    expect(wallet.signPayment).not.toHaveBeenCalled();
  });

  it('B3: budget is NOT reserved when branch (d) rejects', async () => {
    const baseFetch = vi.fn().mockResolvedValueOnce(make402Response());
    const wallet = {
      label: 'stub',
      signPayment: vi.fn().mockResolvedValue(MOCK_SIGNATURE),
    };
    const TOTAL = 1_000_000n;
    const budget = new BudgetState(TOTAL);

    const solvelaFetch = createSolvelaFetch({
      wallet,
      budget,
      maxSignedBodyBytes: 1_000_000,
      baseFetch,
    });

    await solvelaFetch(MOCK_URL, {
      method: 'POST',
      body: '{"model":"test","messages":[]}',
      headers: { 'PAYMENT-SIGNATURE': 'caller-injected-sig' },
    }).catch(() => {});

    // Budget untouched — no reservation was made.
    expect(budget.remaining).toBe(TOTAL);
  });

  it('B4: exactly 1 base-fetch call is made (no retry after branch (d) rejection)', async () => {
    const baseFetch = vi.fn().mockResolvedValueOnce(make402Response());
    const wallet = {
      label: 'stub',
      signPayment: vi.fn().mockResolvedValue(MOCK_SIGNATURE),
    };

    const solvelaFetch = createSolvelaFetch({
      wallet,
      budget: new BudgetState(undefined),
      maxSignedBodyBytes: 1_000_000,
      baseFetch,
    });

    await solvelaFetch(MOCK_URL, {
      method: 'POST',
      body: '{"model":"test","messages":[]}',
      headers: { 'PAYMENT-SIGNATURE': 'caller-injected-sig' },
    }).catch(() => {});

    expect(baseFetch).toHaveBeenCalledTimes(1);
  });

  it('B5: case-insensitive detection — lowercase payment-signature also triggers branch (d)', async () => {
    const baseFetch = vi.fn().mockResolvedValueOnce(make402Response());
    const wallet = {
      label: 'stub',
      signPayment: vi.fn().mockResolvedValue(MOCK_SIGNATURE),
    };

    const solvelaFetch = createSolvelaFetch({
      wallet,
      budget: new BudgetState(undefined),
      maxSignedBodyBytes: 1_000_000,
      baseFetch,
    });

    const err = await solvelaFetch(MOCK_URL, {
      method: 'POST',
      body: '{"model":"test","messages":[]}',
      headers: { 'payment-signature': 'lower-case-injected-sig' },
    }).catch((e: unknown) => e);

    expect(SolvelaPaymentError.isInstance(err)).toBe(true);
    expect(wallet.signPayment).not.toHaveBeenCalled();
  });

  it('B6: case-insensitive detection — mixed-case Payment-Signature also triggers branch (d)', async () => {
    const baseFetch = vi.fn().mockResolvedValueOnce(make402Response());
    const wallet = {
      label: 'stub',
      signPayment: vi.fn().mockResolvedValue(MOCK_SIGNATURE),
    };

    const solvelaFetch = createSolvelaFetch({
      wallet,
      budget: new BudgetState(undefined),
      maxSignedBodyBytes: 1_000_000,
      baseFetch,
    });

    const err = await solvelaFetch(MOCK_URL, {
      method: 'POST',
      body: '{"model":"test","messages":[]}',
      headers: { 'Payment-Signature': 'mixed-case-injected-sig' },
    }).catch((e: unknown) => e);

    expect(SolvelaPaymentError.isInstance(err)).toBe(true);
    expect(wallet.signPayment).not.toHaveBeenCalled();
  });
});
