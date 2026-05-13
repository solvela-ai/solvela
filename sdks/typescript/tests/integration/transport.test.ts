import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { Transport } from '../../src/transport.js';
import { ChatRequest, ChatMessage, ChatResponse, PaymentRequired } from '../../src/types.js';
import { GatewayError, TimeoutError } from '../../src/errors.js';

describe('Transport integration (mocked fetch)', () => {
  const originalFetch = globalThis.fetch;

  afterEach(() => {
    globalThis.fetch = originalFetch;
  });

  function mockFetch(
    status: number,
    body: Record<string, unknown>,
  ): ReturnType<typeof vi.fn> {
    const fn = vi.fn().mockResolvedValue({
      status,
      json: () => Promise.resolve(body),
      body: null,
    });
    globalThis.fetch = fn as unknown as typeof fetch;
    return fn;
  }

  const request = new ChatRequest('gpt-4', [new ChatMessage('user', 'Hello')]);

  it('sendChat returns ChatResponse on 200', async () => {
    const responseBody = {
      id: 'chatcmpl-1',
      object: 'chat.completion',
      created: 123,
      model: 'gpt-4',
      choices: [
        { index: 0, message: { role: 'assistant', content: 'Hi!' }, finish_reason: 'stop' },
      ],
      usage: { prompt_tokens: 5, completion_tokens: 2, total_tokens: 7 },
    };
    mockFetch(200, responseBody);

    const transport = new Transport('http://localhost:8402');
    const result = await transport.sendChat(request);
    expect(result).toBeInstanceOf(ChatResponse);
    expect((result as ChatResponse).choices[0].message.content).toBe('Hi!');
  });

  it('sendChat returns PaymentRequired on 402', async () => {
    const prBody = {
      x402_version: 2,
      resource: { url: '/v1/chat/completions', method: 'POST' },
      accepts: [
        {
          scheme: 'exact',
          network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
          amount: '1000000',
          asset: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
          pay_to: '11111111111111111111111111111111',
          max_timeout_seconds: 300,
        },
      ],
      cost_breakdown: {
        provider_cost: '0.005',
        platform_fee: '0.00025',
        total: '0.00525',
        currency: 'USDC',
        fee_percent: 5,
      },
      error: 'Payment required',
    };
    mockFetch(402, prBody);

    const transport = new Transport('http://localhost:8402');
    const result = await transport.sendChat(request);
    expect(result).toBeInstanceOf(PaymentRequired);
    expect((result as PaymentRequired).x402Version).toBe(2);
  });

  it('sendChat throws GatewayError on 500', async () => {
    mockFetch(500, { error: 'Internal Server Error' });

    const transport = new Transport('http://localhost:8402');
    await expect(transport.sendChat(request)).rejects.toThrow(GatewayError);
  });

  it('sendChat throws GatewayError with status', async () => {
    mockFetch(503, { error: 'Service Unavailable' });

    const transport = new Transport('http://localhost:8402');
    try {
      await transport.sendChat(request);
      expect.unreachable('should have thrown');
    } catch (e) {
      expect(e).toBeInstanceOf(GatewayError);
      expect((e as GatewayError).status).toBe(503);
    }
  });

  it('sendChat passes payment signature header', async () => {
    const fetchFn = mockFetch(200, {
      id: 'c1',
      object: 'chat.completion',
      created: 1,
      model: 'gpt-4',
      choices: [{ index: 0, message: { role: 'assistant', content: 'ok' }, finish_reason: 'stop' }],
    });

    const transport = new Transport('http://localhost:8402');
    await transport.sendChat(request, 'my-sig');

    const callArgs = fetchFn.mock.calls[0];
    expect(callArgs[1].headers['Payment-Signature']).toBe('my-sig');
  });

  it('sendChat sets stream to false in body', async () => {
    const fetchFn = mockFetch(200, {
      id: 'c1',
      object: 'chat.completion',
      created: 1,
      model: 'gpt-4',
      choices: [{ index: 0, message: { role: 'assistant', content: 'ok' }, finish_reason: 'stop' }],
    });

    const transport = new Transport('http://localhost:8402');
    await transport.sendChat(request);

    const body = JSON.parse(fetchFn.mock.calls[0][1].body);
    expect(body.stream).toBe(false);
  });

  it('fetchModels returns model data on 200', async () => {
    const models = [{ id: 'gpt-4', provider: 'openai' }];
    mockFetch(200, { data: models });

    const transport = new Transport('http://localhost:8402');
    const result = await transport.fetchModels();
    expect(result).toHaveLength(1);
    expect(result[0].id).toBe('gpt-4');
  });

  it('fetchModels throws GatewayError on non-200', async () => {
    mockFetch(500, {});

    const transport = new Transport('http://localhost:8402');
    await expect(transport.fetchModels()).rejects.toThrow(GatewayError);
  });
});
