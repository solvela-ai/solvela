import { describe, it, expect, vi, afterEach, beforeEach } from 'vitest';
import { Transport } from '../../src/transport.js';
import { ChatRequest, ChatMessage } from '../../src/types.js';
import { GatewayError } from '../../src/errors.js';

/**
 * Coverage for security audit fixes:
 *  - MEDIUM-1: SSE JSON.parse must not crash the stream on malformed data
 *  - LOW-1:    gateway error strings sanitized (CR/LF/control chars stripped, truncated)
 */

describe('Transport: SSE malformed chunk handling', () => {
  const originalFetch = globalThis.fetch;
  afterEach(() => {
    globalThis.fetch = originalFetch;
  });

  function makeSseBody(lines: string[]): ReadableStream<Uint8Array> {
    const encoder = new TextEncoder();
    const payload = lines.map((l) => `${l}\n\n`).concat(['data: [DONE]\n\n']);
    return new ReadableStream({
      start(controller) {
        for (const line of payload) {
          controller.enqueue(encoder.encode(line));
        }
        controller.close();
      },
    });
  }

  it('skips malformed JSON chunks but yields valid ones', async () => {
    const validChunk = JSON.stringify({
      id: 'c1',
      object: 'chat.completion.chunk',
      created: 1,
      model: 'gpt-4',
      choices: [
        {
          index: 0,
          delta: { role: 'assistant', content: 'ok' },
          finish_reason: null,
        },
      ],
    });

    globalThis.fetch = vi.fn().mockResolvedValue({
      status: 200,
      body: makeSseBody([
        'data: {not json',
        `data: ${validChunk}`,
      ]),
      json: async () => ({}),
      statusText: 'OK',
    }) as unknown as typeof fetch;

    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});

    const t = new Transport('http://localhost:8402');
    const req = new ChatRequest('gpt-4', [new ChatMessage('user', 'hi')]);

    const collected: string[] = [];
    for await (const chunk of t.sendChatStream(req)) {
      const c = chunk.choices[0]?.delta?.content;
      if (c) collected.push(c);
    }

    expect(collected).toEqual(['ok']);
    expect(warnSpy).toHaveBeenCalled();
    warnSpy.mockRestore();
  });
});

describe('Transport: gateway error sanitization', () => {
  const originalFetch = globalThis.fetch;
  afterEach(() => {
    globalThis.fetch = originalFetch;
  });

  it('strips CR/LF/control characters from gateway error messages', async () => {
    globalThis.fetch = vi.fn().mockResolvedValue({
      status: 500,
      json: async () => ({ error: 'oops\r\ninjected: badline\x07' }),
      statusText: 'Internal Server Error',
    }) as unknown as typeof fetch;

    const t = new Transport('http://localhost:8402');
    const req = new ChatRequest('gpt-4', [new ChatMessage('user', 'hi')]);

    try {
      await t.sendChat(req);
      expect.unreachable('should have thrown');
    } catch (e) {
      expect(e).toBeInstanceOf(GatewayError);
      const msg = (e as GatewayError).message;
      expect(msg).not.toContain('\r');
      expect(msg).not.toContain('\n');
      expect(msg).not.toContain('\x07');
      expect(msg).toContain('oops');
    }
  });

  it('truncates excessively long gateway error messages', async () => {
    const long = 'x'.repeat(1000);
    globalThis.fetch = vi.fn().mockResolvedValue({
      status: 500,
      json: async () => ({ error: long }),
      statusText: 'Internal Server Error',
    }) as unknown as typeof fetch;

    const t = new Transport('http://localhost:8402');
    const req = new ChatRequest('gpt-4', [new ChatMessage('user', 'hi')]);

    try {
      await t.sendChat(req);
      expect.unreachable('should have thrown');
    } catch (e) {
      expect((e as GatewayError).message.length).toBeLessThanOrEqual(200);
    }
  });
});
