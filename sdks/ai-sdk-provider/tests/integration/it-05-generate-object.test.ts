/**
 * IT-5: generateObject with zod schema — structured output opt-in vs default.
 *
 * Scenario (per plan §6 Phase 8, row IT-5):
 *   generateObject with a zod schema resolves; the `response_format` field in
 *   the request body reflects `supportsStructuredOutputs`.
 *
 * Two providers under test:
 *   - Provider A: createSolvelaProvider({ wallet, baseURL, supportsStructuredOutputs: true })
 *   - Provider B: createSolvelaProvider({ wallet, baseURL })  ← default false
 *
 * Zod schema: z.object({ name: z.string(), age: z.number() })
 *
 * Mock 200 response: OpenAI-format chat completion where
 *   choices[0].message.content is JSON matching the schema.
 *
 * Assertions — Provider A (opt-in):
 *   A1. generateObject resolves without throwing.
 *   A2. result.object matches { name: string, age: number } (validated by zod at callsite).
 *   A3. Second intercept's request body contains response_format.type === "json_schema".
 *
 * Assertions — Provider B (default):
 *   B1. generateObject resolves without throwing.
 *   B2. result.object matches { name: string, age: number }.
 *   B3. Second intercept's request body does NOT contain response_format.type === "json_schema".
 *       (The openai-compatible layer emits { type: "json_object" } — present but not json_schema.)
 *
 * Transport: undici.MockAgent via installMockGateway — no real network calls.
 *
 * Note on baseURL normalization:
 *   createSolvelaProvider given 'https://gateway.test' (no /v1 suffix).
 *   config.ts normalizeBaseURL appends /v1 → 'https://gateway.test/v1'.
 *   @ai-sdk/openai-compatible appends /chat/completions.
 *   Final URL: 'https://gateway.test/v1/chat/completions'.
 *   MockAgent intercept path: '/v1/chat/completions'.
 *
 * Note on response_format behavior (verified against @ai-sdk/openai-compatible dist/index.mjs):
 *   supportsStructuredOutputs: true  → response_format: { type: "json_schema", json_schema: { ... } }
 *   supportsStructuredOutputs: false → response_format: { type: "json_object" }
 *   The plan §6 IT-5 annotation "NO response_format" refers to the absence of json_schema mode,
 *   not the absence of the key entirely. This test reflects the actual library behavior.
 */

import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { generateObject } from 'ai';
import { z } from 'zod';
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
const MOCK_SIGNATURE = 'mock-base64-signature==';
const INTERCEPT_PATH = '/v1/chat/completions';

/** JSON string that satisfies the test schema. */
const OBJECT_JSON = JSON.stringify({ name: 'Alice', age: 30 });

/** Zod schema used across all tests. */
const PERSON_SCHEMA = z.object({
  name: z.string(),
  age: z.number(),
});

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
 * Register a 402 → 200 intercept pair on `mock.pool`.
 * The 200 response body is a chat completion whose message.content is the
 * given JSON string (must match the zod schema at callsite).
 */
function registerInterceptPair(contentJson: string): void {
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
        data: JSON.stringify(makeChatCompletionSuccess(contentJson)),
        responseOptions: { headers: { 'content-type': 'application/json' } },
      })),
    );
}

/**
 * Parse the request body captured by the second intercept (the signed retry)
 * and return it as a plain object.
 */
function parseSecondRequestBody(): Record<string, unknown> {
  const bodyStr = mock.calls[1]?.body;
  if (!bodyStr) {
    throw new Error('Second intercepted call has no body — ensure two intercepts were registered');
  }
  return JSON.parse(bodyStr) as Record<string, unknown>;
}

// ---------------------------------------------------------------------------
// Provider A — supportsStructuredOutputs: true
// ---------------------------------------------------------------------------

describe('IT-5A: generateObject with supportsStructuredOutputs: true (opt-in)', () => {
  it('A1. resolves without throwing', async () => {
    registerInterceptPair(OBJECT_JSON);

    const providerA = createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: makeStubWallet(MOCK_SIGNATURE),
      supportsStructuredOutputs: true,
    });

    await expect(
      generateObject({
        model: providerA('gpt-4o'),
        schema: PERSON_SCHEMA,
        prompt: 'Give me a person',
      }),
    ).resolves.toBeDefined();
  });

  it('A2. result.object matches the zod schema (name: string, age: number)', async () => {
    registerInterceptPair(OBJECT_JSON);

    const providerA = createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: makeStubWallet(MOCK_SIGNATURE),
      supportsStructuredOutputs: true,
    });

    const result = await generateObject({
      model: providerA('gpt-4o'),
      schema: PERSON_SCHEMA,
      prompt: 'Give me a person',
    });

    // zod parse validates shape — throws if schema is violated
    const parsed = PERSON_SCHEMA.parse(result.object);
    expect(parsed.name).toBe('Alice');
    expect(parsed.age).toBe(30);
  });

  it('A3. second request body contains response_format.type === "json_schema"', async () => {
    registerInterceptPair(OBJECT_JSON);

    const providerA = createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: makeStubWallet(MOCK_SIGNATURE),
      supportsStructuredOutputs: true,
    });

    await generateObject({
      model: providerA('gpt-4o'),
      schema: PERSON_SCHEMA,
      prompt: 'Give me a person',
    });

    expect(mock.calls).toHaveLength(2);
    const body = parseSecondRequestBody();
    expect(body).toHaveProperty('response_format');
    const rf = body['response_format'] as Record<string, unknown>;
    expect(rf['type']).toBe('json_schema');
    // json_schema sub-object must be present
    expect(rf).toHaveProperty('json_schema');
  });
});

// ---------------------------------------------------------------------------
// Provider B — supportsStructuredOutputs: false (default)
// ---------------------------------------------------------------------------

describe('IT-5B: generateObject with supportsStructuredOutputs: false (default)', () => {
  it('B1. resolves without throwing', async () => {
    registerInterceptPair(OBJECT_JSON);

    const providerB = createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: makeStubWallet(MOCK_SIGNATURE),
      // supportsStructuredOutputs omitted — defaults to false
    });

    await expect(
      generateObject({
        model: providerB('gpt-4o'),
        schema: PERSON_SCHEMA,
        prompt: 'Give me a person',
      }),
    ).resolves.toBeDefined();
  });

  it('B2. result.object matches the zod schema (name: string, age: number)', async () => {
    registerInterceptPair(OBJECT_JSON);

    const providerB = createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: makeStubWallet(MOCK_SIGNATURE),
    });

    const result = await generateObject({
      model: providerB('gpt-4o'),
      schema: PERSON_SCHEMA,
      prompt: 'Give me a person',
    });

    const parsed = PERSON_SCHEMA.parse(result.object);
    expect(parsed.name).toBe('Alice');
    expect(parsed.age).toBe(30);
  });

  it('B3. second request body does NOT use response_format.type === "json_schema"', async () => {
    registerInterceptPair(OBJECT_JSON);

    const providerB = createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: makeStubWallet(MOCK_SIGNATURE),
    });

    await generateObject({
      model: providerB('gpt-4o'),
      schema: PERSON_SCHEMA,
      prompt: 'Give me a person',
    });

    expect(mock.calls).toHaveLength(2);
    const body = parseSecondRequestBody();

    // The openai-compatible layer emits { type: "json_object" } when
    // supportsStructuredOutputs is false — response_format is present but
    // must NOT be json_schema mode.
    const rf = body['response_format'] as Record<string, unknown> | undefined;
    expect(rf?.['type']).not.toBe('json_schema');
  });
});

// ---------------------------------------------------------------------------
// IT-5 consolidated: both providers, all key assertions in one test
// ---------------------------------------------------------------------------

describe('IT-5 consolidated: opt-in vs default response_format distinction', () => {
  it('providerA uses json_schema; providerB does not — objects resolve in both cases', async () => {
    // --- Provider A run ---
    registerInterceptPair(OBJECT_JSON);

    const providerA = createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: makeStubWallet(MOCK_SIGNATURE),
      supportsStructuredOutputs: true,
    });

    const resultA = await generateObject({
      model: providerA('gpt-4o'),
      schema: PERSON_SCHEMA,
      prompt: 'Give me a person',
    });

    expect(PERSON_SCHEMA.parse(resultA.object)).toMatchObject({ name: 'Alice', age: 30 });

    const bodyA = parseSecondRequestBody();
    const rfA = bodyA['response_format'] as Record<string, unknown>;
    expect(rfA['type']).toBe('json_schema');

    // Reset state between providers
    await mock.reset();
    mock = installMockGateway(BASE_URL);

    // --- Provider B run ---
    registerInterceptPair(OBJECT_JSON);

    const providerB = createSolvelaProvider({
      baseURL: BASE_URL,
      wallet: makeStubWallet(MOCK_SIGNATURE),
    });

    const resultB = await generateObject({
      model: providerB('gpt-4o'),
      schema: PERSON_SCHEMA,
      prompt: 'Give me a person',
    });

    expect(PERSON_SCHEMA.parse(resultB.object)).toMatchObject({ name: 'Alice', age: 30 });

    const bodyB = parseSecondRequestBody();
    const rfB = bodyB['response_format'] as Record<string, unknown> | undefined;
    expect(rfB?.['type']).not.toBe('json_schema');
  });
});
