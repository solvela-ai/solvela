/**
 * IT-9: User-supplied custom SolvelaWalletAdapter — end-to-end callback contract.
 *
 * Contrast with IT-1 (which used makeStubWallet from mock-gateway.ts):
 *   IT-1 proved the 402→sign→retry loop works with the shared test stub.
 *   IT-9 proves a hand-rolled adapter written by any user works correctly:
 *     - signPayment is invoked exactly once.
 *     - The args object carries the correctly parsed paymentRequired envelope.
 *     - resourceUrl matches the URL the wrapper was invoked with.
 *     - requestBody is a string (the OpenAI chat request JSON).
 *     - signal is either undefined or a valid AbortSignal.
 *     - The signature the custom adapter returns reaches the PAYMENT-SIGNATURE
 *       header on the retry verbatim (proves user-returned value is forwarded
 *       without transformation).
 *
 * Assertions (per plan §6 Phase 8, row IT-9):
 *   A. generateText resolves successfully.
 *   B. capturedArgs.length === 1 (signPayment called exactly once).
 *   C. capturedArgs[0].paymentRequired matches the parsed 402 envelope shape
 *      (x402_version, resource, accepts[0] key fields, cost_breakdown key fields).
 *   D. capturedArgs[0].resourceUrl matches the URL the wrapper was called with.
 *   E. capturedArgs[0].requestBody is a string.
 *   F. capturedArgs[0].signal is either undefined or an AbortSignal.
 *   G. Second intercept's PAYMENT-SIGNATURE header value is exactly
 *      'custom-base64-signature-xyz==' (proves adapter return reached retry).
 *
 * Transport: undici.MockAgent via installMockGateway — no real network calls.
 *
 * Note on resourceUrl:
 *   createSolvelaProvider is given baseURL 'https://gateway.test' (no /v1 suffix).
 *   config.ts normalizeBaseURL appends /v1 → 'https://gateway.test/v1'.
 *   @ai-sdk/openai-compatible appends /chat/completions.
 *   fetch-wrapper receives the full URL: 'https://gateway.test/v1/chat/completions'.
 *   That same URL is forwarded verbatim to signPayment as resourceUrl.
 */

import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { generateText } from 'ai';
import { createSolvelaProvider } from '../../src/provider.js';
import type { SolvelaWalletAdapter } from '../../src/wallet-adapter.js';
import {
  installMockGateway,
  make402Envelope,
  makeChatCompletionSuccess,
  getHeader,
  type MockGatewayHandle,
} from './mock-gateway.js';

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const BASE_URL = 'https://gateway.test';
/** The full URL that the fetch-wrapper (and therefore signPayment) sees. */
const EXPECTED_RESOURCE_URL = 'https://gateway.test/v1/chat/completions';
const INTERCEPT_PATH = '/v1/chat/completions';
const CUSTOM_SIGNATURE = 'custom-base64-signature-xyz==';
const REPLY_TEXT = 'custom adapter reply';

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
// Custom adapter factory
//
// Constructed fresh per test inside each describe block so capturedArgs is
// isolated (no shared state between tests).
// ---------------------------------------------------------------------------

function makeCustomAdapter(): {
  adapter: SolvelaWalletAdapter;
  capturedArgs: Parameters<SolvelaWalletAdapter['signPayment']>[];
} {
  const capturedArgs: Parameters<SolvelaWalletAdapter['signPayment']>[] = [];

  const adapter: SolvelaWalletAdapter = {
    label: 'it9-custom-adapter',
    async signPayment(args) {
      capturedArgs.push(args);
      return CUSTOM_SIGNATURE;
    },
  };

  return { adapter, capturedArgs };
}

// ---------------------------------------------------------------------------
// Helper: register the standard 402 → 200 intercept pair
// ---------------------------------------------------------------------------

function registerInterceptPair(): void {
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
}

// ---------------------------------------------------------------------------
// IT-9 individual assertions
// ---------------------------------------------------------------------------

describe('IT-9: custom SolvelaWalletAdapter — individual assertions', () => {
  it('A. generateText resolves successfully with the custom adapter', async () => {
    registerInterceptPair();
    const { adapter } = makeCustomAdapter();

    const provider = createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: adapter,
    });

    const result = await generateText({
      model: provider('claude-sonnet-4-5'),
      prompt: 'hello from custom adapter',
    });

    expect(result.text).toBe(REPLY_TEXT);
  });

  it('B. signPayment is invoked exactly once', async () => {
    registerInterceptPair();
    const { adapter, capturedArgs } = makeCustomAdapter();

    const provider = createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: adapter,
    });

    await generateText({
      model: provider('claude-sonnet-4-5'),
      prompt: 'hello from custom adapter',
    });

    expect(capturedArgs).toHaveLength(1);
  });

  it('C. capturedArgs[0].paymentRequired matches the parsed 402 envelope shape', async () => {
    registerInterceptPair();
    const { adapter, capturedArgs } = makeCustomAdapter();

    const provider = createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: adapter,
    });

    await generateText({
      model: provider('claude-sonnet-4-5'),
      prompt: 'hello from custom adapter',
    });

    const pr = capturedArgs[0]!.paymentRequired;

    // Top-level fields from the allowlisted ParsedPaymentRequired.
    expect(pr.x402_version).toBe(2);
    expect(pr.error).toBe('Payment required');

    // resource object — matches make402Envelope() defaults.
    expect(pr.resource).toMatchObject({ url: '/v1/chat/completions', method: 'POST' });

    // accepts array — first entry key fields.
    expect(pr.accepts).toHaveLength(1);
    expect(pr.accepts[0]).toMatchObject({
      scheme: 'exact',
      network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
      amount: '2625',
      asset: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
      pay_to: 'RecipientWalletPubkeyHere',
      max_timeout_seconds: 300,
    });

    // cost_breakdown key fields.
    expect(pr.cost_breakdown).toMatchObject({
      provider_cost: '0.002500',
      platform_fee: '0.000125',
      total: '0.002625',
      currency: 'USDC',
      fee_percent: 5,
    });
  });

  it('D. capturedArgs[0].resourceUrl matches the full request URL', async () => {
    registerInterceptPair();
    const { adapter, capturedArgs } = makeCustomAdapter();

    const provider = createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: adapter,
    });

    await generateText({
      model: provider('claude-sonnet-4-5'),
      prompt: 'hello from custom adapter',
    });

    expect(capturedArgs[0]!.resourceUrl).toBe(EXPECTED_RESOURCE_URL);
  });

  it('E. capturedArgs[0].requestBody is a string', async () => {
    registerInterceptPair();
    const { adapter, capturedArgs } = makeCustomAdapter();

    const provider = createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: adapter,
    });

    await generateText({
      model: provider('claude-sonnet-4-5'),
      prompt: 'hello from custom adapter',
    });

    expect(typeof capturedArgs[0]!.requestBody).toBe('string');
  });

  it('F. capturedArgs[0].signal is either undefined or a valid AbortSignal', async () => {
    registerInterceptPair();
    const { adapter, capturedArgs } = makeCustomAdapter();

    const provider = createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: adapter,
    });

    await generateText({
      model: provider('claude-sonnet-4-5'),
      prompt: 'hello from custom adapter',
    });

    const sig = capturedArgs[0]!.signal;
    const isUndefinedOrAbortSignal =
      sig === undefined ||
      (typeof sig === 'object' && sig !== null && 'aborted' in sig);
    expect(isUndefinedOrAbortSignal).toBe(true);
  });

  it('G. PAYMENT-SIGNATURE on retry carries the custom adapter signature verbatim', async () => {
    registerInterceptPair();
    const { adapter } = makeCustomAdapter();

    const provider = createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: adapter,
    });

    await generateText({
      model: provider('claude-sonnet-4-5'),
      prompt: 'hello from custom adapter',
    });

    // mock.calls[1] is the retry (second HTTP call).
    expect(mock.calls[1]).toBeDefined();
    expect(getHeader(mock.calls[1]!, 'payment-signature')).toBe(CUSTOM_SIGNATURE);
  });
});

// ---------------------------------------------------------------------------
// IT-9 consolidated: all assertions in one test (catches ordering regressions)
// ---------------------------------------------------------------------------

describe('IT-9 consolidated: all assertions together', () => {
  it('custom adapter: signPayment args correct, signature forwarded verbatim', async () => {
    registerInterceptPair();
    const { adapter, capturedArgs } = makeCustomAdapter();

    const provider = createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: adapter,
    });

    const result = await generateText({
      model: provider('claude-sonnet-4-5'),
      prompt: 'hello from custom adapter',
    });

    // A. resolves successfully
    expect(result.text).toBe(REPLY_TEXT);

    // B. exactly one signPayment call
    expect(capturedArgs).toHaveLength(1);

    const args = capturedArgs[0]!;

    // C. paymentRequired envelope shape
    const pr = args.paymentRequired;
    expect(pr.x402_version).toBe(2);
    expect(pr.error).toBe('Payment required');
    expect(pr.resource).toMatchObject({ url: '/v1/chat/completions', method: 'POST' });
    expect(pr.accepts).toHaveLength(1);
    expect(pr.accepts[0]).toMatchObject({
      scheme: 'exact',
      network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
      amount: '2625',
      asset: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
      pay_to: 'RecipientWalletPubkeyHere',
      max_timeout_seconds: 300,
    });
    expect(pr.cost_breakdown).toMatchObject({
      provider_cost: '0.002500',
      platform_fee: '0.000125',
      total: '0.002625',
      currency: 'USDC',
      fee_percent: 5,
    });

    // D. resourceUrl is the full URL the fetch-wrapper received
    expect(args.resourceUrl).toBe(EXPECTED_RESOURCE_URL);

    // E. requestBody is a string
    expect(typeof args.requestBody).toBe('string');

    // F. signal is undefined or AbortSignal
    const sig = args.signal;
    const isUndefinedOrAbortSignal =
      sig === undefined ||
      (typeof sig === 'object' && sig !== null && 'aborted' in sig);
    expect(isUndefinedOrAbortSignal).toBe(true);

    // G. retry carries the custom signature verbatim
    expect(mock.calls[1]).toBeDefined();
    expect(getHeader(mock.calls[1]!, 'payment-signature')).toBe(CUSTOM_SIGNATURE);
  });
});
