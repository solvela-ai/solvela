import { describe, it, expect, vi, afterEach } from 'vitest';
import { SolvelaClient } from '../../src/client.js';
import { ChatRequest, ChatMessage, PaymentPayload, SolanaPayload } from '../../src/types.js';
import {
  GatewayError,
  InsufficientBalanceError,
  PaymentRejectedError,
  PaymentRequiredError,
} from '../../src/errors.js';
import { BalanceMonitor } from '../../src/balance.js';
import type { Signer } from '../../src/signer.js';
import type { PaymentAccept, Resource } from '../../src/types.js';

describe('SolvelaClient integration (mocked fetch)', () => {
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
      statusText: 'OK',
    });
    globalThis.fetch = fn as unknown as typeof fetch;
    return fn;
  }

  const successResponseBody = {
    id: 'chatcmpl-1',
    object: 'chat.completion',
    created: 123,
    model: 'gpt-4',
    choices: [
      { index: 0, message: { role: 'assistant', content: 'Hello!' }, finish_reason: 'stop' },
    ],
    usage: { prompt_tokens: 5, completion_tokens: 2, total_tokens: 7 },
  };

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

  it('chat returns response on success', async () => {
    mockFetch(200, successResponseBody);

    const client = new SolvelaClient();
    const request = new ChatRequest('gpt-4', [new ChatMessage('user', 'Hello')]);
    const resp = await client.chat(request);
    expect(resp.choices[0].message.content).toBe('Hello!');
  });

  it('chat uses cache on duplicate request', async () => {
    const fetchFn = mockFetch(200, successResponseBody);

    const client = new SolvelaClient({ config: { enableCache: true } });
    const request = new ChatRequest('gpt-4', [new ChatMessage('user', 'Hello')]);

    await client.chat(request);
    await client.chat(request);

    // Should only have called fetch once (cached second time)
    expect(fetchFn).toHaveBeenCalledTimes(1);
  });

  it('chat throws PaymentRequiredError on 402 without signer', async () => {
    mockFetch(402, prBody);

    const client = new SolvelaClient();
    const request = new ChatRequest('gpt-4', [new ChatMessage('user', 'Hello')]);
    const err = await client.chat(request).then(
      () => {
        throw new Error('expected rejection');
      },
      (e: unknown) => e,
    );
    // Initial 402 challenge (no signature attached yet) must stay a
    // PaymentRequiredError — never the post-signing PaymentRejectedError.
    expect(err).toBeInstanceOf(PaymentRequiredError);
    expect(err).not.toBeInstanceOf(PaymentRejectedError);
  });

  it('chat throws PaymentRejectedError on second 402 after signing', async () => {
    // Gateway returns 402 on every call: the first is the challenge, the
    // second — after a Payment-Signature was attached — is a post-signing
    // rejection and must surface as PaymentRejectedError carrying the second
    // 402 body (mirrors the canonical Python client and Go PR #548).
    const fetchFn = vi.fn().mockResolvedValue({
      status: 402,
      json: async () => prBody,
      body: null,
      statusText: 'Payment Required',
    });
    globalThis.fetch = fetchFn as unknown as typeof fetch;

    const signer: Signer = {
      async signPayment(_a, _r, resource, accepted: PaymentAccept) {
        return new PaymentPayload(2, resource, accepted, new SolanaPayload('tx=='));
      },
    };
    const client = new SolvelaClient({ signer });
    const err = await client
      .chat(new ChatRequest('gpt-4', [new ChatMessage('user', 'Hello')]))
      .then(
        () => {
          throw new Error('expected rejection');
        },
        (e: unknown) => e,
      );
    expect(err).toBeInstanceOf(PaymentRejectedError);
    expect(err).not.toBeInstanceOf(PaymentRequiredError);
    const rejected = err as PaymentRejectedError;
    expect(rejected.paymentRequired).toBeDefined();
    expect(rejected.paymentRequired?.accepts[0].amount).toBe('1000000');
    expect(fetchFn).toHaveBeenCalledTimes(2);
  });

  it('chat with quality retry retries on degraded response', async () => {
    const degradedResponse = {
      ...successResponseBody,
      choices: [
        {
          index: 0,
          message: { role: 'assistant', content: 'I cannot assist with that request.' },
          finish_reason: 'stop',
        },
      ],
    };

    let callCount = 0;
    globalThis.fetch = vi.fn().mockImplementation(async () => {
      callCount++;
      return {
        status: 200,
        json: async () => (callCount === 1 ? degradedResponse : successResponseBody),
        statusText: 'OK',
      };
    }) as unknown as typeof fetch;

    const client = new SolvelaClient({
      config: { enableQualityCheck: true, maxQualityRetries: 1 },
    });
    const request = new ChatRequest('gpt-4', [new ChatMessage('user', 'Hello')]);
    const resp = await client.chat(request);
    expect(resp.choices[0].message.content).toBe('Hello!');
    expect(callCount).toBe(2);
  });

  it('chat balance guard swaps to freeFallbackModel when monitor reports zero balance', async () => {
    // Wires BalanceMonitor → SolvelaClient. Without a polled monitor, Step 1
    // is skipped (lastBalance returns undefined) and the existing 402 path runs.
    const fetchFn = vi.fn().mockResolvedValue({
      status: 200,
      json: async () => ({ ...successResponseBody, model: 'free-model' }),
      statusText: 'OK',
    });
    globalThis.fetch = fetchFn as unknown as typeof fetch;

    const monitor = new BalanceMonitor(async () => 0);
    await monitor.pollOnce();

    const client = new SolvelaClient({
      config: { freeFallbackModel: 'free-model' },
      balanceMonitor: monitor,
    });
    const request = new ChatRequest('gpt-4', [new ChatMessage('user', 'Hi')]);
    const resp = await client.chat(request);

    // Single round-trip — no 402 needed because Step 1 swapped the model
    // preemptively.
    expect(fetchFn).toHaveBeenCalledTimes(1);
    expect(resp.model).toBe('free-model');

    const body = JSON.parse(fetchFn.mock.calls[0][1].body as string);
    expect(body.model).toBe('free-model');
  });

  it('chat balance guard does not fire when monitor has not polled', async () => {
    const fetchFn = mockFetch(200, successResponseBody);
    const monitor = new BalanceMonitor(async () => 0);
    // Intentionally NOT calling pollOnce — lastKnownBalance() is undefined.

    const client = new SolvelaClient({
      config: { freeFallbackModel: 'free-model' },
      balanceMonitor: monitor,
    });
    await client.chat(new ChatRequest('gpt-4', [new ChatMessage('user', 'Hi')]));

    const body = JSON.parse(fetchFn.mock.calls[0][1].body as string);
    expect(body.model).toBe('gpt-4');
  });

  it('chat signs payment when balance equals required amount exactly (boundary)', async () => {
    // Boundary case: balance === amountAtomic. Guard is `<`, so signer must
    // fire. A regression to `<=` would throw InsufficientBalanceError.
    let call = 0;
    globalThis.fetch = vi.fn().mockImplementation(() => {
      call++;
      if (call === 1) {
        return Promise.resolve({
          status: 402,
          json: async () => prBody, // amount: '1000000'
          body: null,
          statusText: 'Payment Required',
        });
      }
      return Promise.resolve({
        status: 200,
        json: async () => successResponseBody,
        body: null,
        statusText: 'OK',
      });
    }) as unknown as typeof fetch;

    let signerCalled = false;
    const signer: Signer = {
      async signPayment(_a, _r, resource, accepted: PaymentAccept) {
        signerCalled = true;
        return new PaymentPayload(2, resource, accepted, new SolanaPayload('tx=='));
      },
    };
    const monitor = new BalanceMonitor(async () => 1000000); // exactly equal
    await monitor.pollOnce();

    const client = new SolvelaClient({ signer, balanceMonitor: monitor });
    await client.chat(new ChatRequest('gpt-4', [new ChatMessage('user', 'Hi')]));
    expect(signerCalled).toBe(true);
    expect(call).toBe(2);
  });

  it('chat surfaces InsufficientBalanceError when monitor balance < required', async () => {
    const prBodyHigh = {
      ...prBody,
      accepts: [{ ...prBody.accepts[0], amount: '5000000' }],
    };
    globalThis.fetch = vi.fn().mockResolvedValue({
      status: 402,
      json: async () => prBodyHigh,
      statusText: 'Payment Required',
    }) as unknown as typeof fetch;

    const signer: Signer = {
      async signPayment(_a, _r, resource, accepted: PaymentAccept) {
        return new PaymentPayload(2, resource, accepted, new SolanaPayload('tx=='));
      },
    };
    const monitor = new BalanceMonitor(async () => 1000);  // 0.001 USDC, far below 5 USDC required
    await monitor.pollOnce();

    const client = new SolvelaClient({ signer, balanceMonitor: monitor });
    await expect(
      client.chat(new ChatRequest('gpt-4', [new ChatMessage('user', 'Hi')])),
    ).rejects.toBeInstanceOf(InsufficientBalanceError);
  });

  it('chat falls back to free model on 402 with freeFallbackModel', async () => {
    let callCount = 0;
    globalThis.fetch = vi.fn().mockImplementation(async () => {
      callCount++;
      if (callCount === 1) {
        return {
          status: 402,
          json: async () => prBody,
          statusText: 'Payment Required',
        };
      }
      return {
        status: 200,
        json: async () => ({
          ...successResponseBody,
          model: 'free-model',
        }),
        statusText: 'OK',
      };
    }) as unknown as typeof fetch;

    const client = new SolvelaClient({
      config: { freeFallbackModel: 'free-model' },
    });
    const request = new ChatRequest('gpt-4', [new ChatMessage('user', 'Hello')]);
    const resp = await client.chat(request);
    expect(resp.model).toBe('free-model');
    expect(callCount).toBe(2);
  });
});

describe('SolvelaClient chatStream payment retry (mocked fetch)', () => {
  const originalFetch = globalThis.fetch;

  afterEach(() => {
    globalThis.fetch = originalFetch;
  });

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

  function makeSseBody(chunks: string[]): ReadableStream<Uint8Array> {
    const encoder = new TextEncoder();
    const lines = chunks.map((c) => `data: ${c}\n\n`).concat(['data: [DONE]\n\n']);
    return new ReadableStream({
      start(controller) {
        for (const line of lines) {
          controller.enqueue(encoder.encode(line));
        }
        controller.close();
      },
    });
  }

  function makeMockSigner(): Signer {
    return {
      async signPayment(
        _amount: number,
        _recipient: string,
        resource: Resource,
        accepted: PaymentAccept,
      ): Promise<PaymentPayload> {
        return new PaymentPayload(2, resource, accepted, new SolanaPayload('fakeTx=='));
      },
    };
  }

  it('chatStream retries with payment signature on 402', async () => {
    const chunkData = JSON.stringify({
      id: 'c1',
      object: 'chat.completion.chunk',
      created: 1,
      model: 'gpt-4',
      choices: [{ index: 0, delta: { role: 'assistant', content: 'Hello' }, finish_reason: null }],
    });

    let callCount = 0;
    globalThis.fetch = vi.fn().mockImplementation(async (_url: string, opts: RequestInit) => {
      callCount++;
      const headers = opts.headers as Record<string, string>;
      if (callCount === 1) {
        // First call — no payment signature — return 402
        expect(headers['Payment-Signature']).toBeUndefined();
        return { status: 402, json: async () => prBody, body: null, statusText: 'Payment Required' };
      }
      // Second call — must have payment signature
      expect(headers['Payment-Signature']).toBeDefined();
      return { status: 200, body: makeSseBody([chunkData]), json: async () => ({}), statusText: 'OK' };
    }) as unknown as typeof fetch;

    const signer = makeMockSigner();
    const client = new SolvelaClient({ signer });
    const request = new ChatRequest('gpt-4', [new ChatMessage('user', 'Hello')]);

    const chunks: string[] = [];
    for await (const chunk of client.chatStream(request)) {
      const content = chunk.choices[0]?.delta?.content;
      if (content) chunks.push(content);
    }

    expect(callCount).toBe(2);
    expect(chunks).toEqual(['Hello']);
  });

  it('chatStream throws PaymentRequiredError on 402 without signer', async () => {
    globalThis.fetch = vi.fn().mockResolvedValue({
      status: 402,
      json: async () => prBody,
      body: null,
      statusText: 'Payment Required',
    }) as unknown as typeof fetch;

    const client = new SolvelaClient();
    const request = new ChatRequest('gpt-4', [new ChatMessage('user', 'Hello')]);

    const err = await (async () => {
      for await (const _chunk of client.chatStream(request)) {
        // consume
      }
    })().then(
      () => {
        throw new Error('expected rejection');
      },
      (e: unknown) => e,
    );
    // Initial 402 challenge (no signature attached yet) must stay a
    // PaymentRequiredError — never the post-signing PaymentRejectedError.
    expect(err).toBeInstanceOf(PaymentRequiredError);
    expect(err).not.toBeInstanceOf(PaymentRejectedError);
  });

  it('chatStream throws PaymentRejectedError on second 402 after signing', async () => {
    // 402 on every call: first is the challenge, second — with the
    // Payment-Signature attached on the streaming retry — is a post-signing
    // rejection and must convert to PaymentRejectedError (mirrors the
    // canonical Python chat_stream and Go ChatStream, PR #548).
    const fetchFn = vi.fn().mockResolvedValue({
      status: 402,
      json: async () => prBody,
      body: null,
      statusText: 'Payment Required',
    });
    globalThis.fetch = fetchFn as unknown as typeof fetch;

    const client = new SolvelaClient({ signer: makeMockSigner() });
    const request = new ChatRequest('gpt-4', [new ChatMessage('user', 'Hello')]);

    const err = await (async () => {
      for await (const _chunk of client.chatStream(request)) {
        // consume
      }
    })().then(
      () => {
        throw new Error('expected rejection');
      },
      (e: unknown) => e,
    );
    expect(err).toBeInstanceOf(PaymentRejectedError);
    expect(err).not.toBeInstanceOf(PaymentRequiredError);
    const rejected = err as PaymentRejectedError;
    expect(rejected.paymentRequired).toBeDefined();
    expect(rejected.paymentRequired?.accepts[0].amount).toBe('1000000');
    expect(fetchFn).toHaveBeenCalledTimes(2);
  });

  it('chatStream propagates a non-402 error from the signed retry unconverted', async () => {
    // 402 challenge, then 500 on the signed retry: the conversion must stay
    // scoped to PaymentRequiredError — anything else from the signed retry
    // propagates verbatim (never wrapped into PaymentRejectedError).
    const fetchFn = vi
      .fn()
      .mockResolvedValueOnce({
        status: 402,
        json: async () => prBody,
        body: null,
        statusText: 'Payment Required',
      })
      .mockResolvedValueOnce({
        status: 500,
        json: async () => ({}),
        body: null,
        statusText: 'Internal Server Error',
      });
    globalThis.fetch = fetchFn as unknown as typeof fetch;

    const client = new SolvelaClient({ signer: makeMockSigner() });
    const request = new ChatRequest('gpt-4', [new ChatMessage('user', 'Hello')]);

    const err = await (async () => {
      for await (const _chunk of client.chatStream(request)) {
        // consume
      }
    })().then(
      () => {
        throw new Error('expected rejection');
      },
      (e: unknown) => e,
    );
    expect(err).toBeInstanceOf(GatewayError);
    expect(err).not.toBeInstanceOf(PaymentRejectedError);
    expect(err).not.toBeInstanceOf(PaymentRequiredError);
    expect(fetchFn).toHaveBeenCalledTimes(2);
  });
});
