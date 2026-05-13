import { describe, it, expect, vi, afterEach } from 'vitest';
import { OpenAICompat } from '../../src/openai_compat.js';
import { SolvelaClient } from '../../src/client.js';

describe('OpenAICompat', () => {
  const originalFetch = globalThis.fetch;

  afterEach(() => {
    globalThis.fetch = originalFetch;
  });

  it('has chat.completions.create method', () => {
    const client = new SolvelaClient();
    const compat = new OpenAICompat(client);
    expect(typeof compat.chat.completions.create).toBe('function');
  });

  it('delegates create to client.chat for non-streaming', async () => {
    const successResponseBody = {
      id: 'chatcmpl-1',
      object: 'chat.completion',
      created: 123,
      model: 'gpt-4',
      choices: [
        { index: 0, message: { role: 'assistant', content: 'Hi!' }, finish_reason: 'stop' },
      ],
      usage: { prompt_tokens: 5, completion_tokens: 2, total_tokens: 7 },
    };

    globalThis.fetch = vi.fn().mockResolvedValue({
      status: 200,
      json: async () => successResponseBody,
      statusText: 'OK',
    }) as unknown as typeof fetch;

    const client = new SolvelaClient();
    const compat = new OpenAICompat(client);
    const result = await compat.chat.completions.create({
      model: 'gpt-4',
      messages: [{ role: 'user', content: 'Hello' }],
    });

    // Non-streaming returns ChatResponse
    expect(result).toBeDefined();
    expect((result as any).choices[0].message.content).toBe('Hi!');
  });
});
