/**
 * IT-8: Budget exceeded — generateText throws SolvelaBudgetExceededError
 * before any sign call; no 402 retry fetch made; adapter NOT invoked.
 *
 * Scenario (plan §6 Phase 8 row IT-8):
 *   - Provider is configured with sessionBudget: 100n (atomic USDC units).
 *   - Mock gateway returns a single 402 with amount '999999999999' (far exceeds budget).
 *   - fetch-wrapper parses the 402, attempts to reserve budget, finds cost > remaining,
 *     throws SolvelaBudgetExceededError synchronously — before calling the wallet adapter
 *     and before issuing the retry fetch.
 *
 * Assertions:
 *   A. generateText rejects with SolvelaBudgetExceededError.
 *   B. Exactly 1 HTTP call reaches the mock gateway (the initial 402 probe; no retry).
 *   C. spyAdapter.signPayment is called ZERO times (budget guard fires before sign).
 *   D. The error's isRetryable === false.
 *
 * Note on pending interceptors:
 *   Only ONE intercept is registered (the 402). undici.MockAgent.assertNoPendingInterceptors()
 *   runs in mock.reset() — a second, unconsumed intercept would fail teardown.
 *   This is the correct guard: if the implementation ever issues a retry when the budget
 *   is exhausted, reset() will surface an extra pending interceptor as a test failure.
 *
 * Transport: undici.MockAgent via installMockGateway — no real network calls.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { generateText } from 'ai';
import { createSolvelaProvider } from '../../src/provider.js';
import { SolvelaBudgetExceededError } from '../../src/errors.js';
import {
  installMockGateway,
  make402Envelope,
  makeStubWallet,
  type MockGatewayHandle,
} from './mock-gateway.js';

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const BASE_URL = 'https://gateway.test';
const INTERCEPT_PATH = '/v1/chat/completions';

/**
 * A budget of 100 atomic USDC units. The 402 envelope amount below (999999999999)
 * is many orders of magnitude larger, guaranteeing an immediate budget failure.
 */
const SESSION_BUDGET = 100n;

/**
 * Amount in the 402 envelope — must exceed SESSION_BUDGET to trigger the error.
 * This is a large value so the test is robust against any future budget-unit changes.
 */
const OVER_BUDGET_AMOUNT = '999999999999';

// ---------------------------------------------------------------------------
// Test setup
// ---------------------------------------------------------------------------

let mock: MockGatewayHandle;

beforeEach(() => {
  mock = installMockGateway(BASE_URL);
});

afterEach(async () => {
  // assertNoPendingInterceptors() is called inside reset(). If any extra intercept
  // was registered (e.g. a retry intercept) and never consumed, this will surface
  // a test failure — which is exactly the behaviour we want for double-spend guard.
  await mock.reset();
});

// ---------------------------------------------------------------------------
// IT-8: Budget exceeded
// ---------------------------------------------------------------------------

describe('IT-8: budget exceeded — SolvelaBudgetExceededError thrown before any sign', () => {
  it('A. generateText rejects with SolvelaBudgetExceededError', async () => {
    // Register ONLY the initial 402 probe. No second intercept — budget guard
    // fires synchronously after parsing the 402 and before any retry fetch.
    mock.pool
      .intercept({ path: INTERCEPT_PATH, method: 'POST' })
      .reply(
        mock.captureReply(() => ({
          statusCode: 402,
          data: JSON.stringify(make402Envelope({ amount: OVER_BUDGET_AMOUNT, total: OVER_BUDGET_AMOUNT })),
          responseOptions: { headers: { 'content-type': 'application/json' } },
        })),
      );

    const adapter = makeStubWallet();
    const provider = createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: adapter,
      sessionBudget: SESSION_BUDGET,
    });

    await expect(
      generateText({ model: provider('claude-sonnet-4-5'), prompt: 'hello' }),
    ).rejects.toSatisfy((err: unknown) => SolvelaBudgetExceededError.isInstance(err));
  });

  it('B. exactly 1 HTTP call reaches the mock gateway (no retry)', async () => {
    mock.pool
      .intercept({ path: INTERCEPT_PATH, method: 'POST' })
      .reply(
        mock.captureReply(() => ({
          statusCode: 402,
          data: JSON.stringify(make402Envelope({ amount: OVER_BUDGET_AMOUNT, total: OVER_BUDGET_AMOUNT })),
          responseOptions: { headers: { 'content-type': 'application/json' } },
        })),
      );

    const provider = createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: makeStubWallet(),
      sessionBudget: SESSION_BUDGET,
    });

    await expect(
      generateText({ model: provider('claude-sonnet-4-5'), prompt: 'hello' }),
    ).rejects.toThrow();

    expect(mock.calls).toHaveLength(1);
  });

  it('C. spyAdapter.signPayment is called ZERO times', async () => {
    mock.pool
      .intercept({ path: INTERCEPT_PATH, method: 'POST' })
      .reply(
        mock.captureReply(() => ({
          statusCode: 402,
          data: JSON.stringify(make402Envelope({ amount: OVER_BUDGET_AMOUNT, total: OVER_BUDGET_AMOUNT })),
          responseOptions: { headers: { 'content-type': 'application/json' } },
        })),
      );

    const adapter = makeStubWallet();
    // Spy wraps the existing signPayment function — adapter is a plain object so
    // vi.spyOn targets the property directly.
    const signSpy = vi.spyOn(adapter, 'signPayment');

    const provider = createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: adapter,
      sessionBudget: SESSION_BUDGET,
    });

    await expect(
      generateText({ model: provider('claude-sonnet-4-5'), prompt: 'hello' }),
    ).rejects.toThrow();

    expect(signSpy).toHaveBeenCalledTimes(0);

    vi.restoreAllMocks();
  });

  it('D. error isRetryable === false', async () => {
    mock.pool
      .intercept({ path: INTERCEPT_PATH, method: 'POST' })
      .reply(
        mock.captureReply(() => ({
          statusCode: 402,
          data: JSON.stringify(make402Envelope({ amount: OVER_BUDGET_AMOUNT, total: OVER_BUDGET_AMOUNT })),
          responseOptions: { headers: { 'content-type': 'application/json' } },
        })),
      );

    const provider = createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: makeStubWallet(),
      sessionBudget: SESSION_BUDGET,
    });

    let caught: unknown;
    try {
      await generateText({ model: provider('claude-sonnet-4-5'), prompt: 'hello' });
    } catch (err) {
      caught = err;
    }

    expect(caught).toBeDefined();
    expect(SolvelaBudgetExceededError.isInstance(caught)).toBe(true);
    expect((caught as SolvelaBudgetExceededError).isRetryable).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// IT-8 consolidated: all four assertions in one test (catches ordering bugs)
// ---------------------------------------------------------------------------

describe('IT-8 consolidated: all assertions together', () => {
  it('budget exceeded: BudgetExceededError thrown, 1 call, 0 sign calls, isRetryable false', async () => {
    mock.pool
      .intercept({ path: INTERCEPT_PATH, method: 'POST' })
      .reply(
        mock.captureReply(() => ({
          statusCode: 402,
          data: JSON.stringify(make402Envelope({ amount: OVER_BUDGET_AMOUNT, total: OVER_BUDGET_AMOUNT })),
          responseOptions: { headers: { 'content-type': 'application/json' } },
        })),
      );

    const adapter = makeStubWallet();
    const signSpy = vi.spyOn(adapter, 'signPayment');

    const provider = createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: adapter,
      sessionBudget: SESSION_BUDGET,
    });

    let caught: unknown;
    try {
      await generateText({ model: provider('claude-sonnet-4-5'), prompt: 'hello' });
    } catch (err) {
      caught = err;
    }

    // A. correct error type
    expect(SolvelaBudgetExceededError.isInstance(caught)).toBe(true);
    // B. exactly 1 HTTP call (probe 402 only — no retry)
    expect(mock.calls).toHaveLength(1);
    // C. wallet adapter NEVER invoked
    expect(signSpy).toHaveBeenCalledTimes(0);
    // D. not retryable
    expect((caught as SolvelaBudgetExceededError).isRetryable).toBe(false);

    vi.restoreAllMocks();
  });
});
