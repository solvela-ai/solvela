import { describe, it, expect, vi, afterEach } from 'vitest';
import { SolvelaClient } from '../../src/client.js';
import { ChatRequest, ChatMessage } from '../../src/types.js';

/**
 * Verifies that the X-Solvela-Retry-Reason: degraded header is sent on quality
 * retries but NOT on the initial request — parity with Rust/Go/Python SDKs.
 */
describe('quality retry header', () => {
  const originalFetch = globalThis.fetch;

  afterEach(() => {
    globalThis.fetch = originalFetch;
  });

  function makeChatBody(content: string) {
    return {
      id: 'chatcmpl-test',
      object: 'chat.completion',
      created: 123,
      model: 'gpt-4o',
      choices: [
        { index: 0, message: { role: 'assistant', content }, finish_reason: 'stop' },
      ],
      usage: { prompt_tokens: 5, completion_tokens: 3, total_tokens: 8 },
    };
  }

  it('sends X-Solvela-Retry-Reason: degraded on retry but not on initial request', async () => {
    const capturedHeaders: string[][] = [];
    let callCount = 0;

    const mockFn = vi.fn().mockImplementation((url: string, init: RequestInit) => {
      const headers = init.headers as Record<string, string>;
      capturedHeaders.push(Object.entries(headers).map(([k, v]) => `${k}: ${v}`));

      callCount++;
      // First call: return a degraded response (known error phrase triggers retry)
      // Second call: return a normal response
      const content =
        callCount === 1
          ? 'I cannot assist with that request.'
          : 'Hello, here is your answer!';

      return Promise.resolve({
        status: 200,
        json: () => Promise.resolve(makeChatBody(content)),
        body: null,
        statusText: 'OK',
      });
    });

    globalThis.fetch = mockFn as unknown as typeof fetch;

    const client = new SolvelaClient({
      config: {
        gatewayUrl: 'http://localhost:8402',
        enableQualityCheck: true,
        maxQualityRetries: 1,
      },
    });

    const response = await client.chat(
      new ChatRequest('gpt-4o', [new ChatMessage('user', 'Hello')]),
    );

    // Two fetch calls: initial + one retry
    expect(mockFn).toHaveBeenCalledTimes(2);

    // Final response should be the non-degraded one
    expect(response.choices[0].message.content).toBe('Hello, here is your answer!');

    // First call must NOT have the retry header
    const firstCallHeaders = capturedHeaders[0].join('\n').toLowerCase();
    expect(firstCallHeaders).not.toContain('x-solvela-retry-reason');

    // Second (retry) call MUST have X-Solvela-Retry-Reason: degraded
    const secondCallHeaders = capturedHeaders[1].join('\n').toLowerCase();
    expect(secondCallHeaders).toContain('x-solvela-retry-reason: degraded');
  });
});
