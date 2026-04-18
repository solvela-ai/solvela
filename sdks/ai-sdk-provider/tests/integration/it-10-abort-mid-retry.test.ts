/**
 * IT-10: AbortController.abort() fires cleanly mid-retry.
 *
 * Scenario (per plan §6 Phase 8 row IT-10):
 *   1. First call → mock returns 402.
 *   2. fetch-wrapper calls wallet.signPayment(); adapter completes signing and
 *      calls controller.abort() just before returning. The signal is now aborted.
 *   3. Back in fetch-wrapper, the post-sign guard (fetch-wrapper.ts:370)
 *      detects `init.signal.aborted`, calls budget.release(), calls warnOnce(),
 *      and throws an AbortError. The retry fetch is NEVER issued.
 *   4. generateText rejects with an AbortError (name === 'AbortError').
 *   5. The warn message matches /aborted mid-retry/ and does NOT contain the
 *      adapter's signature value or any 44+ char base58 substring.
 *   6. Budget was released: a second generateText on the same provider succeeds.
 *
 * Abort path used: post-sign guard (fetch-wrapper.ts §T2-E).
 *   The adapter does NOT throw — it returns a valid signature after aborting.
 *   The catch block at line ~354 is NOT exercised; the aborted-check at
 *   line ~370 is the one that trips.
 *
 * warnOnce note: warn-once.ts uses a module-scoped Set. Multiple tests in the
 *   same file sharing the same message string would only see one console.warn
 *   call total. This test file uses a SINGLE consolidated test so the assertion
 *   `toHaveBeenCalledTimes(1)` is reliable regardless of test order.
 *
 * Intercept layout:
 *   - Call 1 (aborted):     1 × 402 intercept only. Retry never fires.
 *   - Call 2 (verification): 1 × 402 + 1 × 200 intercept (like IT-1 happy path).
 *   Total: 3 intercepts registered; all consumed by reset()'s assertNoPendingInterceptors.
 *
 * Budget: sessionBudget = 2625n (exact cost of one call per make402Envelope).
 *   After the abort, release() must run; otherwise the second reserve(2625n)
 *   would fail with SolvelaBudgetExceededError (remaining = 0n < 2625n).
 *   The verification call succeeding is proof that release ran.
 *
 * Transport: undici MockAgent via installMockGateway — zero real network calls.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { generateText } from 'ai';
import { createSolvelaProvider } from '../../src/provider.js';
import {
  installMockGateway,
  make402Envelope,
  makeChatCompletionSuccess,
  makeStubWallet,
  type MockGatewayHandle,
} from './mock-gateway.js';
import type { SolvelaWalletAdapter } from '../../src/wallet-adapter.js';

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const BASE_URL = 'https://gateway.test';
const MOCK_SIGNATURE = 'mock-base64-signature==';
const INTERCEPT_PATH = '/v1/chat/completions';
const VERIFY_TEXT = 'budget released ok';

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
// IT-10
// ---------------------------------------------------------------------------

describe('IT-10: abort mid-retry — post-sign guard releases budget and warns', () => {
  it(
    'rejects with AbortError, warns once without signature bytes, and releases budget so a subsequent call succeeds',
    async () => {
      // -----------------------------------------------------------------------
      // Part 1: Aborted call
      //
      // Register exactly ONE 402 intercept. The retry fetch never fires because
      // the post-sign guard throws before it is attempted.
      // -----------------------------------------------------------------------

      const controller = new AbortController();

      // Adapter: signs normally, then aborts the signal just before returning.
      // fetch-wrapper passes init.signal to signPayment but the adapter does
      // NOT throw — it returns a valid signature. The post-sign guard
      // (fetch-wrapper.ts:370) is the code path exercised here.
      const abortingWallet: SolvelaWalletAdapter = {
        label: 'abort-during-sign',
        signPayment: async (_args) => {
          // Tiny yield so the function is genuinely async (matches real adapters).
          await new Promise<void>((resolve) => setTimeout(resolve, 5));
          // Fire the abort AFTER signing is complete — signal is now aborted.
          controller.abort();
          // Return the signature; the post-sign guard will detect the abort.
          return MOCK_SIGNATURE;
        },
      };

      // Budget sized to exactly one call cost (amount from make402Envelope: '2625').
      // If release() does not run after the abort, the second reserve(2625n)
      // will throw SolvelaBudgetExceededError (remaining 0n < 2625n).
      const provider = createSolvelaProvider({
        baseURL: BASE_URL,
        wallet: abortingWallet,
        sessionBudget: 2625n,
      });

      // Register the single 402 for the aborted call.
      mock.pool
        .intercept({ path: INTERCEPT_PATH, method: 'POST' })
        .reply(
          mock.captureReply(() => ({
            statusCode: 402,
            data: JSON.stringify(make402Envelope()),
            responseOptions: { headers: { 'content-type': 'application/json' } },
          })),
        );

      // Spy on console.warn BEFORE invoking generateText.
      const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});

      // -----------------------------------------------------------------------
      // Assertion A: generateText rejects with an AbortError.
      // -----------------------------------------------------------------------
      let abortErr: unknown;
      try {
        await generateText({
          model: provider('claude-sonnet-4-5'),
          prompt: 'hello',
          abortSignal: controller.signal,
        });
        // Should never reach here — generateText must reject.
        expect.fail('generateText should have rejected with AbortError');
      } catch (err) {
        abortErr = err;
      }

      expect(abortErr).toBeDefined();
      expect((abortErr as { name?: unknown }).name).toBe('AbortError');

      // -----------------------------------------------------------------------
      // Assertion B: console.warn called exactly once with a message matching
      // /aborted mid-retry/.
      // -----------------------------------------------------------------------
      expect(warnSpy).toHaveBeenCalledTimes(1);
      const warnMsg: string = warnSpy.mock.calls[0]?.[0] as string;
      expect(warnMsg).toMatch(/aborted mid-retry/);

      // -----------------------------------------------------------------------
      // Assertion C: warn message does NOT leak the signature value or any
      // 44+ char base58 substring.
      // -----------------------------------------------------------------------
      // The adapter's exact mock signature must not appear in the warn message.
      expect(warnMsg).not.toContain(MOCK_SIGNATURE);
      // Guard against future real-signature leaks (base58 chars, 44+ length).
      expect(warnMsg).not.toMatch(/[1-9A-HJ-NP-Za-km-z]{44,}/);

      // Restore warn spy before the verification call so we don't interfere.
      warnSpy.mockRestore();

      // -----------------------------------------------------------------------
      // Part 2: Budget-released verification call
      //
      // If budget.release() did NOT run, reserve(2625n) would throw
      // SolvelaBudgetExceededError because remaining = 0n < 2625n.
      // The call succeeding here is proof that release ran.
      //
      // Register a fresh 402 + 200 sequence (like IT-1 happy path).
      // -----------------------------------------------------------------------

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
            data: JSON.stringify(makeChatCompletionSuccess(VERIFY_TEXT)),
            responseOptions: { headers: { 'content-type': 'application/json' } },
          })),
        );

      // Use the same provider instance (same BudgetState) with a non-aborted call.
      // Replace the wallet with a plain stub so this call completes normally.
      // We cannot swap out wallet on an existing provider — create a second provider
      // sharing the same budget state is not exposed. Instead, re-use the original
      // provider whose abortingWallet will now sign normally because no abort fires
      // (controller is already spent; a new AbortController is not supplied).
      const verifyResult = await generateText({
        model: provider('claude-sonnet-4-5'),
        prompt: 'verify budget released',
        // No abortSignal — this call must run to completion.
      });

      // -----------------------------------------------------------------------
      // Assertion D: verification call succeeds with the expected reply text.
      // -----------------------------------------------------------------------
      expect(verifyResult.text).toBe(VERIFY_TEXT);
    },
    // Generous timeout: adapter has a 5ms artificial delay; allow plenty of headroom.
    10_000,
  );
});
