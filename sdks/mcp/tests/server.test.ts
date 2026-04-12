/**
 * MCP server unit tests.
 *
 * Tests the GatewayClient and tool definitions without requiring a live
 * gateway or npm build — all gateway HTTP calls are intercepted via
 * global fetch mocking using Node.js's built-in test runner.
 */

import { describe, it, before, afterEach, mock } from 'node:test';
import assert from 'node:assert/strict';

// ---------------------------------------------------------------------------
// Import modules under test (no build needed — Node strips TS with --experimental-strip-types)
// ---------------------------------------------------------------------------

import { GatewayClient } from '../src/client.ts';
import { TOOLS } from '../src/tools.ts';

// ---------------------------------------------------------------------------
// Fetch mock helpers
// ---------------------------------------------------------------------------

function mockFetch(responses: Array<{ status: number; body: unknown }>) {
  let callIndex = 0;
  const originalFetch = globalThis.fetch;

  // @ts-expect-error — intentional mock override
  globalThis.fetch = async (_url: string, _init?: RequestInit): Promise<Response> => {
    const entry = responses[callIndex++] ?? responses[responses.length - 1];
    const bodyStr = JSON.stringify(entry.body);
    return {
      status: entry.status,
      ok: entry.status >= 200 && entry.status < 300,
      json: () => Promise.resolve(JSON.parse(bodyStr)),
      text: () => Promise.resolve(bodyStr),
    } as unknown as Response;
  };

  return () => {
    globalThis.fetch = originalFetch;
    callIndex = 0;
  };
}

// ---------------------------------------------------------------------------
// GatewayClient tests
// ---------------------------------------------------------------------------

describe('GatewayClient', () => {
  it('uses RCR_API_URL env var when no apiUrl option provided', () => {
    process.env['RCR_API_URL'] = 'http://localhost:9999';
    const c = new GatewayClient();
    assert.equal(c.apiUrl, 'http://localhost:9999');
    delete process.env['RCR_API_URL'];
  });

  it('strips trailing slash from apiUrl', () => {
    const c = new GatewayClient({ apiUrl: 'http://localhost:8402/' });
    assert.equal(c.apiUrl, 'http://localhost:8402');
  });

  it('falls back to production URL when no env var or option', () => {
    delete process.env['RCR_API_URL'];
    delete process.env['SOLVELA_API_URL'];
    const c = new GatewayClient();
    assert.equal(c.apiUrl, 'https://api.solvela.ai');
  });

  it('chat succeeds on 200', async () => {
    const chatResp = {
      id: 'test-id',
      object: 'chat.completion',
      created: 1234567890,
      model: 'openai/gpt-4o',
      choices: [{ index: 0, message: { role: 'assistant', content: 'Hello!' }, finish_reason: 'stop' }],
      usage: { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 },
    };

    const restore = mockFetch([{ status: 200, body: chatResp }]);
    try {
      const c = new GatewayClient({ apiUrl: 'http://test.local' });
      const resp = await c.chat('openai/gpt-4o', [{ role: 'user', content: 'Hi' }]);
      assert.equal(resp.choices[0].message.content, 'Hello!');
      assert.equal(c.spendSummary().total_requests, 1);
      assert.equal(c.spendSummary().session_usdc_spent, '0.000000');
    } finally {
      restore();
    }
  });

  it('chat handles 402 → retry flow', async () => {
    const paymentRequired = {
      x402_version: 2,
      accepts: [{
        scheme: 'exact',
        network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
        amount: '2500',
        asset: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
        pay_to: '11111111111111111111111111111111',
        max_timeout_seconds: 300,
      }],
      cost_breakdown: {
        provider_cost: '0.002375',
        platform_fee: '0.000125',
        total: '0.002500',
        currency: 'USDC',
        fee_percent: 5,
      },
      error: 'Payment required',
    };

    const chatResp = {
      id: 'paid-id',
      object: 'chat.completion',
      created: 1234567890,
      model: 'openai/gpt-4o',
      choices: [{ index: 0, message: { role: 'assistant', content: 'Paid reply' }, finish_reason: 'stop' }],
      usage: { prompt_tokens: 20, completion_tokens: 10, total_tokens: 30 },
    };

    // First call: 402. Second call (with payment header): 200.
    const restore = mockFetch([
      { status: 402, body: { error: { message: JSON.stringify(paymentRequired) } } },
      { status: 200, body: chatResp },
    ]);

    try {
      const c = new GatewayClient({ apiUrl: 'http://test.local' });
      const resp = await c.chat('openai/gpt-4o', [{ role: 'user', content: 'Hi' }]);
      assert.equal(resp.choices[0].message.content, 'Paid reply');

      const spend = c.spendSummary();
      assert.equal(spend.total_requests, 1);
      assert.equal(spend.session_usdc_spent, '0.002500');
    } finally {
      restore();
    }
  });

  it('throws BudgetExceededError when session budget would be exceeded', async () => {
    const paymentRequired = {
      x402_version: 2,
      accepts: [{
        scheme: 'exact',
        network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
        amount: '5000',
        asset: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
        pay_to: '11111111111111111111111111111111',
        max_timeout_seconds: 300,
      }],
      cost_breakdown: {
        provider_cost: '0.004750',
        platform_fee: '0.000250',
        total: '0.005000',
        currency: 'USDC',
        fee_percent: 5,
      },
      error: 'Payment required',
    };

    const restore = mockFetch([
      { status: 402, body: { error: { message: JSON.stringify(paymentRequired) } } },
    ]);

    try {
      const c = new GatewayClient({ apiUrl: 'http://test.local', sessionBudget: 0.001 });
      await assert.rejects(
        () => c.chat('openai/gpt-4o', [{ role: 'user', content: 'Hi' }]),
        /Session budget.*exceeded/,
      );
    } finally {
      restore();
    }
  });

  it('listModels returns model list', async () => {
    const modelsResp = {
      object: 'list',
      data: [
        { id: 'openai/gpt-4o', object: 'model', owned_by: 'openai' },
        { id: 'anthropic/claude-sonnet-4', object: 'model', owned_by: 'anthropic' },
      ],
    };

    const restore = mockFetch([{ status: 200, body: modelsResp }]);
    try {
      const c = new GatewayClient({ apiUrl: 'http://test.local' });
      const resp = await c.listModels();
      assert.equal(resp.data.length, 2);
      assert.equal(resp.data[0].id, 'openai/gpt-4o');
    } finally {
      restore();
    }
  });

  it('health returns gateway status', async () => {
    const restore = mockFetch([{ status: 200, body: { status: 'ok', solana_rpc: 'connected' } }]);
    try {
      const c = new GatewayClient({ apiUrl: 'http://test.local' });
      const resp = await c.health();
      assert.equal(resp.status, 'ok');
      assert.equal(resp.solana_rpc, 'connected');
    } finally {
      restore();
    }
  });

  it('spendSummary shows budget_remaining when budget is set', () => {
    const c = new GatewayClient({ apiUrl: 'http://test.local', sessionBudget: 1.0 });
    const spend = c.spendSummary();
    assert.equal(spend.budget_remaining, '1.000000');
  });

  it('spendSummary shows null budget_remaining when no budget set', () => {
    const c = new GatewayClient({ apiUrl: 'http://test.local' });
    assert.equal(c.spendSummary().budget_remaining, null);
  });
});

// ---------------------------------------------------------------------------
// Tools definition tests (no network)
// ---------------------------------------------------------------------------

describe('TOOLS', () => {
  it('exports exactly 5 tools', () => {
    assert.equal(TOOLS.length, 5);
  });

  it('tool names are correct', () => {
    const names = TOOLS.map((t) => t.name);
    assert.deepEqual(names.sort(), ['chat', 'list_models', 'smart_chat', 'spending', 'wallet_status']);
  });

  it('chat tool requires model and prompt', () => {
    const chat = TOOLS.find((t) => t.name === 'chat')!;
    assert.deepEqual(chat.inputSchema.required, ['model', 'prompt']);
  });

  it('smart_chat tool requires only prompt', () => {
    const sc = TOOLS.find((t) => t.name === 'smart_chat')!;
    assert.deepEqual(sc.inputSchema.required, ['prompt']);
  });

  it('wallet_status tool requires no inputs', () => {
    const ws = TOOLS.find((t) => t.name === 'wallet_status')!;
    assert.deepEqual(ws.inputSchema.required, []);
  });

  it('smart_chat profile enum contains expected values', () => {
    const sc = TOOLS.find((t) => t.name === 'smart_chat')!;
    const profileProp = (sc.inputSchema.properties as Record<string, { enum?: string[] }>)['profile'];
    assert.deepEqual(profileProp?.enum, ['eco', 'auto', 'premium', 'free']);
  });

  it('all tools have non-empty descriptions', () => {
    for (const tool of TOOLS) {
      assert.ok(tool.description && tool.description.length > 10, `Tool ${tool.name} has no description`);
    }
  });
});
