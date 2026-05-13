import { describe, it, expect } from 'vitest';
import {
  X402_VERSION,
  USDC_MINT,
  SOLANA_NETWORK,
  MAX_TIMEOUT_SECONDS,
  PLATFORM_FEE_PERCENT,
} from '../../src/constants.js';
import {
  ChatMessage,
  ChatRequest,
  ChatResponse,
  ChatChunk,
  PaymentRequired,
  PaymentPayload,
  SolanaPayload,
  CostBreakdown,
  Resource,
  PaymentAccept,
  Usage,
  ChatChoice,
  ChatDelta,
  ChatChunkChoice,
} from '../../src/types.js';

describe('constants', () => {
  it('has correct X402_VERSION', () => {
    expect(X402_VERSION).toBe(2);
  });

  it('has correct USDC_MINT', () => {
    expect(USDC_MINT).toBe('EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v');
  });

  it('has correct SOLANA_NETWORK', () => {
    expect(SOLANA_NETWORK).toBe('solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp');
  });

  it('has correct MAX_TIMEOUT_SECONDS', () => {
    expect(MAX_TIMEOUT_SECONDS).toBe(300);
  });

  it('has correct PLATFORM_FEE_PERCENT', () => {
    expect(PLATFORM_FEE_PERCENT).toBe(5);
  });
});

describe('ChatMessage', () => {
  it('toJSON uses snake_case and omits undefined', () => {
    const msg = new ChatMessage('user', 'Hello');
    const json = msg.toJSON();
    expect(json).toEqual({ role: 'user', content: 'Hello' });
    expect(json).not.toHaveProperty('name');
    expect(json).not.toHaveProperty('tool_calls');
    expect(json).not.toHaveProperty('tool_call_id');
  });

  it('toJSON includes optional fields when set', () => {
    const msg = new ChatMessage('assistant', 'Hi', 'bot', [], 'tc-1');
    const json = msg.toJSON();
    expect(json).toEqual({
      role: 'assistant',
      content: 'Hi',
      name: 'bot',
      tool_calls: [],
      tool_call_id: 'tc-1',
    });
  });

  it('fromJSON parses snake_case', () => {
    const msg = ChatMessage.fromJSON({
      role: 'user',
      content: 'Test',
      tool_call_id: 'abc',
    });
    expect(msg.role).toBe('user');
    expect(msg.content).toBe('Test');
    expect(msg.toolCallId).toBe('abc');
  });
});

describe('ChatRequest', () => {
  it('toJSON includes stream defaulting to false', () => {
    const req = new ChatRequest('gpt-4', [new ChatMessage('user', 'Hi')]);
    const json = req.toJSON();
    expect(json.model).toBe('gpt-4');
    expect(json.stream).toBe(false);
    expect(json.messages).toHaveLength(1);
    expect(json.messages[0].role).toBe('user');
  });

  it('toJSON includes optional fields when set', () => {
    const req = new ChatRequest(
      'gpt-4',
      [new ChatMessage('user', 'Hi')],
      100,
      0.7,
      0.9,
      true,
    );
    const json = req.toJSON();
    expect(json.max_tokens).toBe(100);
    expect(json.temperature).toBe(0.7);
    expect(json.top_p).toBe(0.9);
    expect(json.stream).toBe(true);
  });

  it('fromJSON parses snake_case', () => {
    const req = ChatRequest.fromJSON({
      model: 'gpt-4',
      messages: [{ role: 'user', content: 'Hi' }],
      max_tokens: 50,
      stream: true,
    });
    expect(req.model).toBe('gpt-4');
    expect(req.maxTokens).toBe(50);
    expect(req.stream).toBe(true);
    expect(req.messages[0]).toBeInstanceOf(ChatMessage);
  });
});

describe('ChatResponse', () => {
  it('fromJSON parses full response', () => {
    const resp = ChatResponse.fromJSON({
      id: 'chatcmpl-1',
      object: 'chat.completion',
      created: 1234567890,
      model: 'gpt-4',
      choices: [
        {
          index: 0,
          message: { role: 'assistant', content: 'Hello!' },
          finish_reason: 'stop',
        },
      ],
      usage: {
        prompt_tokens: 10,
        completion_tokens: 5,
        total_tokens: 15,
      },
    });
    expect(resp.id).toBe('chatcmpl-1');
    expect(resp.model).toBe('gpt-4');
    expect(resp.choices).toHaveLength(1);
    expect(resp.choices[0].message.content).toBe('Hello!');
    expect(resp.choices[0].finishReason).toBe('stop');
    expect(resp.usage?.promptTokens).toBe(10);
    expect(resp.usage?.completionTokens).toBe(5);
    expect(resp.usage?.totalTokens).toBe(15);
  });

  it('toJSON produces snake_case', () => {
    const resp = new ChatResponse(
      'id-1',
      'chat.completion',
      123,
      'gpt-4',
      [new ChatChoice(0, new ChatMessage('assistant', 'Hi'), 'stop')],
      new Usage(10, 5, 15),
    );
    const json = resp.toJSON();
    expect(json.finish_reason).toBeUndefined(); // finish_reason is inside choices
    expect(json.choices[0].finish_reason).toBe('stop');
    expect(json.usage.prompt_tokens).toBe(10);
  });
});

describe('ChatChunk', () => {
  it('fromJSON parses streaming chunk', () => {
    const chunk = ChatChunk.fromJSON({
      id: 'chatcmpl-1',
      object: 'chat.completion.chunk',
      created: 123,
      model: 'gpt-4',
      choices: [
        {
          index: 0,
          delta: { role: 'assistant', content: 'Hi' },
          finish_reason: null,
        },
      ],
    });
    expect(chunk.id).toBe('chatcmpl-1');
    expect(chunk.choices[0].delta.content).toBe('Hi');
    expect(chunk.choices[0].finishReason).toBeNull();
  });
});

describe('PaymentRequired', () => {
  it('fromJSON parses 402 response', () => {
    const pr = PaymentRequired.fromJSON({
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
        provider_cost: '0.01',
        platform_fee: '0.0005',
        total: '0.0105',
        currency: 'USDC',
        fee_percent: 5,
      },
      error: 'Payment required',
    });
    expect(pr.x402Version).toBe(2);
    expect(pr.resource.url).toBe('/v1/chat/completions');
    expect(pr.accepts).toHaveLength(1);
    expect(pr.accepts[0].scheme).toBe('exact');
    expect(pr.accepts[0].amount).toBe('1000000');
    expect(pr.costBreakdown.total).toBe('0.0105');
  });
});

describe('PaymentPayload', () => {
  it('toJSON produces snake_case', () => {
    const payload = new PaymentPayload(
      2,
      new Resource('/v1/chat/completions', 'POST'),
      new PaymentAccept(
        'exact',
        'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
        '1000000',
        'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
        '11111111111111111111111111111111',
        300,
      ),
      new SolanaPayload('txhash123'),
    );
    const json = payload.toJSON();
    expect(json.x402_version).toBe(2);
    expect(json.resource.url).toBe('/v1/chat/completions');
    expect(json.accepted.scheme).toBe('exact');
    expect(json.accepted.network).toBe('solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp');
    expect(json.payload.transaction).toBe('txhash123');
  });
});
