/**
 * Tests for @solvela/openfang-router.
 *
 * Run with:
 *   node --import tsx --test tests/router.test.ts
 */

import { describe, it, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import type { IncomingMessage, ServerResponse } from 'node:http';

import { ConfigError, createSolvelaRouter } from '../src/index.js';

interface MockServer {
  url: string;
  close: () => Promise<void>;
  setMode: (mode: 'ok' | '402' | 'error' | 'stream') => void;
  lastBody: () => string | undefined;
  lastHeaders: () => Record<string, string | string[] | undefined>;
}

function startMockGateway(): Promise<MockServer> {
  let mode: 'ok' | '402' | 'error' | 'stream' = 'ok';
  let lastBody: string | undefined;
  let lastHeaders: Record<string, string | string[] | undefined> = {};

  const server = createServer((req: IncomingMessage, res: ServerResponse) => {
    let raw = '';
    req.on('data', (c) => { raw += c.toString(); });
    req.on('end', () => {
      lastBody = raw;
      lastHeaders = req.headers;

      if (req.url !== '/v1/chat/completions') {
        res.writeHead(404);
        res.end('Not found');
        return;
      }

      if (mode === 'ok') {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify({
          id: 'test-id',
          object: 'chat.completion',
          created: Date.now(),
          model: 'auto',
          choices: [{
            index: 0,
            message: { role: 'assistant', content: 'Hello from mock' },
            finish_reason: 'stop',
          }],
          usage: { prompt_tokens: 5, completion_tokens: 10, total_tokens: 15 },
        }));
        return;
      }

      if (mode === '402') {
        if (req.headers['payment-signature']) {
          res.writeHead(200, { 'content-type': 'application/json' });
          res.end(JSON.stringify({
            id: 'paid-id',
            object: 'chat.completion',
            created: Date.now(),
            model: 'auto',
            choices: [{
              index: 0,
              message: { role: 'assistant', content: 'Paid response' },
              finish_reason: 'stop',
            }],
          }));
          return;
        }
        const paymentRequired = {
          x402_version: 2,
          accepts: [{
            scheme: 'usdc-spl',
            network: 'mainnet',
            amount: '1000',
            asset: 'USDC',
            pay_to: 'GaTeWaYPuBkEyXXXXXXXXXXXXXXXXXXXXXXXXXXXXX',
            max_timeout_seconds: 30,
          }],
          cost_breakdown: {
            provider_cost: '0.000800',
            platform_fee: '0.000200',
            total: '0.001000',
            currency: 'USDC',
            fee_percent: 20,
          },
          error: 'Payment required',
        };
        res.writeHead(402, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ error: { message: JSON.stringify(paymentRequired) } }));
        return;
      }

      if (mode === 'stream') {
        res.writeHead(200, { 'content-type': 'text/event-stream' });
        res.write('data: {"choices":[{"delta":{"content":"Hi"}}]}\n');
        res.write('data: {"choices":[{"delta":{"content":" there"}}]}\n');
        res.write('data: [DONE]\n');
        res.end();
        return;
      }

      res.writeHead(500, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ error: { message: 'Internal server error' } }));
    });
  });

  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      const addr = server.address() as { port: number };
      resolve({
        url: `http://127.0.0.1:${addr.port}`,
        setMode: (m) => { mode = m; },
        lastBody: () => lastBody,
        lastHeaders: () => lastHeaders,
        close: () => new Promise((r) => server.close(() => r())),
      });
    });
  });
}

let mock: MockServer;

before(async () => { mock = await startMockGateway(); });
after(async () => { await mock.close(); });

describe('createSolvelaRouter — factory shape', () => {
  it('throws ConfigError when gatewayUrl is missing', () => {
    const saved = process.env.LLM_ROUTER_API_URL;
    delete process.env.LLM_ROUTER_API_URL;
    assert.throws(
      () => createSolvelaRouter(),
      (err) => err instanceof ConfigError && err.message.includes('gatewayUrl'),
    );
    if (saved !== undefined) process.env.LLM_ROUTER_API_URL = saved;
  });

  it('returns a plugin descriptor with name/version/complete/completeStream', () => {
    const r = createSolvelaRouter({ gatewayUrl: 'http://localhost:8402' });
    assert.equal(r.name, '@solvela/openfang-router');
    assert.equal(r.version, '0.1.0');
    assert.equal(typeof r.complete, 'function');
    assert.equal(typeof r.completeStream, 'function');
  });
});

describe('complete — happy path', () => {
  it('returns a ChatResponse on 200', async () => {
    mock.setMode('ok');
    const r = createSolvelaRouter({ gatewayUrl: mock.url });
    const resp = await r.complete({ messages: [{ role: 'user', content: 'hello' }] });
    assert.equal(resp.choices[0].message.content, 'Hello from mock');
  });

  it('uses configured defaultModel when model is omitted', async () => {
    mock.setMode('ok');
    const r = createSolvelaRouter({ gatewayUrl: mock.url, defaultModel: 'eco' });
    await r.complete({ messages: [{ role: 'user', content: 'hi' }] });
    const body = JSON.parse(mock.lastBody() ?? '{}');
    assert.equal(body.model, 'eco');
  });
});

describe('complete — 402 → payment → 200', () => {
  it('handles the full payment flow with stub signing when @solana/web3.js is absent', async () => {
    mock.setMode('402');
    const r = createSolvelaRouter({ gatewayUrl: mock.url, walletKey: 'stub-key' });
    const resp = await r.complete({ messages: [{ role: 'user', content: 'pay' }] });
    assert.equal(resp.choices[0].message.content, 'Paid response');
  });
});

describe('complete — agentic profile auto-selection', () => {
  it('sets profile=agentic when tools array is non-empty', async () => {
    mock.setMode('ok');
    const r = createSolvelaRouter({ gatewayUrl: mock.url, profile: 'eco' });
    await r.complete({
      messages: [{ role: 'user', content: 'use tools' }],
      tools: [{ type: 'function', function: { name: 'lookup' } }],
    });
    const body = JSON.parse(mock.lastBody() ?? '{}');
    assert.equal(body.profile, 'agentic');
  });

  it('preserves configured profile when tools are absent', async () => {
    mock.setMode('ok');
    const r = createSolvelaRouter({ gatewayUrl: mock.url, profile: 'premium' });
    await r.complete({ messages: [{ role: 'user', content: 'plain chat' }] });
    const body = JSON.parse(mock.lastBody() ?? '{}');
    assert.equal(body.profile, 'premium');
  });
});

describe('complete — error paths', () => {
  it('throws RouterError on 5xx', async () => {
    mock.setMode('error');
    const r = createSolvelaRouter({ gatewayUrl: mock.url });
    await assert.rejects(
      () => r.complete({ messages: [{ role: 'user', content: 'boom' }] }),
      (err: unknown) => err instanceof Error && err.message.includes('500'),
    );
  });
});

describe('complete — timeout', () => {
  it('aborts when timeoutMs elapses before response', async () => {
    // Listen on a port that accepts the connection but never responds.
    const slow = createServer(() => { /* hold open */ });
    await new Promise<void>((resolve) => slow.listen(0, '127.0.0.1', resolve));
    const addr = slow.address() as { port: number };
    const r = createSolvelaRouter({
      gatewayUrl: `http://127.0.0.1:${addr.port}`,
      timeoutMs: 50,
    });
    await assert.rejects(
      () => r.complete({ messages: [{ role: 'user', content: 'slow' }] }),
      (err: unknown) => err instanceof Error,
    );
    slow.close();
  });
});

describe('completeStream — async iterable', () => {
  it('yields decoded SSE chunks', async () => {
    mock.setMode('stream');
    const r = createSolvelaRouter({ gatewayUrl: mock.url });
    const chunks: string[] = [];
    for await (const c of r.completeStream({
      messages: [{ role: 'user', content: 'stream' }],
      stream: true,
    })) {
      chunks.push(c.raw);
    }
    assert.ok(chunks.length >= 2);
    assert.ok(chunks.includes('[DONE]'));
  });
});
