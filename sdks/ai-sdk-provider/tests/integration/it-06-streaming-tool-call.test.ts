/**
 * IT-6: Streaming tool call — part ordering and argument assembly.
 *
 * Assertions (per plan §6 Phase 8, row IT-6):
 *   A. Stream parts include tool-input-start, tool-input-delta (1+),
 *      tool-input-end, and tool-call emitted in that strict order.
 *   B. The tool-call part's .input assembles to { x: 42 }.
 *   C. finish part appears after tool-call.
 *   D. Exactly 2 HTTP calls reach the mock gateway (402 then SSE 200).
 *
 * Transport: undici.MockAgent via installMockGateway — no real network calls.
 *
 * SSE shape: OpenAI-compatible tool-call delta stream.
 *   Chunk 1: tool_calls[0] with id, type, function.name and empty arguments
 *   Chunk 2: delta with partial arguments '{"x'
 *   Chunk 3: delta with remaining arguments '":42}'
 *   Chunk 4: empty delta with finish_reason 'tool_calls'
 *
 * Note on tool definition:
 *   No `execute` callback is provided so streamText stops after one round-trip,
 *   keeping the HTTP call count at exactly 2 (402 + 200-SSE).
 *
 * Note on field names (verified against node_modules/ai/dist/index.d.ts):
 *   - tool() uses `inputSchema` (not `parameters`)
 *   - tool-call part carries `.input` (not `.args`) per StaticToolCall<TOOLS>
 */

import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { streamText, tool } from 'ai';
import { jsonSchema } from '@ai-sdk/provider-utils';
import { createSolvelaProvider } from '../../src/provider.js';
import {
  installMockGateway,
  make402Envelope,
  makeSSEStreamBody,
  makeStubWallet,
  type MockGatewayHandle,
} from './mock-gateway.js';

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const BASE_URL = 'https://gateway.test';
const MOCK_SIGNATURE = 'mock-base64-signature==';
const INTERCEPT_PATH = '/v1/chat/completions';

// SSE chunks mimic OpenAI tool-call delta stream.
// Each is a JSON object passed to makeSSEStreamBody; [DONE] is auto-appended.
const TOOL_CALL_SSE_CHUNKS: string[] = [
  JSON.stringify({
    choices: [{
      delta: {
        tool_calls: [{
          index: 0,
          id: 'call_1',
          type: 'function',
          function: { name: 'foo', arguments: '' },
        }],
      },
    }],
  }),
  JSON.stringify({
    choices: [{
      delta: {
        tool_calls: [{ index: 0, function: { arguments: '{"x' } }],
      },
    }],
  }),
  JSON.stringify({
    choices: [{
      delta: {
        tool_calls: [{ index: 0, function: { arguments: '":42}' } }],
      },
    }],
  }),
  JSON.stringify({
    choices: [{
      delta: {},
      finish_reason: 'tool_calls',
    }],
  }),
];

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
// Helper: register the standard 402-then-SSE-200 intercept pair
// ---------------------------------------------------------------------------

function registerIntercepts(): void {
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
        data: makeSSEStreamBody(TOOL_CALL_SSE_CHUNKS),
        responseOptions: { headers: { 'content-type': 'text/event-stream' } },
      })),
    );
}

// ---------------------------------------------------------------------------
// Helper: build provider + collect all fullStream parts
// ---------------------------------------------------------------------------

async function runStream(): Promise<Array<{ type: string; [k: string]: unknown }>> {
  const provider = createSolvelaProvider({
    baseURL: BASE_URL,
    wallet: makeStubWallet(MOCK_SIGNATURE),
  });

  const result = streamText({
    model: provider('gpt-4o'),
    prompt: 'call foo with x=42',
    tools: {
      foo: tool({
        description: 'A test tool that accepts a number x.',
        inputSchema: jsonSchema<{ x: number }>({
          type: 'object',
          properties: { x: { type: 'number' } },
          required: ['x'],
        }),
        // No execute — single round-trip; no follow-up LLM call.
      }),
    },
  });

  const parts: Array<{ type: string; [k: string]: unknown }> = [];
  for await (const part of result.fullStream) {
    parts.push(part as { type: string; [k: string]: unknown });
  }
  return parts;
}

// ---------------------------------------------------------------------------
// IT-6
// ---------------------------------------------------------------------------

describe('IT-6: streaming tool call — part ordering and argument assembly', () => {
  it('A. tool-input-start appears before tool-input-delta and tool-input-end', async () => {
    registerIntercepts();
    const parts = await runStream();
    const types = parts.map((p) => p.type);

    const startIdx = types.indexOf('tool-input-start');
    const endIdx = types.lastIndexOf('tool-input-end');
    const deltaIdx = types.indexOf('tool-input-delta');

    expect(startIdx, 'tool-input-start must be present').toBeGreaterThanOrEqual(0);
    expect(deltaIdx, 'tool-input-delta must be present').toBeGreaterThanOrEqual(0);
    expect(endIdx, 'tool-input-end must be present').toBeGreaterThanOrEqual(0);

    expect(startIdx).toBeLessThan(deltaIdx);
    expect(deltaIdx).toBeLessThan(endIdx);
  });

  it('A. tool-call appears after tool-input-end', async () => {
    registerIntercepts();
    const parts = await runStream();
    const types = parts.map((p) => p.type);

    const endIdx = types.lastIndexOf('tool-input-end');
    const toolCallIdx = types.indexOf('tool-call');

    expect(endIdx, 'tool-input-end must be present').toBeGreaterThanOrEqual(0);
    expect(toolCallIdx, 'tool-call must be present').toBeGreaterThanOrEqual(0);
    expect(endIdx).toBeLessThan(toolCallIdx);
  });

  it('A. at least one tool-input-delta part is emitted', async () => {
    registerIntercepts();
    const parts = await runStream();
    const deltaCount = parts.filter((p) => p.type === 'tool-input-delta').length;
    expect(deltaCount).toBeGreaterThanOrEqual(1);
  });

  it('B. tool-call input assembles to { x: 42 }', async () => {
    registerIntercepts();
    const parts = await runStream();

    const toolCallPart = parts.find((p) => p.type === 'tool-call');
    expect(toolCallPart, 'tool-call part must be present').toBeDefined();

    // StaticToolCall<TOOLS> carries .input (verified in node_modules/ai/dist/index.d.ts)
    expect((toolCallPart as { input?: unknown }).input).toEqual({ x: 42 });
  });

  it('C. finish part appears after tool-call', async () => {
    registerIntercepts();
    const parts = await runStream();
    const types = parts.map((p) => p.type);

    const toolCallIdx = types.indexOf('tool-call');
    const finishIdx = types.indexOf('finish');

    expect(toolCallIdx, 'tool-call must be present').toBeGreaterThanOrEqual(0);
    expect(finishIdx, 'finish must be present').toBeGreaterThanOrEqual(0);
    expect(toolCallIdx).toBeLessThan(finishIdx);
  });

  it('D. exactly 2 HTTP calls reach the mock gateway', async () => {
    registerIntercepts();
    await runStream();
    expect(mock.calls).toHaveLength(2);
  });
});

// ---------------------------------------------------------------------------
// IT-6 consolidated: all assertions in one sweep (catches ordering regressions)
// ---------------------------------------------------------------------------

describe('IT-6 consolidated: full ordering + assembly + call count', () => {
  it('tool-input-start < tool-input-delta(s) < tool-input-end < tool-call < finish; input={x:42}; 2 calls', async () => {
    registerIntercepts();
    const parts = await runStream();
    const types = parts.map((p) => p.type);

    // --- ordering ---
    const startIdx = types.indexOf('tool-input-start');
    const firstDeltaIdx = types.indexOf('tool-input-delta');
    const lastDeltaIdx = types.lastIndexOf('tool-input-delta');
    const endIdx = types.lastIndexOf('tool-input-end');
    const toolCallIdx = types.indexOf('tool-call');
    const finishIdx = types.indexOf('finish');

    expect(startIdx, 'tool-input-start').toBeGreaterThanOrEqual(0);
    expect(firstDeltaIdx, 'tool-input-delta').toBeGreaterThanOrEqual(0);
    expect(endIdx, 'tool-input-end').toBeGreaterThanOrEqual(0);
    expect(toolCallIdx, 'tool-call').toBeGreaterThanOrEqual(0);
    expect(finishIdx, 'finish').toBeGreaterThanOrEqual(0);

    expect(startIdx).toBeLessThan(firstDeltaIdx);
    expect(lastDeltaIdx).toBeLessThan(endIdx);
    expect(endIdx).toBeLessThan(toolCallIdx);
    expect(toolCallIdx).toBeLessThan(finishIdx);

    // at least one delta between start and end
    const deltaCount = types.filter((t) => t === 'tool-input-delta').length;
    expect(deltaCount).toBeGreaterThanOrEqual(1);

    // --- argument assembly ---
    const toolCallPart = parts.find((p) => p.type === 'tool-call');
    expect((toolCallPart as { input?: unknown }).input).toEqual({ x: 42 });

    // --- HTTP call count ---
    expect(mock.calls).toHaveLength(2);
  });
});
