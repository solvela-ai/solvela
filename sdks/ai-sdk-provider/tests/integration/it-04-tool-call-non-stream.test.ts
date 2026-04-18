/**
 * IT-4: Tool call (non-stream) — generateText with a zod tool returns a tool-call content part.
 *
 * Assertions (per plan §6 Phase 8, row IT-4):
 *   A. generateText completes (does not throw).
 *   B. result.toolCalls contains at least one entry with type 'tool-call'.
 *   C. The tool call has the correct toolName matching the defined tool.
 *   D. The tool call input is deserialized from the JSON arguments string.
 *   E. The tool call has a non-empty toolCallId.
 *   F. Exactly 2 HTTP calls reach the mock gateway.
 *   G. Second call carries PAYMENT-SIGNATURE (the stub wallet's signature).
 *
 * Transport: undici.MockAgent via installMockGateway — no real network calls.
 *
 * Mock response shape: OpenAI-compatible chat completion with
 *   choices[0].message.tool_calls = [{ id, type: 'function', function: { name, arguments } }]
 *   finish_reason: 'tool_calls'
 *
 * Note on field names (AI SDK v5):
 *   - tool() helper uses `inputSchema` (not `parameters`).
 *   - result.toolCalls entries have `toolName`, `input` (not `args`), `toolCallId`.
 */

import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { generateText, stepCountIs, tool } from 'ai';
import { z } from 'zod';
import { createSolvelaProvider } from '../../src/provider.js';
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
const MOCK_SIGNATURE = 'mock-base64-signature==';
const INTERCEPT_PATH = '/v1/chat/completions';

/** The tool name the mock gateway will report the model used. */
const TOOL_NAME = 'get_value';

/** The tool call id the mock gateway will return. */
const TOOL_CALL_ID = 'call_mock_001';

/** The arguments the mock "model" decided to call the tool with. */
const TOOL_ARGS = { x: 42 };

// ---------------------------------------------------------------------------
// Helper: build an OpenAI-format 200 response carrying a tool call
// ---------------------------------------------------------------------------

/**
 * Builds a minimal OpenAI-compatible chat completion body whose first choice
 * contains a tool call rather than assistant text content.
 *
 * Shape: https://platform.openai.com/docs/api-reference/chat/object
 *   choices[0].message.tool_calls = [{ id, type, function: { name, arguments } }]
 *   finish_reason: 'tool_calls'
 */
function makeToolCallCompletion(
  toolName: string,
  toolCallId: string,
  args: Record<string, unknown>,
): object {
  return {
    id: 'chatcmpl-mock-tool-call',
    object: 'chat.completion',
    created: 1_700_000_001,
    model: 'claude-sonnet-4.5',
    choices: [
      {
        index: 0,
        message: {
          role: 'assistant',
          content: null,
          tool_calls: [
            {
              id: toolCallId,
              type: 'function',
              function: {
                name: toolName,
                arguments: JSON.stringify(args),
              },
            },
          ],
        },
        finish_reason: 'tool_calls',
        logprobs: null,
      },
    ],
    usage: {
      prompt_tokens: 15,
      completion_tokens: 10,
      total_tokens: 25,
    },
  };
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
// Shared provider + tool definitions (recreated per test to avoid state leak)
// ---------------------------------------------------------------------------

function makeProvider() {
  return createSolvelaProvider({
    baseURL: BASE_URL,
    wallet: makeStubWallet(MOCK_SIGNATURE),
  });
}

/**
 * A simple zod-backed tool with a single numeric input `x`.
 * No `execute` — we only verify the tool-call part is emitted; we do not
 * want the SDK to attempt a second agentic step.
 */
const valueTools = {
  [TOOL_NAME]: tool({
    description: 'Return the square of x',
    inputSchema: z.object({ x: z.number() }),
  }),
};

// ---------------------------------------------------------------------------
// Helper: register 402 + tool-call-200 intercepts
// ---------------------------------------------------------------------------

function registerToolCallIntercepts() {
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
        data: JSON.stringify(
          makeToolCallCompletion(TOOL_NAME, TOOL_CALL_ID, TOOL_ARGS),
        ),
        responseOptions: { headers: { 'content-type': 'application/json' } },
      })),
    );
}

// ---------------------------------------------------------------------------
// IT-4 — individual assertion tests
// ---------------------------------------------------------------------------

describe('IT-4: tool call (non-stream)', () => {
  it('A. generateText completes without throwing', async () => {
    registerToolCallIntercepts();
    const provider = makeProvider();

    await expect(
      generateText({
        model: provider('claude-sonnet-4.5'),
        prompt: 'compute something',
        tools: valueTools,
        stopWhen: stepCountIs(1),
      }),
    ).resolves.toBeDefined();
  });

  it('B. result.toolCalls contains at least one tool-call entry', async () => {
    registerToolCallIntercepts();
    const provider = makeProvider();

    const result = await generateText({
      model: provider('claude-sonnet-4.5'),
      prompt: 'compute something',
      tools: valueTools,
      maxSteps: 1,
    });

    expect(result.toolCalls.length).toBeGreaterThanOrEqual(1);
    expect(result.toolCalls[0].type).toBe('tool-call');
  });

  it('C. tool call has the correct toolName', async () => {
    registerToolCallIntercepts();
    const provider = makeProvider();

    const result = await generateText({
      model: provider('claude-sonnet-4.5'),
      prompt: 'compute something',
      tools: valueTools,
      maxSteps: 1,
    });

    expect(result.toolCalls[0].toolName).toBe(TOOL_NAME);
  });

  it('D. tool call input is deserialized from the JSON arguments string', async () => {
    registerToolCallIntercepts();
    const provider = makeProvider();

    const result = await generateText({
      model: provider('claude-sonnet-4.5'),
      prompt: 'compute something',
      tools: valueTools,
      maxSteps: 1,
    });

    // The AI SDK parses `function.arguments` JSON and validates against inputSchema.
    expect(result.toolCalls[0].input).toEqual(TOOL_ARGS);
  });

  it('E. tool call has a non-empty toolCallId', async () => {
    registerToolCallIntercepts();
    const provider = makeProvider();

    const result = await generateText({
      model: provider('claude-sonnet-4.5'),
      prompt: 'compute something',
      tools: valueTools,
      maxSteps: 1,
    });

    expect(typeof result.toolCalls[0].toolCallId).toBe('string');
    expect(result.toolCalls[0].toolCallId.length).toBeGreaterThan(0);
  });

  it('F. exactly 2 HTTP calls reach the mock gateway', async () => {
    registerToolCallIntercepts();
    const provider = makeProvider();

    await generateText({
      model: provider('claude-sonnet-4.5'),
      prompt: 'compute something',
      tools: valueTools,
      maxSteps: 1,
    });

    expect(mock.calls).toHaveLength(2);
  });

  it('G. second HTTP call carries PAYMENT-SIGNATURE matching the stub wallet', async () => {
    registerToolCallIntercepts();
    const provider = makeProvider();

    await generateText({
      model: provider('claude-sonnet-4.5'),
      prompt: 'compute something',
      tools: valueTools,
      maxSteps: 1,
    });

    expect(getHeader(mock.calls[1], 'payment-signature')).toBe(MOCK_SIGNATURE);
  });
});

// ---------------------------------------------------------------------------
// IT-4 consolidated — all assertions together (catches ordering / state bugs)
// ---------------------------------------------------------------------------

describe('IT-4 consolidated: all assertions together', () => {
  it('tool-call: completes, toolName/input/toolCallId correct, 2 calls, sig on call 2', async () => {
    registerToolCallIntercepts();
    const provider = makeProvider();

    const result = await generateText({
      model: provider('claude-sonnet-4.5'),
      prompt: 'compute something',
      tools: valueTools,
      maxSteps: 1,
    });

    // B. at least one tool-call part
    expect(result.toolCalls.length).toBeGreaterThanOrEqual(1);
    const tc = result.toolCalls[0];
    expect(tc.type).toBe('tool-call');

    // C. correct toolName
    expect(tc.toolName).toBe(TOOL_NAME);

    // D. deserialized input
    expect(tc.input).toEqual(TOOL_ARGS);

    // E. non-empty toolCallId
    expect(typeof tc.toolCallId).toBe('string');
    expect(tc.toolCallId.length).toBeGreaterThan(0);

    // F. exactly 2 HTTP calls
    expect(mock.calls).toHaveLength(2);

    // G. second call carries PAYMENT-SIGNATURE
    expect(getHeader(mock.calls[1], 'payment-signature')).toBe(MOCK_SIGNATURE);
  });
});
