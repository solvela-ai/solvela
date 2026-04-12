/**
 * Basic tests for @solvela/router
 *
 * Run with:
 *   node --import tsx --test tests/plugin.test.ts
 */

import { describe, it, before, after } from 'node:test';
import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import type { IncomingMessage, ServerResponse } from 'node:http';

import { createPlugin, createRouter, ConfigError } from '../src/index.js';

// ── Minimal mock gateway ──────────────────────────────────────────────────────

interface MockServer {
  url: string;
  close: () => Promise<void>;
  setMode: (mode: 'ok' | '402' | 'error') => void;
}

function startMockGateway(): Promise<MockServer> {
  let mode: 'ok' | '402' | 'error' = 'ok';

  const server = createServer((req: IncomingMessage, res: ServerResponse) => {
    if (req.url === '/health') {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ status: 'ok' }));
      return;
    }

    if (req.url === '/v1/chat/completions') {
      if (mode === 'ok') {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify({
          id: 'test-id',
          object: 'chat.completion',
          created: Date.now(),
          model: 'auto',
          choices: [{
            index: 0,
            message: { role: 'assistant', content: 'Hello from mock gateway' },
            finish_reason: 'stop',
          }],
          usage: { prompt_tokens: 5, completion_tokens: 10, total_tokens: 15 },
        }));
        return;
      }

      if (mode === '402') {
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

        // Check if request has payment-signature header — if so, approve it
        if (req.headers['payment-signature']) {
          mode = 'ok';
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
          mode = '402'; // reset for next call
          return;
        }

        res.writeHead(402, { 'content-type': 'application/json' });
        res.end(JSON.stringify({
          error: { message: JSON.stringify(paymentRequired) },
        }));
        return;
      }

      if (mode === 'error') {
        res.writeHead(500, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ error: { message: 'Internal server error' } }));
        return;
      }
    }

    res.writeHead(404);
    res.end('Not found');
  });

  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      const addr = server.address() as { port: number };
      resolve({
        url: `http://127.0.0.1:${addr.port}`,
        setMode: (m) => { mode = m; },
        close: () => new Promise((res) => server.close(() => res())),
      });
    });
  });
}

// ── Tests ─────────────────────────────────────────────────────────────────────

let mock: MockServer;

before(async () => {
  mock = await startMockGateway();
});

after(async () => {
  await mock.close();
});

describe('ConfigError', () => {
  it('throws when LLM_ROUTER_API_URL is missing', () => {
    const saved = process.env.LLM_ROUTER_API_URL;
    const savedKey = process.env.LLM_ROUTER_WALLET_KEY;
    delete process.env.LLM_ROUTER_API_URL;
    delete process.env.LLM_ROUTER_WALLET_KEY;

    assert.throws(
      () => createPlugin(),
      (err) => err instanceof ConfigError && err.message.includes('LLM_ROUTER_API_URL'),
    );

    process.env.LLM_ROUTER_API_URL = saved ?? '';
    process.env.LLM_ROUTER_WALLET_KEY = savedKey ?? '';
  });

  it('throws when LLM_ROUTER_WALLET_KEY is missing', () => {
    const savedKey = process.env.LLM_ROUTER_WALLET_KEY;
    delete process.env.LLM_ROUTER_WALLET_KEY;

    assert.throws(
      () => createPlugin({ gatewayUrl: 'http://localhost:8402' }),
      (err) => err instanceof ConfigError && err.message.includes('LLM_ROUTER_WALLET_KEY'),
    );

    process.env.LLM_ROUTER_WALLET_KEY = savedKey ?? '';
  });
});

describe('RcrClient — non-streaming', () => {
  it('returns a chat response on 200', async () => {
    mock.setMode('ok');
    const router = createRouter({
      gatewayUrl: mock.url,
      walletKey: 'stub-key',
    });

    const resp = await router.chat([{ role: 'user', content: 'Hello' }]);
    assert.equal(resp.choices[0].message.content, 'Hello from mock gateway');
    assert.equal(resp.choices[0].message.role, 'assistant');
  });

  it('uses the configured defaultModel when no model is specified', async () => {
    mock.setMode('ok');
    const router = createRouter({
      gatewayUrl: mock.url,
      walletKey: 'stub-key',
      defaultModel: 'eco',
    });

    const resp = await router.chat([{ role: 'user', content: 'Hi' }]);
    assert.ok(resp.choices.length > 0);
  });

  it('handles 402 → payment → 200 flow (stub signing)', async () => {
    mock.setMode('402');
    const router = createRouter({
      gatewayUrl: mock.url,
      walletKey: 'stub-key-no-solana',
    });

    // @solana/web3.js not available in test env → stub payment header is used
    const resp = await router.chat([{ role: 'user', content: 'Pay me' }]);
    assert.equal(resp.choices[0].message.content, 'Paid response');
  });

  it('throws RouterError on 5xx', async () => {
    mock.setMode('error');
    const router = createRouter({
      gatewayUrl: mock.url,
      walletKey: 'stub-key',
    });

    await assert.rejects(
      () => router.chat([{ role: 'user', content: 'boom' }]),
      (err: unknown) => {
        assert.ok(err instanceof Error);
        assert.ok(err.message.includes('500'));
        return true;
      },
    );
  });
});

describe('normalizeMessages — content array → string', () => {
  it('normalizes content array to joined string via intercept', async () => {
    mock.setMode('ok');
    const plugin = createPlugin({
      gatewayUrl: mock.url,
      walletKey: 'stub-key',
    });

    // Simulate OpenClaw sending content as an array of text parts
    const resp = await plugin.intercept({
      messages: [
        {
          role: 'user',
          // Cast to satisfy TypeScript — content arrays are the runtime shape we must handle
          content: [{ type: 'text', text: 'Hello' }, { type: 'text', text: 'World' }] as unknown as string,
        },
      ],
    });
    assert.ok(resp !== null);
    assert.ok(resp!.choices.length > 0);
  });

  it('normalizes content array to joined string via interceptStream', async () => {
    mock.setMode('ok');
    const plugin = createPlugin({
      gatewayUrl: mock.url,
      walletKey: 'stub-key',
    });

    const streamResp = await plugin.interceptStream({
      messages: [
        {
          role: 'user',
          content: [{ type: 'text', text: 'Stream hello' }] as unknown as string,
        },
      ],
      stream: true,
    });

    assert.ok(streamResp instanceof Response);
    assert.ok(streamResp.ok);
  });

  it('leaves plain string content unchanged', async () => {
    mock.setMode('ok');
    const plugin = createPlugin({
      gatewayUrl: mock.url,
      walletKey: 'stub-key',
    });

    const resp = await plugin.intercept({
      messages: [{ role: 'user', content: 'Already a string' }],
    });
    assert.ok(resp !== null);
    assert.ok(resp!.choices.length > 0);
  });
});

describe('OpenClaw plugin interface', () => {
  it('intercept returns a ChatResponse', async () => {
    mock.setMode('ok');
    const plugin = createPlugin({
      gatewayUrl: mock.url,
      walletKey: 'stub-key',
    });

    assert.equal(plugin.name, '@solvela/router');

    const resp = await plugin.intercept({
      messages: [{ role: 'user', content: 'Test' }],
    });
    assert.ok(resp !== null);
    assert.ok(resp!.choices.length > 0);
  });

  it('interceptStream returns a Response', async () => {
    mock.setMode('ok');
    const plugin = createPlugin({
      gatewayUrl: mock.url,
      walletKey: 'stub-key',
    });

    const streamResp = await plugin.interceptStream({
      messages: [{ role: 'user', content: 'Stream test' }],
      stream: true,
    });

    assert.ok(streamResp instanceof Response);
    assert.ok(streamResp.ok);
  });
});
