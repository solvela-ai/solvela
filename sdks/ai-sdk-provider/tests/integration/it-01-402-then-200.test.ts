/**
 * IT-1: 402 once, then 200 — end-to-end generateText happy path.
 *
 * Assertions (per plan §6 Phase 8):
 *   A. generateText resolves with text === 'hello world'.
 *   B. Exactly 2 HTTP calls reach the mock gateway.
 *   C. First call carries no PAYMENT-SIGNATURE header.
 *   D. Second call carries PAYMENT-SIGNATURE with the stub wallet's value verbatim.
 *
 * Transport: undici.MockAgent via installMockGateway — no real network calls.
 *
 * Note on baseURL:
 *   createSolvelaProvider is given baseURL 'https://gateway.test' (no /v1 suffix).
 *   config.ts normalizeBaseURL appends /v1 → 'https://gateway.test/v1'.
 *   @ai-sdk/openai-compatible then appends /chat/completions.
 *   Final URL: 'https://gateway.test/v1/chat/completions'.
 *   MockAgent intercept path: '/v1/chat/completions'.
 *   MockAgent origin (agent.get): 'https://gateway.test'.
 */

import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { generateText } from 'ai';
import { createSolvelaProvider } from '../../src/provider.js';
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
// IT-1
// ---------------------------------------------------------------------------

describe('IT-1: 402 → 200 happy path', () => {
  it('A. generateText resolves with the expected text from the 200 response', async () => {
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
    });

    const result = await generateText({
      model: provider('claude-sonnet-4-5'),
      prompt: 'hello',
    });

    expect(result.text).toBe(REPLY_TEXT);
  });

  it('B. exactly 2 HTTP calls reach the mock gateway', async () => {
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
    });

    await generateText({
      model: provider('claude-sonnet-4-5'),
      prompt: 'hello',
    });

    expect(mock.calls).toHaveLength(2);
  });

  it('C. first HTTP call carries no PAYMENT-SIGNATURE header', async () => {
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
    });

    await generateText({
      model: provider('claude-sonnet-4-5'),
      prompt: 'hello',
    });

    expect(mock.calls[0]).toBeDefined();
    // Header must be absent on the first (unsigned) attempt.
    expect(getHeader(mock.calls[0], 'payment-signature')).toBeUndefined();
  });

  it('D. second HTTP call carries PAYMENT-SIGNATURE matching the stub wallet verbatim', async () => {
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
    });

    await generateText({
      model: provider('claude-sonnet-4-5'),
      prompt: 'hello',
    });

    expect(mock.calls[1]).toBeDefined();
    // The retry must carry the exact signature returned by signPayment.
    expect(getHeader(mock.calls[1], 'payment-signature')).toBe(MOCK_SIGNATURE);
  });
});

// ---------------------------------------------------------------------------
// Consolidated IT-1 (all four assertions in one test — catches ordering bugs)
// ---------------------------------------------------------------------------

describe('IT-1 consolidated: all four assertions together', () => {
  it('402→200: text correct, 2 calls, no sig on call 1, sig on call 2', async () => {
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
    });

    const result = await generateText({
      model: provider('claude-sonnet-4-5'),
      prompt: 'hello',
    });

    // A. correct text
    expect(result.text).toBe(REPLY_TEXT);
    // B. exactly 2 calls
    expect(mock.calls).toHaveLength(2);
    // C. no sig on first call
    expect(getHeader(mock.calls[0], 'payment-signature')).toBeUndefined();
    // D. correct sig on second call
    expect(getHeader(mock.calls[1], 'payment-signature')).toBe(MOCK_SIGNATURE);
  });
});
