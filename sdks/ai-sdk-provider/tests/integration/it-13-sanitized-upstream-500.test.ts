/**
 * IT-13: Sanitized upstream 500 on retry leg (T1-C Option A seam).
 *
 * Scenario:
 *   Intercept 1: 402 with valid payment envelope.
 *   Intercept 2 (retry leg): 500 with body {"error":"internal","signature_ref":"SHOULD_REDACT"}
 *     and a response header PAYMENT-SIGNATURE: SHOULD_BE_STRIPPED (gateway echoing the
 *     signature — defense-in-depth test).
 *
 * Sentinel value: 'mock-base64-signature==' (MOCK_SIGNATURE constant below).
 *
 * Test A — first call with upstream 500 after payment:
 *   - generateText rejects.
 *   - SolvelaUpstreamError.isInstance(err) is true.
 *   - err.statusCode === 500.
 *   - err.isRetryable === false (post-payment explicit override).
 *   - responseHeaders has no key with lowercase 'payment-signature' (any case).
 *   - SHOULD_BE_STRIPPED not present in any responseHeaders value.
 *   - err.requestBodyValues === undefined.
 *   - Sentinel battery (mock-base64-signature==): absent from message, stack,
 *     cause, toString(), JSON.stringify(err).
 *
 * Test B — follow-up call on same provider instance, 402→500:
 *   - Second consecutive generateText on the same provider, same failure pattern.
 *   - Budget (set to exactly one call's cost) is released (not finalized) by the
 *     first failure, so the second reserve succeeds — proves no accidental finalize.
 *   - Second call also throws SolvelaUpstreamError, not SolvelaBudgetExceededError.
 *   - No cross-call signature leakage.
 */

import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { generateText } from 'ai';
import { createSolvelaProvider } from '../../src/provider.js';
import { SolvelaUpstreamError } from '../../src/errors.js';
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
const MOCK_SIGNATURE = 'mock-base64-signature==';
const INTERCEPT_PATH = '/v1/chat/completions';

/**
 * The 500 body the mock gateway returns on the retry leg.
 * Contains a literal 'signature_ref' key whose value ('SHOULD_REDACT') is
 * short enough not to trigger base58/hex regex, but verifying it appears in
 * responseBody (non-redacted) is intentional — the sentinel check is for
 * MOCK_SIGNATURE, not for 'SHOULD_REDACT'.
 */
const UPSTREAM_500_BODY = JSON.stringify({
  error: 'internal',
  signature_ref: 'SHOULD_REDACT',
});

/**
 * The echoed PAYMENT-SIGNATURE value the mock gateway returns as a response
 * header on the retry leg (defense-in-depth: assert it is stripped).
 */
const ECHOED_SIGNATURE_VALUE = 'SHOULD_BE_STRIPPED';

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
// Shared helpers
// ---------------------------------------------------------------------------

/**
 * Register one 402→500 intercept pair on mock.pool.
 * The 500 response echoes a PAYMENT-SIGNATURE response header.
 */
function register402Then500(): void {
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
        statusCode: 500,
        data: UPSTREAM_500_BODY,
        responseOptions: {
          headers: {
            'content-type': 'application/json',
            // Gateway echoing the payment signature back in a response header —
            // the wrapper must strip it before surfacing in the error.
            'PAYMENT-SIGNATURE': ECHOED_SIGNATURE_VALUE,
          },
        },
      })),
    );
}

/**
 * Run a generateText and return the caught error. Asserts that the call
 * always rejects (any resolve is a test failure).
 */
async function generateAndCatch(): Promise<unknown> {
  try {
    await generateText({
      model: createSolvelaProvider({
        baseURL: BASE_URL,
        wallet: makeStubWallet(MOCK_SIGNATURE),
      })('claude-sonnet-4-5'),
      prompt: 'hello',
    });
    // generateText should never resolve in this scenario.
    throw new Error('generateText resolved unexpectedly; expected it to reject');
  } catch (err) {
    return err;
  }
}

// ---------------------------------------------------------------------------
// IT-13-A: First call — upstream 500 after payment
// ---------------------------------------------------------------------------

describe('IT-13-A: first call upstream 500 after payment', () => {
  it('A-1. generateText rejects', async () => {
    register402Then500();
    await expect(
      generateText({
        model: createSolvelaProvider({
          baseURL: BASE_URL,
          wallet: makeStubWallet(MOCK_SIGNATURE),
        })('claude-sonnet-4-5'),
        prompt: 'hello',
      }),
    ).rejects.toBeDefined();
  });

  it('A-2. thrown error is SolvelaUpstreamError', async () => {
    register402Then500();
    const err = await generateAndCatch();
    expect(SolvelaUpstreamError.isInstance(err)).toBe(true);
  });

  it('A-3. statusCode === 500', async () => {
    register402Then500();
    const err = await generateAndCatch();
    expect(SolvelaUpstreamError.isInstance(err)).toBe(true);
    const upstream = err as SolvelaUpstreamError;
    expect(upstream.statusCode).toBe(500);
  });

  it('A-4. isRetryable === false (post-payment explicit override)', async () => {
    register402Then500();
    const err = await generateAndCatch();
    expect(SolvelaUpstreamError.isInstance(err)).toBe(true);
    const upstream = err as SolvelaUpstreamError;
    expect(upstream.isRetryable).toBe(false);
  });

  it('A-5. responseHeaders does not contain PAYMENT-SIGNATURE (any case)', async () => {
    register402Then500();
    const err = await generateAndCatch();
    expect(SolvelaUpstreamError.isInstance(err)).toBe(true);
    const upstream = err as SolvelaUpstreamError;

    // No header key (any case) should be 'payment-signature'.
    for (const k of Object.keys(upstream.responseHeaders ?? {})) {
      expect(k.toLowerCase()).not.toBe('payment-signature');
    }

    // Explicit undefined checks for both casings.
    expect(upstream.responseHeaders?.['PAYMENT-SIGNATURE']).toBeUndefined();
    expect(upstream.responseHeaders?.['payment-signature']).toBeUndefined();
  });

  it('A-6. SHOULD_BE_STRIPPED value not present in any responseHeaders value', async () => {
    register402Then500();
    const err = await generateAndCatch();
    expect(SolvelaUpstreamError.isInstance(err)).toBe(true);
    const upstream = err as SolvelaUpstreamError;

    for (const v of Object.values(upstream.responseHeaders ?? {})) {
      expect(v).not.toContain(ECHOED_SIGNATURE_VALUE);
    }
  });

  it('A-7. requestBodyValues === undefined', async () => {
    register402Then500();
    const err = await generateAndCatch();
    expect(SolvelaUpstreamError.isInstance(err)).toBe(true);
    const upstream = err as SolvelaUpstreamError;
    expect(upstream.requestBodyValues).toBeUndefined();
  });

  it('A-7b. responseBody is defined and sentinel-free', async () => {
    register402Then500();
    const err = await generateAndCatch();
    expect(SolvelaUpstreamError.isInstance(err)).toBe(true);
    const upstream = err as SolvelaUpstreamError;
    // The field must be populated (Option A seam buffers the error envelope).
    expect(upstream.responseBody).toBeDefined();
    // The sentinel signature must not appear in the body.
    expect(upstream.responseBody).not.toContain(MOCK_SIGNATURE);
    // The echoed gateway signature value must not appear.
    expect(upstream.responseBody).not.toContain(ECHOED_SIGNATURE_VALUE);
  });

  it('A-8. sentinel battery: MOCK_SIGNATURE absent from message', async () => {
    register402Then500();
    const err = await generateAndCatch();
    expect(SolvelaUpstreamError.isInstance(err)).toBe(true);
    const upstream = err as SolvelaUpstreamError;
    expect(upstream.message).not.toContain(MOCK_SIGNATURE);
  });

  it('A-9. sentinel battery: MOCK_SIGNATURE absent from stack', async () => {
    register402Then500();
    const err = await generateAndCatch();
    expect(SolvelaUpstreamError.isInstance(err)).toBe(true);
    const upstream = err as SolvelaUpstreamError;
    expect(upstream.stack ?? '').not.toContain(MOCK_SIGNATURE);
  });

  it('A-10. sentinel battery: MOCK_SIGNATURE absent from cause', async () => {
    register402Then500();
    const err = await generateAndCatch();
    expect(SolvelaUpstreamError.isInstance(err)).toBe(true);
    const upstream = err as SolvelaUpstreamError;
    // cause is undefined — the wrapper constructs SolvelaUpstreamError without cause.
    expect(upstream.cause).toBeUndefined();
    // Belt-and-braces: stringify cause regardless.
    expect(String(upstream.cause ?? '')).not.toContain(MOCK_SIGNATURE);
  });

  it('A-11. sentinel battery: MOCK_SIGNATURE absent from toString()', async () => {
    register402Then500();
    const err = await generateAndCatch();
    expect(SolvelaUpstreamError.isInstance(err)).toBe(true);
    const upstream = err as SolvelaUpstreamError;
    expect(upstream.toString()).not.toContain(MOCK_SIGNATURE);
  });

  it('A-12. sentinel battery: MOCK_SIGNATURE absent from JSON.stringify(err)', async () => {
    register402Then500();
    const err = await generateAndCatch();
    expect(SolvelaUpstreamError.isInstance(err)).toBe(true);
    const upstream = err as SolvelaUpstreamError;
    let serialized: string;
    try {
      serialized = JSON.stringify(upstream);
    } catch {
      // If JSON.stringify throws (circular ref etc.), use a fallback.
      serialized = String(upstream);
    }
    expect(serialized).not.toContain(MOCK_SIGNATURE);
  });
});

// ---------------------------------------------------------------------------
// IT-13-A consolidated: all A assertions in one test (catches ordering bugs)
// ---------------------------------------------------------------------------

describe('IT-13-A consolidated: all assertions together', () => {
  it('402→500: SolvelaUpstreamError, statusCode 500, isRetryable false, no sig leak', async () => {
    register402Then500();
    const err = await generateAndCatch();

    // Type assertion
    expect(SolvelaUpstreamError.isInstance(err)).toBe(true);
    const upstream = err as SolvelaUpstreamError;

    // statusCode and retryability
    expect(upstream.statusCode).toBe(500);
    expect(upstream.isRetryable).toBe(false);

    // responseHeaders: no PAYMENT-SIGNATURE in any case
    for (const k of Object.keys(upstream.responseHeaders ?? {})) {
      expect(k.toLowerCase()).not.toBe('payment-signature');
    }
    expect(upstream.responseHeaders?.['PAYMENT-SIGNATURE']).toBeUndefined();
    expect(upstream.responseHeaders?.['payment-signature']).toBeUndefined();

    // No echoed signature value in any header value
    for (const v of Object.values(upstream.responseHeaders ?? {})) {
      expect(v).not.toContain(ECHOED_SIGNATURE_VALUE);
    }

    // requestBodyValues
    expect(upstream.requestBodyValues).toBeUndefined();

    // cause is undefined
    expect(upstream.cause).toBeUndefined();

    // Sentinel battery: MOCK_SIGNATURE must not appear on any of the 5 surfaces
    expect(upstream.message).not.toContain(MOCK_SIGNATURE);
    expect(upstream.stack ?? '').not.toContain(MOCK_SIGNATURE);
    expect(String(upstream.cause ?? '')).not.toContain(MOCK_SIGNATURE);
    expect(upstream.toString()).not.toContain(MOCK_SIGNATURE);
    let serialized: string;
    try {
      serialized = JSON.stringify(upstream);
    } catch {
      serialized = String(upstream);
    }
    expect(serialized).not.toContain(MOCK_SIGNATURE);

    // responseBody: populated and sentinel-free
    expect(upstream.responseBody).toBeDefined();
    expect(upstream.responseBody).not.toContain(MOCK_SIGNATURE);
    expect(upstream.responseBody).not.toContain(ECHOED_SIGNATURE_VALUE);
  });
});

// ---------------------------------------------------------------------------
// IT-13-B: Follow-up call on same provider — cross-call independence
// ---------------------------------------------------------------------------

describe('IT-13-B: follow-up call on same provider instance', () => {
  it('B-1. second call also throws SolvelaUpstreamError (not SolvelaBudgetExceededError)', async () => {
    // Register two 402→500 intercept pairs for two consecutive calls.
    register402Then500(); // call 1: 402 + 500
    register402Then500(); // call 2: 402 + 500

    // Budget set to exactly the cost of one request (2625 atomic USDC units,
    // matching make402Envelope() default). The first call must release (not
    // finalize) its reservation so the second call can reserve again.
    // If the first call accidentally finalized, the second would throw
    // SolvelaBudgetExceededError instead of SolvelaUpstreamError.
    const provider = createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: makeStubWallet(MOCK_SIGNATURE),
      sessionBudget: 2625n,
    });

    const err1 = await (async () => {
      try {
        await generateText({ model: provider('claude-sonnet-4-5'), prompt: 'hello' });
        throw new Error('call 1 resolved unexpectedly');
      } catch (e) {
        return e;
      }
    })();

    const err2 = await (async () => {
      try {
        await generateText({ model: provider('claude-sonnet-4-5'), prompt: 'world' });
        throw new Error('call 2 resolved unexpectedly');
      } catch (e) {
        return e;
      }
    })();

    // Both must be SolvelaUpstreamError — not SolvelaBudgetExceededError.
    expect(SolvelaUpstreamError.isInstance(err1)).toBe(true);
    expect(SolvelaUpstreamError.isInstance(err2)).toBe(true);

    // Both must have statusCode 500.
    expect((err1 as SolvelaUpstreamError).statusCode).toBe(500);
    expect((err2 as SolvelaUpstreamError).statusCode).toBe(500);
  });

  it('B-2. no cross-call signature leakage: sentinel absent from both errors', async () => {
    register402Then500();
    register402Then500();

    const provider = createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: makeStubWallet(MOCK_SIGNATURE),
      sessionBudget: 2625n,
    });

    const errors: SolvelaUpstreamError[] = [];
    for (const prompt of ['hello', 'world']) {
      try {
        await generateText({ model: provider('claude-sonnet-4-5'), prompt });
      } catch (err) {
        if (SolvelaUpstreamError.isInstance(err)) {
          errors.push(err);
        }
      }
    }

    expect(errors).toHaveLength(2);

    for (const err of errors) {
      // No PAYMENT-SIGNATURE in response headers.
      for (const k of Object.keys(err.responseHeaders ?? {})) {
        expect(k.toLowerCase()).not.toBe('payment-signature');
      }
      expect(err.responseHeaders?.['PAYMENT-SIGNATURE']).toBeUndefined();
      expect(err.responseHeaders?.['payment-signature']).toBeUndefined();

      // Sentinel battery on each error independently.
      expect(err.message).not.toContain(MOCK_SIGNATURE);
      expect(err.stack ?? '').not.toContain(MOCK_SIGNATURE);
      expect(String(err.cause ?? '')).not.toContain(MOCK_SIGNATURE);
      expect(err.toString()).not.toContain(MOCK_SIGNATURE);
      let serialized: string;
      try {
        serialized = JSON.stringify(err);
      } catch {
        serialized = String(err);
      }
      expect(serialized).not.toContain(MOCK_SIGNATURE);
    }
  });

  it('B-3. 4 total HTTP calls reach the mock (2 per generateText invocation)', async () => {
    register402Then500();
    register402Then500();

    const provider = createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: makeStubWallet(MOCK_SIGNATURE),
      sessionBudget: 2625n,
    });

    for (const prompt of ['hello', 'world']) {
      try {
        await generateText({ model: provider('claude-sonnet-4-5'), prompt });
      } catch {
        // Expected.
      }
    }

    expect(mock.calls).toHaveLength(4);
  });
});
