/**
 * MCP server unit tests.
 *
 * Tests the GatewayClient and tool definitions without requiring a live
 * gateway or npm build — all gateway HTTP calls are intercepted via
 * global fetch mocking using Node.js's built-in test runner.
 *
 * All tests run in "without-key mode" (SOLANA_WALLET_KEY not set). The SDK
 * deliberately returns a stub payload in that mode — this tests the 402 retry
 * flow and budget enforcement without requiring a real Solana RPC connection.
 */

import { describe, it, before, afterEach } from 'node:test';
import assert from 'node:assert/strict';

// ---------------------------------------------------------------------------
// Import modules under test (no build needed — Node strips TS with --experimental-strip-types)
// ---------------------------------------------------------------------------

import { GatewayClient } from '../src/client.ts';
import { TOOLS } from '../src/tools.ts';

// ---------------------------------------------------------------------------
// Ensure signing key is absent for all tests (without-key mode)
// ---------------------------------------------------------------------------

before(() => {
  delete process.env['SOLANA_WALLET_KEY'];
});

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
// Shared 402 payload fixtures
// ---------------------------------------------------------------------------

const payment402 = {
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

// ---------------------------------------------------------------------------
// GatewayClient tests
// ---------------------------------------------------------------------------

describe('GatewayClient', () => {
  it('uses SOLVELA_API_URL env var when no apiUrl option provided', () => {
    process.env['SOLVELA_API_URL'] = 'http://localhost:9998';
    const c = new GatewayClient();
    assert.equal(c.apiUrl, 'http://localhost:9998');
    delete process.env['SOLVELA_API_URL'];
  });

  it('falls back to RCR_API_URL for compat when SOLVELA_API_URL is not set', () => { // compat
    delete process.env['SOLVELA_API_URL'];
    process.env['RCR_API_URL'] = 'http://localhost:9999'; // compat
    const c = new GatewayClient();
    assert.equal(c.apiUrl, 'http://localhost:9999');
    delete process.env['RCR_API_URL']; // compat
  });

  it('strips trailing slash from apiUrl', () => {
    const c = new GatewayClient({ apiUrl: 'http://localhost:8402/' });
    assert.equal(c.apiUrl, 'http://localhost:8402');
  });

  it('falls back to production URL when no env var or option', () => {
    delete process.env['RCR_API_URL']; // compat
    delete process.env['SOLVELA_API_URL'];
    const c = new GatewayClient();
    assert.equal(c.apiUrl, 'https://api.solvela.ai');
  });

  it('chat succeeds on 200', async () => {
    const resp200 = {
      id: 'test-id',
      object: 'chat.completion',
      created: 1234567890,
      model: 'openai/gpt-4o',
      choices: [{ index: 0, message: { role: 'assistant', content: 'Hello!' }, finish_reason: 'stop' }],
      usage: { prompt_tokens: 10, completion_tokens: 5, total_tokens: 15 },
    };

    const restore = mockFetch([{ status: 200, body: resp200 }]);
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

  it('chat handles 402 → retry flow (without-key mode — stub payment header)', async () => {
    // Without SOLANA_WALLET_KEY the SDK returns a stub payload — the 402 retry still
    // exercises the full client flow (parse, filter, sign, retry with header).
    const restore = mockFetch([
      { status: 402, body: { error: { message: JSON.stringify(payment402) } } },
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

  it('throws budget-exceeded error when session budget would be exceeded', async () => {
    const bigPayment = {
      ...payment402,
      accepts: [{ ...payment402.accepts[0], amount: '5000' }],
      cost_breakdown: { ...payment402.cost_breakdown, total: '0.005000' },
    };

    const restore = mockFetch([
      { status: 402, body: { error: { message: JSON.stringify(bigPayment) } } },
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

  it('race condition — two parallel calls under budget only allow one through (T1-H)', async () => {
    // Budget: $0.10. Each call costs $0.08. Only one should succeed.
    const costPayment = {
      ...payment402,
      cost_breakdown: { ...payment402.cost_breakdown, total: '0.08' },
    };

    let callCount = 0;
    const originalFetch = globalThis.fetch;
    // @ts-expect-error — intentional mock override
    globalThis.fetch = async (_url: string, _init?: RequestInit): Promise<Response> => {
      callCount++;
      const call = callCount;
      if (call <= 2) {
        // HF4: Yield to microtask queue to force real interleaving into the mutex.
        await new Promise((r) => setImmediate(r));
        // The two parallel initial calls both get 402
        const bodyStr = JSON.stringify({ error: { message: JSON.stringify(costPayment) } });
        return {
          status: 402,
          ok: false,
          json: () => Promise.resolve(JSON.parse(bodyStr)),
          text: () => Promise.resolve(bodyStr),
        } as unknown as Response;
      }
      // Any retry after the first gets 200
      const bodyStr = JSON.stringify(chatResp);
      return {
        status: 200,
        ok: true,
        json: () => Promise.resolve(JSON.parse(bodyStr)),
        text: () => Promise.resolve(bodyStr),
      } as unknown as Response;
    };

    try {
      const c = new GatewayClient({ apiUrl: 'http://test.local', sessionBudget: 0.10 });
      const results = await Promise.allSettled([
        c.chat('openai/gpt-4o', [{ role: 'user', content: 'call 1' }]),
        c.chat('openai/gpt-4o', [{ role: 'user', content: 'call 2' }]),
      ]);

      const successes = results.filter((r) => r.status === 'fulfilled');
      const failures = results.filter((r) => r.status === 'rejected');

      assert.equal(successes.length, 1, 'Exactly one call should succeed under budget');
      assert.equal(failures.length, 1, 'Exactly one call should be rejected for budget exceeded');

      const failedReason = (failures[0] as PromiseRejectedResult).reason as Error;
      assert.match(failedReason.message, /Session budget.*exceeded/);
    } finally {
      globalThis.fetch = originalFetch;
    }
  });

  it('race stress — 20 parallel calls with budget for exactly 10 do not overrun (HF4)', async () => {
    // Budget: $1.00. Each call costs $0.10. Exactly 10 should succeed.
    const costPayment = {
      ...payment402,
      cost_breakdown: { ...payment402.cost_breakdown, total: '0.10' },
    };

    let callCount = 0;
    const originalFetch = globalThis.fetch;
    // @ts-expect-error — intentional mock override
    globalThis.fetch = async (_url: string, _init?: RequestInit): Promise<Response> => {
      callCount++;
      const call = callCount;
      // First 20 calls are the initial probes — all return 402 with a microtask yield.
      if (call <= 20) {
        await new Promise((r) => setImmediate(r));
        const bodyStr = JSON.stringify({ error: { message: JSON.stringify(costPayment) } });
        return {
          status: 402,
          ok: false,
          json: () => Promise.resolve(JSON.parse(bodyStr)),
          text: () => Promise.resolve(bodyStr),
        } as unknown as Response;
      }
      // Retries (signed requests) all get 200.
      const bodyStr = JSON.stringify(chatResp);
      return {
        status: 200,
        ok: true,
        json: () => Promise.resolve(JSON.parse(bodyStr)),
        text: () => Promise.resolve(bodyStr),
      } as unknown as Response;
    };

    try {
      const c = new GatewayClient({ apiUrl: 'http://test.local', sessionBudget: 1.00 });
      const promises = Array.from({ length: 20 }, (_, i) =>
        c.chat('openai/gpt-4o', [{ role: 'user', content: `call ${i + 1}` }]),
      );
      const results = await Promise.allSettled(promises);

      const successes = results.filter((r) => r.status === 'fulfilled');
      const failures = results.filter((r) => r.status === 'rejected');

      // Exactly 10 should succeed (budget = $1.00, cost = $0.10 each).
      assert.equal(successes.length, 10, `Expected 10 successes, got ${successes.length}`);
      assert.equal(failures.length, 10, `Expected 10 failures, got ${failures.length}`);

      // All failures must be budget-exceeded errors, not something else.
      for (const f of failures) {
        const reason = (f as PromiseRejectedResult).reason as Error;
        assert.match(
          reason.message,
          /Session budget.*exceeded/,
          `Unexpected failure reason: ${reason.message}`,
        );
      }

      // sessionSpent must not exceed budget.
      const spend = c.spendSummary();
      const spent = parseFloat(spend.session_usdc_spent);
      assert.ok(spent <= 1.0, `sessionSpent ${spent} exceeded budget 1.0`);
    } finally {
      globalThis.fetch = originalFetch;
    }
  });

  it('filterAccepts — escrow mode rejects if no escrow accepts', async () => {
    // No escrow scheme in 402 response; 'escrow' mode should throw before signing.
    const restore = mockFetch([
      { status: 402, body: { error: { message: JSON.stringify(payment402) } } },
    ]);

    try {
      const c = new GatewayClient({ apiUrl: 'http://test.local', signingMode: 'escrow' });
      await assert.rejects(
        () => c.chat('openai/gpt-4o', [{ role: 'user', content: 'Hi' }]),
        /No payment accepts match signing mode 'escrow'/,
      );
    } finally {
      restore();
    }
  });

  it('filterAccepts — direct mode passes exact scheme through', async () => {
    // 'exact' scheme is non-escrow; direct mode should accept it.
    const restore = mockFetch([
      { status: 402, body: { error: { message: JSON.stringify(payment402) } } },
      { status: 200, body: chatResp },
    ]);

    try {
      const c = new GatewayClient({ apiUrl: 'http://test.local', signingMode: 'direct' });
      const resp = await c.chat('openai/gpt-4o', [{ role: 'user', content: 'Hi' }]);
      assert.equal(resp.choices[0].message.content, 'Paid reply');
    } finally {
      restore();
    }
  });

  it('signing mode off — skips payment header and proceeds', async () => {
    // In 'off' mode after a 402 the client sends without a payment header;
    // if gateway then returns 200 (dev bypass), client succeeds.
    const restore = mockFetch([
      { status: 402, body: { error: { message: JSON.stringify(payment402) } } },
      { status: 200, body: chatResp },
    ]);

    try {
      const c = new GatewayClient({ apiUrl: 'http://test.local', signingMode: 'off' });
      const resp = await c.chat('openai/gpt-4o', [{ role: 'user', content: 'Hi' }]);
      assert.equal(resp.choices[0].message.content, 'Paid reply');
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
