/**
 * IT-7: Network error on retry — sanitized error, no PAYMENT-SIGNATURE leak,
 * retry-bomb guard NOT triggered, budget reservation released.
 *
 * Scenario (per plan §6 Phase 8 row IT-7):
 *   1. First intercept returns 402 with valid envelope (triggers sign + retry).
 *   2. Second intercept calls replyWithError(new Error('network closed')) —
 *      undici throws a transport error on the retry fetch instead of returning
 *      a Response.
 *
 * Assertions:
 *   A. generateText rejects (error propagates to caller).
 *   B. The sentinel signature value is absent from every error surface:
 *      err.message, err.stack, err.cause (recursively), JSON.stringify(err),
 *      err.responseHeaders (if present), err.requestBodyValues (if present).
 *   C. Exactly 2 HTTP calls were attempted — confirmed by registering exactly
 *      2 intercepts and asserting assertNoPendingInterceptors() passes (no
 *      un-consumed intercept = no 3rd call). Note: replyWithError bypasses
 *      captureReply, so mock.calls.length === 1 (only the 402 is captured).
 *   D. Budget reservation is released after network failure — a subsequent call
 *      on the same provider instance with an affordable budget succeeds.
 *
 * Retry-bomb guard (plan §4.3 "retry-bomb guard"):
 *   The guard fires only on a retry 402 response, NOT on network errors.
 *   This is verified implicitly: the error thrown is not
 *   SolvelaPaymentError("Payment rejected after retry").
 *
 * Transport: undici.MockAgent via installMockGateway — no real network calls.
 *
 * Sentinel signature value: 'IT7-SENTINEL-SIG-abcDEF==' — distinct from IT-1
 * to prevent false-negatives from incidental byte matches.
 */

import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { generateText } from 'ai';
import { createSolvelaProvider } from '../../src/provider.js';
import {
  installMockGateway,
  make402Envelope,
  makeChatCompletionSuccess,
  makeStubWallet,
  type MockGatewayHandle,
} from './mock-gateway.js';

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const BASE_URL = 'https://gateway.test';
const INTERCEPT_PATH = '/v1/chat/completions';

/**
 * Distinct sentinel used for this scenario. Must be absent from every error
 * surface after a network failure — the signature was built but not submitted
 * (the retry fetch itself failed before any response was read).
 */
const IT7_SENTINEL_SIG = 'IT7-SENTINEL-SIG-abcDEF==';

/**
 * The mock-gateway 402 envelope uses amount '2625'. Budget must be >= 2625
 * to afford the first call and enough for a second call in the budget-release
 * assertion.
 */
const AMPLE_BUDGET = 10_000n; // well above 2 * 2625

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/**
 * Walk the cause chain of an error recursively and collect every Error-like
 * object encountered. Uses a visited Set to guard against cycles.
 */
function collectCauseChain(root: unknown): unknown[] {
  const collected: unknown[] = [];
  const visited = new Set<unknown>();

  function visit(node: unknown): void {
    if (node == null || visited.has(node)) return;
    visited.add(node);
    collected.push(node);
    const cause = (node as Record<string, unknown>)['cause'];
    if (cause !== undefined) visit(cause);
  }

  visit(root);
  return collected;
}

/**
 * Assert the sentinel signature string is absent from all error surfaces.
 *
 * Surfaces checked:
 *   - err.message
 *   - err.stack
 *   - err.cause (recursively, full chain)
 *   - JSON.stringify(err)
 *   - err.responseHeaders (if present as object or string)
 *   - err.requestBodyValues (if present)
 */
function assertSentinelAbsent(err: unknown, sentinel: string): void {
  const e = err as Record<string, unknown>;

  // message
  if (typeof e['message'] === 'string') {
    expect(e['message'], 'sentinel absent from err.message').not.toContain(sentinel);
  }

  // stack
  if (typeof e['stack'] === 'string') {
    expect(e['stack'], 'sentinel absent from err.stack').not.toContain(sentinel);
  }

  // cause chain — walk recursively, check message + stack of each node
  const causeChain = collectCauseChain(e['cause']);
  for (const node of causeChain) {
    const n = node as Record<string, unknown>;
    if (typeof n['message'] === 'string') {
      expect(n['message'], 'sentinel absent from cause.message').not.toContain(sentinel);
    }
    if (typeof n['stack'] === 'string') {
      expect(n['stack'], 'sentinel absent from cause.stack').not.toContain(sentinel);
    }
    // If a cause node is stringifiable, check that too
    try {
      const causeStr = JSON.stringify(n);
      expect(causeStr, 'sentinel absent from JSON.stringify(cause node)').not.toContain(sentinel);
    } catch {
      // Non-serializable cause — skip JSON check for this node
    }
  }

  // JSON.stringify(err) — catches enumerable fields not explicitly named above
  try {
    const serialized = JSON.stringify(err);
    expect(serialized, 'sentinel absent from JSON.stringify(err)').not.toContain(sentinel);
  } catch {
    // Non-serializable error — skip JSON check
  }

  // responseHeaders — may be object or stringified
  const rh = e['responseHeaders'];
  if (rh !== undefined) {
    if (typeof rh === 'string') {
      expect(rh, 'sentinel absent from err.responseHeaders (string)').not.toContain(sentinel);
    } else if (typeof rh === 'object' && rh !== null) {
      const rhStr = JSON.stringify(rh);
      expect(rhStr, 'sentinel absent from err.responseHeaders (object)').not.toContain(sentinel);
    }
  }

  // requestBodyValues — may be object or string
  const rbv = e['requestBodyValues'];
  if (rbv !== undefined) {
    if (typeof rbv === 'string') {
      expect(rbv, 'sentinel absent from err.requestBodyValues (string)').not.toContain(sentinel);
    } else if (typeof rbv === 'object' && rbv !== null) {
      const rbvStr = JSON.stringify(rbv);
      expect(rbvStr, 'sentinel absent from err.requestBodyValues (object)').not.toContain(sentinel);
    }
  }
}

// ---------------------------------------------------------------------------
// Test setup
// ---------------------------------------------------------------------------

let mock: MockGatewayHandle;

beforeEach(() => {
  mock = installMockGateway(BASE_URL);
});

afterEach(async () => {
  await mock.reset();
});

// ---------------------------------------------------------------------------
// Helper: register the IT-7 two-intercept sequence on mock.pool.
//
// First intercept: 402 (captured via captureReply — call is recorded).
// Second intercept: replyWithError (NOT captured — undici throws before
// returning a Response, so captureReply's callback is never invoked).
// ---------------------------------------------------------------------------

function registerIT7Intercepts(): void {
  // First intercept — 402 with valid payment envelope.
  mock.pool
    .intercept({ path: INTERCEPT_PATH, method: 'POST' })
    .reply(
      mock.captureReply(() => ({
        statusCode: 402,
        data: JSON.stringify(make402Envelope()),
        responseOptions: { headers: { 'content-type': 'application/json' } },
      })),
    );

  // Second intercept — network-level error (transport close).
  // undici's replyWithError() causes the fetch Promise to reject with the
  // given Error, simulating a connection drop during the retry.
  // This intercept is consumed (so assertNoPendingInterceptors passes) but
  // does NOT invoke captureReply, so mock.calls remains length 1.
  mock.pool
    .intercept({ path: INTERCEPT_PATH, method: 'POST' })
    .replyWithError(new Error('network closed'));
}

// ---------------------------------------------------------------------------
// A. generateText rejects
// ---------------------------------------------------------------------------

describe('IT-7: network error on retry', () => {
  it('A. generateText rejects when the retry fetch encounters a network error', async () => {
    registerIT7Intercepts();

    const provider = createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: makeStubWallet(IT7_SENTINEL_SIG),
      sessionBudget: AMPLE_BUDGET,
    });

    await expect(
      generateText({
        model: provider('claude-sonnet-4-5'),
        prompt: 'hello',
        maxRetries: 0,
      }),
    ).rejects.toThrow();
  });

  // ---------------------------------------------------------------------------
  // B. Sentinel absence battery
  // ---------------------------------------------------------------------------

  it('B. sentinel signature value is absent from every error surface', async () => {
    registerIT7Intercepts();

    const provider = createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: makeStubWallet(IT7_SENTINEL_SIG),
      sessionBudget: AMPLE_BUDGET,
    });

    let caught: unknown;
    try {
      await generateText({
        model: provider('claude-sonnet-4-5'),
        prompt: 'hello',
        maxRetries: 0,
      });
    } catch (err) {
      caught = err;
    }

    expect(caught, 'an error must have been thrown').toBeDefined();
    assertSentinelAbsent(caught, IT7_SENTINEL_SIG);
  });

  // ---------------------------------------------------------------------------
  // C. Exactly 2 HTTP calls were attempted (no 3rd retry)
  //
  // Proof mechanism:
  //   - We registered exactly 2 intercepts.
  //   - mock.reset() calls agent.assertNoPendingInterceptors() — if any
  //     intercept was NOT consumed, the test fails with "pending interceptors".
  //   - agent.disableNetConnect() + no third registered intercept means a 3rd
  //     call would throw MockNotMatchedError before reaching any assertion.
  //
  // Note on mock.calls.length:
  //   replyWithError bypasses captureReply entirely, so only the 402 call is
  //   captured. mock.calls.length === 1 is the correct post-flight value here.
  // ---------------------------------------------------------------------------

  it('C. exactly 2 HTTP calls attempted — both intercepts consumed, no 3rd call', async () => {
    registerIT7Intercepts();

    const provider = createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: makeStubWallet(IT7_SENTINEL_SIG),
      sessionBudget: AMPLE_BUDGET,
    });

    // Swallow the expected rejection — we care about side effects.
    await generateText({
      model: provider('claude-sonnet-4-5'),
      prompt: 'hello',
      maxRetries: 0,
    }).catch(() => undefined);

    // Only the 402 is captured (replyWithError bypasses captureReply).
    // assertNoPendingInterceptors() in mock.reset() proves the 2nd was consumed.
    expect(mock.calls).toHaveLength(1);
    expect(mock.calls[0], 'first call must be defined').toBeDefined();
    // The 402 call must not carry a PAYMENT-SIGNATURE (it is the unsigned first attempt).
    expect(
      mock.calls[0]?.headers['payment-signature'],
      'first call carries no PAYMENT-SIGNATURE',
    ).toBeUndefined();

    // assertNoPendingInterceptors runs in afterEach → mock.reset().
    // Reaching this line without a MockNotMatchedError proves no 3rd call fired.
  });

  // ---------------------------------------------------------------------------
  // D. Budget reservation released — same provider succeeds on follow-up call
  //
  // Approach:
  //   - Use a budget of exactly 5250 (2 × 2625) — enough for exactly two
  //     successful requests, each costing 2625 atomic USDC units.
  //   - First request: 402 then network error → reservation released (no debit).
  //   - Second request: 402 then 200 → budget finalized (2625 debited).
  //   - If the reservation were NOT released, the second request would see only
  //     5250 - 2625 (reserved) = 2625 available = exactly enough, so we set the
  //     budget to 2625 exactly: if not released, reserve would see 0 remaining.
  //
  //   Budget = 2625n means: one request's cost (2625) reserved after first 402.
  //   If the network error does NOT release the reservation, the second call's
  //   reserve() would see remaining = 2625 - 2625 (still reserved) = 0 < 2625
  //   and throw SolvelaBudgetExceededError. If it IS released, remaining = 2625
  //   and the second call proceeds normally.
  // ---------------------------------------------------------------------------

  it('D. budget reservation is released after network failure — follow-up call succeeds', async () => {
    const TIGHT_BUDGET = 2625n; // exactly one request's cost from make402Envelope

    // --- First call: 402 then network error ---
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
      .replyWithError(new Error('network closed'));

    // --- Second call: 402 then 200 ---
    mock.pool
      .intercept({ path: INTERCEPT_PATH, method: 'POST' })
      .reply(
        mock.captureReply(() => ({
          statusCode: 402,
          data: JSON.stringify(make402Envelope()),
          responseOptions: { headers: { 'content-type': 'application/json' } },
        })),
      );

    const REPLY_TEXT = 'budget released ok';
    mock.pool
      .intercept({ path: INTERCEPT_PATH, method: 'POST' })
      .reply(
        mock.captureReply(() => ({
          statusCode: 200,
          data: JSON.stringify(makeChatCompletionSuccess(REPLY_TEXT)),
          responseOptions: { headers: { 'content-type': 'application/json' } },
        })),
      );

    // Same provider instance — budget state is shared.
    const provider = createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: makeStubWallet(IT7_SENTINEL_SIG),
      sessionBudget: TIGHT_BUDGET,
    });

    // First call: must fail with the network error.
    await expect(
      generateText({
        model: provider('claude-sonnet-4-5'),
        prompt: 'hello',
        maxRetries: 0,
      }),
    ).rejects.toThrow();

    // Second call: must succeed — proving the reservation was released.
    const result = await generateText({
      model: provider('claude-sonnet-4-5'),
      prompt: 'hello again',
      maxRetries: 0,
    });

    expect(result.text).toBe(REPLY_TEXT);
  });

  // ---------------------------------------------------------------------------
  // Consolidated: all assertions in one test — catches ordering bugs
  // ---------------------------------------------------------------------------

  it('consolidated: rejects, sentinel absent, 2 interceptors consumed, budget released', async () => {
    const TIGHT_BUDGET = 2625n;

    // --- First sequence: 402 + network error ---
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
      .replyWithError(new Error('network closed'));

    // --- Second sequence: 402 + 200 (proves budget release) ---
    mock.pool
      .intercept({ path: INTERCEPT_PATH, method: 'POST' })
      .reply(
        mock.captureReply(() => ({
          statusCode: 402,
          data: JSON.stringify(make402Envelope()),
          responseOptions: { headers: { 'content-type': 'application/json' } },
        })),
      );
    const REPLY_TEXT = 'consolidated ok';
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
      wallet: makeStubWallet(IT7_SENTINEL_SIG),
      sessionBudget: TIGHT_BUDGET,
    });

    // A. Rejects
    let caught: unknown;
    try {
      await generateText({ model: provider('claude-sonnet-4-5'), prompt: 'hello', maxRetries: 0 });
    } catch (err) {
      caught = err;
    }
    expect(caught, 'first call must reject').toBeDefined();

    // B. Sentinel absence battery on the caught error
    assertSentinelAbsent(caught, IT7_SENTINEL_SIG);

    // C. First call was captured (the 402); only 1 entry because replyWithError
    //    bypasses captureReply. The missing entry is the network-error call.
    //    Both intercepts were consumed — assertNoPendingInterceptors in reset()
    //    would fail if the second intercept was not consumed (i.e., no 3rd call).
    expect(mock.calls).toHaveLength(1);
    // No SolvelaPaymentError("Payment rejected after retry") — this error string
    // only appears when the retry returns 402, not on network errors.
    const msg = typeof caught === 'object' && caught !== null
      ? String((caught as Record<string, unknown>)['message'] ?? '')
      : String(caught);
    expect(msg).not.toContain('Payment rejected after retry');

    // D. Budget released — second call succeeds with the same tight budget.
    const result = await generateText({ model: provider('claude-sonnet-4-5'), prompt: 'hello again', maxRetries: 0 });
    expect(result.text).toBe(REPLY_TEXT);
  });
});
