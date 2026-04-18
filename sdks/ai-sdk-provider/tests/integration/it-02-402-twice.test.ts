/**
 * IT-2: 402 twice — payment rejected on retry.
 *
 * Assertions (per plan §6 Phase 8):
 *   A. generateText rejects — the thrown error is a SolvelaPaymentError.
 *   B. SolvelaPaymentError.isInstance(err) is true.
 *   C. Error message matches "Payment rejected after retry".
 *   D. Exactly 2 HTTP calls reach the mock gateway (no infinite retry).
 *   E. Second HTTP call carries PAYMENT-SIGNATURE header.
 *   F. Sentinel signature value is absent from err.message, err.stack, and
 *      JSON.stringify(err) (signed header never leaks into error surface).
 *
 * Transport: undici.MockAgent via installMockGateway — no real network calls.
 *
 * Sentinel note:
 *   The wallet stub uses a distinctive value 'SENTINEL-IT02-DO-NOT-LEAK-x7K9Qz=='
 *   rather than the default 'mock-base64-signature=='.  Assertion F verifies that
 *   this exact string does not appear anywhere in the thrown error, proving the
 *   fetch-wrapper strips PAYMENT-SIGNATURE from all error surfaces before throwing
 *   SolvelaPaymentError (Sec-8 / T1-C).
 *
 * Source-of-truth for error message:
 *   fetch-wrapper.ts:419 — `message: 'Payment rejected after retry'`
 *   This is a literal string; no escaping required in the assertion.
 */

import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { generateText } from 'ai';
import { createSolvelaProvider } from '../../src/provider.js';
import { SolvelaPaymentError } from '../../src/errors.js';
import {
  installMockGateway,
  make402Envelope,
  makeStubWallet,
  getHeader,
  type MockGatewayHandle,
} from './mock-gateway.js';

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const BASE_URL = 'https://gateway.test';
/**
 * A distinctive sentinel that cannot plausibly appear in framework internals.
 * Used to verify the signed header value never leaks into the error surface.
 */
const SENTINEL_SIGNATURE = 'SENTINEL-IT02-DO-NOT-LEAK-x7K9Qz==';
const INTERCEPT_PATH = '/v1/chat/completions';

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
// Shared helper — registers two 402 intercepts and runs generateText
// ---------------------------------------------------------------------------

/**
 * Register two 402 intercepts then call generateText, capturing the thrown
 * error.  Returns the raw caught value (typed `unknown`) so individual test
 * assertions can each inspect it.
 *
 * Uses try/catch rather than `.rejects` so we hold the error *instance* for
 * SolvelaPaymentError.isInstance(), err.stack, and JSON.stringify(err) checks.
 */
async function runAndCapture(): Promise<unknown> {
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
        statusCode: 402,
        data: JSON.stringify(make402Envelope()),
        responseOptions: { headers: { 'content-type': 'application/json' } },
      })),
    );

  const provider = createSolvelaProvider({
    baseURL: BASE_URL,
    wallet: makeStubWallet(SENTINEL_SIGNATURE),
  });

  let caught: unknown;
  try {
    await generateText({
      model: provider('claude-sonnet-4-5'),
      prompt: 'hello',
    });
  } catch (err) {
    caught = err;
  }
  return caught;
}

// ---------------------------------------------------------------------------
// IT-2 — per-assertion tests
// ---------------------------------------------------------------------------

describe('IT-2: 402 → 402 rejected payment path', () => {
  it('A. generateText rejects — the thrown error is a SolvelaPaymentError', async () => {
    const err = await runAndCapture();
    // Must have thrown — if undefined, generateText resolved unexpectedly.
    expect(err).toBeDefined();
    // Walk err.cause in case the AI SDK wraps the error.
    const actual =
      SolvelaPaymentError.isInstance(err)
        ? err
        : SolvelaPaymentError.isInstance((err as { cause?: unknown })?.cause)
          ? (err as { cause: unknown }).cause
          : err;
    expect(SolvelaPaymentError.isInstance(actual)).toBe(true);
  });

  it('B. SolvelaPaymentError.isInstance(err) is true', async () => {
    const err = await runAndCapture();
    const actual =
      SolvelaPaymentError.isInstance(err)
        ? err
        : SolvelaPaymentError.isInstance((err as { cause?: unknown })?.cause)
          ? (err as { cause: unknown }).cause
          : err;
    expect(SolvelaPaymentError.isInstance(actual)).toBe(true);
  });

  it('C. error message matches "Payment rejected after retry"', async () => {
    const err = await runAndCapture();
    const actual =
      SolvelaPaymentError.isInstance(err)
        ? err
        : SolvelaPaymentError.isInstance((err as { cause?: unknown })?.cause)
          ? (err as { cause: unknown }).cause as SolvelaPaymentError
          : err;
    expect((actual as { message: string }).message).toContain(
      'Payment rejected after retry',
    );
  });

  it('D. exactly 2 HTTP calls reach the mock gateway (no infinite retry)', async () => {
    await runAndCapture();
    expect(mock.calls).toHaveLength(2);
  });

  it('E. second HTTP call carries PAYMENT-SIGNATURE header', async () => {
    await runAndCapture();
    expect(mock.calls[1]).toBeDefined();
    // The retry (second call) must carry the wallet's signature.
    expect(getHeader(mock.calls[1], 'payment-signature')).toBe(SENTINEL_SIGNATURE);
  });

  it('F. sentinel signature value is absent from err.message, err.stack, and JSON.stringify(err)', async () => {
    const err = await runAndCapture();
    expect(err).toBeDefined();

    const message = (err as { message?: string })?.message ?? '';
    const stack = (err as { stack?: string })?.stack ?? '';
    let serialized: string;
    try {
      serialized = JSON.stringify(err);
    } catch {
      serialized = '';
    }

    expect(message).not.toContain(SENTINEL_SIGNATURE);
    expect(stack).not.toContain(SENTINEL_SIGNATURE);
    expect(serialized).not.toContain(SENTINEL_SIGNATURE);
  });
});

// ---------------------------------------------------------------------------
// IT-2 consolidated (all six assertions in one test — catches ordering bugs)
// ---------------------------------------------------------------------------

describe('IT-2 consolidated: all six assertions together', () => {
  it('402→402: throws SolvelaPaymentError, 2 calls, sig on call 2, no sentinel leak', async () => {
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
          statusCode: 402,
          data: JSON.stringify(make402Envelope()),
          responseOptions: { headers: { 'content-type': 'application/json' } },
        })),
      );

    const provider = createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: makeStubWallet(SENTINEL_SIGNATURE),
    });

    let err: unknown;
    try {
      await generateText({
        model: provider('claude-sonnet-4-5'),
        prompt: 'hello',
      });
    } catch (e) {
      err = e;
    }

    // Must have thrown.
    expect(err).toBeDefined();

    // Resolve the error instance (direct or wrapped).
    const actual =
      SolvelaPaymentError.isInstance(err)
        ? err
        : SolvelaPaymentError.isInstance((err as { cause?: unknown })?.cause)
          ? (err as { cause: unknown }).cause as SolvelaPaymentError
          : err;

    // A. generateText threw.
    expect(actual).toBeDefined();

    // B. SolvelaPaymentError.isInstance
    expect(SolvelaPaymentError.isInstance(actual)).toBe(true);

    // C. message
    expect((actual as { message: string }).message).toContain(
      'Payment rejected after retry',
    );

    // D. exactly 2 calls
    expect(mock.calls).toHaveLength(2);

    // E. second call carries PAYMENT-SIGNATURE
    expect(mock.calls[1]).toBeDefined();
    expect(getHeader(mock.calls[1], 'payment-signature')).toBe(SENTINEL_SIGNATURE);

    // F. sentinel absent from all error surfaces
    const message = (err as { message?: string })?.message ?? '';
    const stack = (err as { stack?: string })?.stack ?? '';
    let serialized: string;
    try {
      serialized = JSON.stringify(err);
    } catch {
      serialized = '';
    }
    expect(message).not.toContain(SENTINEL_SIGNATURE);
    expect(stack).not.toContain(SENTINEL_SIGNATURE);
    expect(serialized).not.toContain(SENTINEL_SIGNATURE);
  });
});
