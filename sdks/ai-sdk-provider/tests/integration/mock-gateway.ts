/**
 * Shared mock-gateway helper for Solvela AI SDK provider integration tests.
 *
 * Design contract for follow-up agents (IT-2..IT-13):
 *
 *   1. Call `installMockGateway(baseUrl)` once per test file (or per test if
 *      isolation is needed). It sets undici's global dispatcher and returns
 *      `{ pool, agent, calls, reset }`.
 *
 *   2. Register intercepts on `mock.pool` using the CALLBACK form so that
 *      every request is captured in `mock.calls`:
 *
 *       mock.pool.intercept({ path: '/v1/chat/completions', method: 'POST' })
 *         .reply(mock.captureReply(() => ({
 *           statusCode: 402,
 *           data: make402Envelope(),
 *           responseOptions: { headers: { 'content-type': 'application/json' } },
 *         })));
 *
 *   3. After the test, call `await mock.reset()` (or put it in `afterEach`).
 *      reset() calls assertNoPendingInterceptors(), restores the previous
 *      global dispatcher, and closes the agent.
 *
 *   4. Inspect captured calls via `mock.calls`:
 *       - `mock.calls[0].headers['payment-signature']` → undefined (first leg)
 *       - `mock.calls[1].headers['payment-signature']` → 'mock-base64-signature=='
 *
 * Header note: undici delivers request headers to the callback as a plain
 * lowercase-keyed Record<string, string>. Always compare header names in
 * lowercase (or use `getHeader(call, 'payment-signature')`).
 *
 * baseURL normalization note: createSolvelaProvider appends /v1 if absent,
 * and @ai-sdk/openai-compatible appends /chat/completions. So:
 *   installMockGateway('https://gateway.test')  →  intercept path '/v1/chat/completions'
 *   agent.get('https://gateway.test')           →  origin 'https://gateway.test'
 *
 * SSE note: makeSSEStreamBody() returns a plain string accepted directly by
 * MockInterceptor.reply() as the `data` argument. Pass content-type
 * 'text/event-stream' in responseOptions.headers.
 */

import {
  MockAgent,
  getGlobalDispatcher,
  setGlobalDispatcher,
  type Dispatcher,
} from 'undici';
import type { Interceptable, MockInterceptor } from 'undici/types/mock-interceptor';

import type { SolvelaWalletAdapter, SolvelaPaymentRequired } from '../../src/wallet-adapter.js';

// ---------------------------------------------------------------------------
// Captured call record
// ---------------------------------------------------------------------------

/**
 * One captured HTTP call as seen by the mock gateway.
 * headers are always lowercase-keyed (undici normalizes them).
 */
export interface CapturedCall {
  path: string;
  method: string;
  /** Lowercase-keyed request headers delivered by undici. */
  headers: Record<string, string>;
  /** Raw request body string (or undefined if no body was sent). */
  body: string | undefined;
}

// ---------------------------------------------------------------------------
// Convenience header accessor (case-insensitive)
// ---------------------------------------------------------------------------

/**
 * Get a request header value case-insensitively from a CapturedCall.
 * Returns undefined if the header is absent.
 *
 * @example
 * const sig = getHeader(mock.calls[1], 'payment-signature');
 */
export function getHeader(
  call: CapturedCall,
  name: string,
): string | undefined {
  return call.headers[name.toLowerCase()];
}

// ---------------------------------------------------------------------------
// Mock gateway handle returned by installMockGateway
// ---------------------------------------------------------------------------

export interface MockGatewayHandle {
  /** The Interceptable pool — register intercepts here. */
  pool: Interceptable;
  /** The MockAgent — rarely needed directly; prefer pool. */
  agent: MockAgent;
  /**
   * All HTTP calls captured so far, in arrival order.
   * Populated automatically by captureReply() wrappers.
   * Mutate with caution; reset() clears this array in place.
   */
  calls: CapturedCall[];
  /**
   * Wrap a reply-options producer so the call is recorded in `calls[]`.
   *
   * Usage:
   *   pool.intercept({ path: '/v1/chat/completions', method: 'POST' })
   *     .reply(mock.captureReply(() => ({
   *       statusCode: 402,
   *       data: make402Envelope(),
   *       responseOptions: { headers: { 'content-type': 'application/json' } },
   *     })));
   *
   * The inner function receives the same MockResponseCallbackOptions that
   * undici passes to the raw callback, so you can branch on path/headers
   * inside it if needed.
   */
  captureReply(
    producer: (
      opts: MockInterceptor.MockResponseCallbackOptions,
    ) => MockInterceptor.MockReplyOptionsCallback<object> extends (
      opts: MockInterceptor.MockResponseCallbackOptions,
    ) => infer R
      ? R
      : never,
  ): MockInterceptor.MockReplyOptionsCallback<object>;
  /**
   * Tear down: assert no pending interceptors, restore the previous global
   * dispatcher, close the agent, and clear `calls[]`.
   * Safe to call multiple times; subsequent calls are no-ops.
   */
  reset: () => Promise<void>;
}

// ---------------------------------------------------------------------------
// installMockGateway
// ---------------------------------------------------------------------------

/**
 * Install a fresh MockAgent as the global undici dispatcher.
 *
 * @param baseUrl  Origin to intercept (e.g. 'https://gateway.test').
 *                 Must match the first argument of `createSolvelaProvider({ baseURL })`.
 *                 Note: do NOT include the /v1 suffix — the provider appends it.
 *
 * @example
 * ```typescript
 * const mock = installMockGateway('https://gateway.test');
 *
 * mock.pool
 *   .intercept({ path: '/v1/chat/completions', method: 'POST' })
 *   .reply(mock.captureReply(() => ({
 *     statusCode: 402,
 *     data: make402Envelope(),
 *     responseOptions: { headers: { 'content-type': 'application/json' } },
 *   })));
 *
 * afterEach(mock.reset);
 * ```
 */
export function installMockGateway(baseUrl: string): MockGatewayHandle {
  const previousDispatcher: Dispatcher = getGlobalDispatcher();
  const agent = new MockAgent({ connections: 1 });
  agent.disableNetConnect();
  setGlobalDispatcher(agent);

  // Extract just the origin (scheme + host + optional port) for agent.get().
  const origin = new URL(baseUrl).origin;
  const pool = agent.get<Interceptable>(origin);

  const calls: CapturedCall[] = [];
  let tornDown = false;

  function captureReply(
    producer: (
      opts: MockInterceptor.MockResponseCallbackOptions,
    ) => ReturnType<MockInterceptor.MockReplyOptionsCallback<object>>,
  ): MockInterceptor.MockReplyOptionsCallback<object> {
    return (opts: MockInterceptor.MockResponseCallbackOptions) => {
      // undici preserves the caller's original header casing (e.g. PAYMENT-SIGNATURE
      // arrives as-is, not lowercased). Normalise to lowercase here so that
      // getHeader() and scenario assertions work uniformly regardless of what
      // casing the fetch-wrapper or @ai-sdk/openai-compatible sends.
      const rawHeaders = (opts.headers ?? {}) as Record<string, string>;
      const normalizedHeaders: Record<string, string> = {};
      for (const [k, v] of Object.entries(rawHeaders)) {
        normalizedHeaders[k.toLowerCase()] = v;
      }

      // body may be a string, Buffer, or null; normalise to string | undefined
      let bodyStr: string | undefined;
      if (typeof opts.body === 'string') {
        bodyStr = opts.body;
      } else if (opts.body instanceof Buffer) {
        bodyStr = opts.body.toString('utf-8');
      } else if (opts.body != null) {
        // ArrayBuffer, Uint8Array, ReadableStream, etc — best-effort
        try {
          bodyStr = String(opts.body);
        } catch {
          bodyStr = undefined;
        }
      }

      calls.push({
        path: opts.path,
        method: opts.method,
        headers: normalizedHeaders,
        body: bodyStr,
      });

      return producer(opts);
    };
  }

  async function reset(): Promise<void> {
    if (tornDown) return;
    tornDown = true;
    // Surface un-consumed interceptors as test failures.
    try {
      agent.assertNoPendingInterceptors();
    } finally {
      setGlobalDispatcher(previousDispatcher);
      await agent.close();
      calls.length = 0;
    }
  }

  return { pool, agent, calls, captureReply, reset };
}

// ---------------------------------------------------------------------------
// 402 envelope builder
// ---------------------------------------------------------------------------

/**
 * Subset of SolvelaPaymentRequired fields that can be overridden in tests.
 */
export interface PaymentRequiredOverrides {
  /** Override the first `accepts[]` entry amount (atomic USDC units string). */
  amount?: string;
  /** Override the first `accepts[]` pay_to wallet. */
  pay_to?: string;
  /** Override cost_breakdown.total decimal string. */
  total?: string;
  /** Fully replace the accepts array. */
  accepts?: SolvelaPaymentRequired['accepts'];
}

/**
 * Build a valid 402 gateway envelope body matching the shape of
 * `tests/fixtures/402-envelope.json`.
 *
 * The inner `error.message` is a JSON-stringified SolvelaPaymentRequired, which
 * is what the fetch-wrapper parses.
 *
 * @param overrides  Optional field overrides for payment parameters.
 * @returns          Plain object suitable for `.reply(statusCode, body)`.
 */
export function make402Envelope(overrides?: PaymentRequiredOverrides): object {
  const amount = overrides?.amount ?? '2625';
  const pay_to = overrides?.pay_to ?? 'RecipientWalletPubkeyHere';
  const total = overrides?.total ?? '0.002625';

  const paymentRequired: SolvelaPaymentRequired = {
    x402_version: 2,
    resource: { url: '/v1/chat/completions', method: 'POST' },
    accepts: overrides?.accepts ?? [
      {
        scheme: 'exact',
        network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
        amount,
        asset: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
        pay_to,
        max_timeout_seconds: 300,
      },
    ],
    cost_breakdown: {
      provider_cost: '0.002500',
      platform_fee: '0.000125',
      total,
      currency: 'USDC',
      fee_percent: 5,
    },
    error: 'Payment required',
  };

  return {
    error: {
      type: 'invalid_payment',
      message: JSON.stringify(paymentRequired),
    },
  };
}

// ---------------------------------------------------------------------------
// Chat completion 200 response builder
// ---------------------------------------------------------------------------

/**
 * Build a minimal OpenAI-compatible chat completion response body.
 *
 * @param text  The assistant reply text (becomes `choices[0].message.content`).
 * @returns     Plain object accepted by `.reply(200, body)`.
 */
export function makeChatCompletionSuccess(text: string): object {
  return {
    id: 'chatcmpl-mock-integration',
    object: 'chat.completion',
    created: 1_700_000_000,
    model: 'claude-sonnet-4-5',
    choices: [
      {
        index: 0,
        message: { role: 'assistant', content: text },
        finish_reason: 'stop',
        logprobs: null,
      },
    ],
    usage: {
      prompt_tokens: 10,
      completion_tokens: 2,
      total_tokens: 12,
    },
  };
}

// ---------------------------------------------------------------------------
// SSE stream body builder
// ---------------------------------------------------------------------------

/**
 * Build a valid SSE body string for streaming text-delta scenarios (IT-3, IT-6, IT-11).
 *
 * Each chunk string is emitted as `data: <chunk>\n\n`. A final `data: [DONE]\n\n`
 * terminator is always appended.
 *
 * Usage with MockInterceptor:
 *   pool.intercept({ path: '/v1/chat/completions', method: 'POST' })
 *     .reply(mock.captureReply(() => ({
 *       statusCode: 200,
 *       data: makeSSEStreamBody(['{"choices":[{"delta":{"content":"hi"},"index":0}]}']),
 *       responseOptions: { headers: { 'content-type': 'text/event-stream' } },
 *     })));
 *
 * @param chunks  Array of raw data payloads (the part after `data: `).
 *                For OpenAI-compatible streaming, pass JSON-stringified delta objects.
 * @returns       Plain string that undici MockAgent forwards as the response body.
 */
export function makeSSEStreamBody(chunks: string[]): string {
  const lines = chunks.map((c) => `data: ${c}\n\n`);
  lines.push('data: [DONE]\n\n');
  return lines.join('');
}

// ---------------------------------------------------------------------------
// Stub wallet adapter builder
// ---------------------------------------------------------------------------

/**
 * Build a minimal SolvelaWalletAdapter whose signPayment always resolves with
 * the given signature value.
 *
 * The returned adapter is a plain object (not a class instance) — compatible
 * with SolvelaWalletAdapter structural typing.
 *
 * @param signatureValue  Base64 signature string returned by signPayment.
 *                        Defaults to `'mock-base64-signature=='`.
 *
 * @example
 * const solvela = createSolvelaProvider({
 *   baseURL: 'https://gateway.test',
 *   wallet: makeStubWallet(),
 * });
 */
export function makeStubWallet(
  signatureValue = 'mock-base64-signature==',
): SolvelaWalletAdapter {
  return {
    label: 'stub-wallet',
    signPayment: (_args) => Promise.resolve(signatureValue),
  };
}
