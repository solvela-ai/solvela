/**
 * IT-3: 402 once, then SSE stream — end-to-end streamText happy path.
 *
 * Assertions (per plan §6 Phase 8, success criterion S3):
 *   A. streamText returns a result with a consumable fullStream.
 *   B. The first part emitted is type 'start' (the fullStream stream-start event).
 *   C. At least one 'text-delta' part is emitted.
 *   D. The final part is type 'finish' with a finishReason field.
 *   E. Exactly 2 HTTP calls reach the mock gateway.
 *   F. Second call carries PAYMENT-SIGNATURE matching the stub wallet value.
 *   G. Assembled text matches the concatenation of the SSE content chunks.
 *
 * Transport: undici.MockAgent via installMockGateway — no real network calls.
 *
 * SSE body note:
 *   makeSSEStreamBody accepts raw data payload strings (the part after "data: ").
 *   Each chunk is a JSON-stringified OpenAI-compatible stream delta object.
 *   A delta chunk with content produces a 'text-delta' part.
 *   A chunk with finish_reason produces the finishReason recorded by the SDK.
 *   The final 'data: [DONE]\n\n' terminator is appended automatically.
 *
 * fullStream part ordering (ai@6 SDK, @ai-sdk/openai-compatible):
 *   start → start-step → text-start → text-delta(s) → text-end
 *   → finish-step → finish
 *   (response-metadata may appear between start-step and text-start)
 *   The test collects all parts and asserts order and presence constraints,
 *   not exact sequence positions, to be robust against SDK version changes.
 *
 * Note on 'stream-start' vs 'start':
 *   The plan §6 IT-3 references "stream-start" — this was written against an
 *   older/lower-level API. The actual TextStreamPart emitted by result.fullStream
 *   uses type 'start' (confirmed in ai/dist/index.d.ts TextStreamPart union and
 *   ai/dist/index.js line 6922). The assertions here use the correct 'start' type.
 */

import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { streamText } from 'ai';
import { createSolvelaProvider } from '../../src/provider.js';
import {
  installMockGateway,
  make402Envelope,
  makeSSEStreamBody,
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

/** Content strings that will become SSE delta chunks. */
const SSE_CHUNKS = ['Hello', ', ', 'world', '!'];

/**
 * Build OpenAI-compatible SSE delta chunk JSON strings.
 * Each produces a choices[0].delta.content that @ai-sdk/openai-compatible
 * maps to a 'text-delta' TextStreamPart.
 */
function makeDeltaChunk(content: string, index = 0): string {
  return JSON.stringify({
    id: 'chatcmpl-mock-stream',
    object: 'chat.completion.chunk',
    created: 1_700_000_000,
    model: 'gpt-4o',
    choices: [
      {
        index,
        delta: { role: 'assistant', content },
        finish_reason: null,
      },
    ],
  });
}

/**
 * Build a finish chunk: delta is empty, finish_reason is set.
 * @ai-sdk/openai-compatible maps this to the finishReason on the 'finish' part.
 */
function makeFinishChunk(finishReason = 'stop', index = 0): string {
  return JSON.stringify({
    id: 'chatcmpl-mock-stream',
    object: 'chat.completion.chunk',
    created: 1_700_000_000,
    model: 'gpt-4o',
    choices: [
      {
        index,
        delta: {},
        finish_reason: finishReason,
      },
    ],
  });
}

/** The expected assembled text from all SSE content chunks. */
const EXPECTED_TEXT = SSE_CHUNKS.join('');

/** Build the full SSE body: content deltas + finish chunk. */
function buildSSEBody(): string {
  const chunks = [
    ...SSE_CHUNKS.map((c) => makeDeltaChunk(c)),
    makeFinishChunk('stop'),
  ];
  return makeSSEStreamBody(chunks);
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
// Helpers
// ---------------------------------------------------------------------------

/**
 * Register the two intercepts (402 then SSE 200) and invoke streamText.
 * Returns the StreamTextResult for assertion.
 */
function setupAndStream() {
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
        data: buildSSEBody(),
        responseOptions: { headers: { 'content-type': 'text/event-stream' } },
      })),
    );

  const provider = createSolvelaProvider({
    baseURL: BASE_URL,
    wallet: makeStubWallet(MOCK_SIGNATURE),
  });

  return streamText({
    model: provider('gpt-4o'),
    prompt: 'Say hello',
  });
}

// ---------------------------------------------------------------------------
// IT-3 — Individual assertions
// ---------------------------------------------------------------------------

describe('IT-3: 402 → SSE stream happy path', () => {
  it('A. streamText returns a result object with a fullStream', async () => {
    const result = setupAndStream();

    // Consume the stream to avoid pending-interceptor errors on reset.
    // eslint-disable-next-line @typescript-eslint/no-unused-vars
    for await (const _part of result.fullStream) {
      // drain
    }

    // If streamText returned without throwing, the result has fullStream.
    expect(result).toBeDefined();
    expect(typeof result.fullStream[Symbol.asyncIterator]).toBe('function');
  });

  it('B. first part from fullStream has type "start"', async () => {
    const result = setupAndStream();

    let firstPart: { type: string } | undefined;
    for await (const part of result.fullStream) {
      firstPart = part;
      break;
    }

    // Drain remaining so reset() assertNoPendingInterceptors passes.
    // eslint-disable-next-line @typescript-eslint/no-unused-vars
    for await (const _part of result.fullStream) {
      // drain
    }

    expect(firstPart).toBeDefined();
    expect(firstPart?.type).toBe('start');
  });

  it('C. fullStream emits at least one text-delta part', async () => {
    const result = setupAndStream();

    const parts: Array<{ type: string }> = [];
    for await (const part of result.fullStream) {
      parts.push(part);
    }

    const textDeltas = parts.filter((p) => p.type === 'text-delta');
    expect(textDeltas.length).toBeGreaterThanOrEqual(1);
  });

  it('D. final part from fullStream has type "finish" with finishReason', async () => {
    const result = setupAndStream();

    const parts: Array<{ type: string; [key: string]: unknown }> = [];
    for await (const part of result.fullStream) {
      parts.push(part);
    }

    const lastPart = parts[parts.length - 1];
    expect(lastPart).toBeDefined();
    expect(lastPart?.type).toBe('finish');
    // finishReason is present on the finish part.
    expect((lastPart as { type: string; finishReason?: string }).finishReason).toBeDefined();
  });

  it('E. exactly 2 HTTP calls reach the mock gateway', async () => {
    const result = setupAndStream();

    for await (const _part of result.fullStream) {
      // drain
    }

    expect(mock.calls).toHaveLength(2);
  });

  it('F. second HTTP call carries PAYMENT-SIGNATURE matching the stub wallet', async () => {
    const result = setupAndStream();

    for await (const _part of result.fullStream) {
      // drain
    }

    expect(mock.calls[1]).toBeDefined();
    expect(getHeader(mock.calls[1], 'payment-signature')).toBe(MOCK_SIGNATURE);
  });

  it('G. assembled text matches concatenation of SSE content chunks', async () => {
    const result = setupAndStream();

    // Collect text from text-delta parts.
    let assembled = '';
    for await (const part of result.fullStream) {
      if (part.type === 'text-delta') {
        assembled += (part as { type: 'text-delta'; text: string }).text;
      }
    }

    expect(assembled).toBe(EXPECTED_TEXT);
  });
});

// ---------------------------------------------------------------------------
// IT-3 consolidated — all assertions in one test (catches ordering bugs)
// ---------------------------------------------------------------------------

describe('IT-3 consolidated: all assertions together', () => {
  it('402→SSE: start first, text-deltas present, finish last with reason, 2 calls, sig on call 2, text matches', async () => {
    const result = setupAndStream();

    // Collect all parts.
    const parts: Array<{ type: string; [key: string]: unknown }> = [];
    for await (const part of result.fullStream) {
      parts.push(part);
    }

    // B. First part is 'start'.
    expect(parts[0]?.type).toBe('start');

    // C. At least one text-delta.
    const textDeltas = parts.filter((p) => p.type === 'text-delta');
    expect(textDeltas.length).toBeGreaterThanOrEqual(1);

    // D. Last part is 'finish' with finishReason.
    const lastPart = parts[parts.length - 1];
    expect(lastPart?.type).toBe('finish');
    expect((lastPart as { finishReason?: string }).finishReason).toBeDefined();

    // E. Exactly 2 HTTP calls.
    expect(mock.calls).toHaveLength(2);

    // F. No sig on first call; sig on second call.
    expect(getHeader(mock.calls[0], 'payment-signature')).toBeUndefined();
    expect(getHeader(mock.calls[1], 'payment-signature')).toBe(MOCK_SIGNATURE);

    // G. Assembled text equals expected.
    const assembled = textDeltas
      .map((p) => (p as { text: string }).text)
      .join('');
    expect(assembled).toBe(EXPECTED_TEXT);
  });
});
