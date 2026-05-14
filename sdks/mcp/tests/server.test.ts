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
import { validateArgs } from '../src/validate-args.ts';
import { McpError, ErrorCode } from '@modelcontextprotocol/sdk/types.js';

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

  // -------------------------------------------------------------------------
  // SEC-H1: PaymentExpectations — refuse-to-sign on phishing 402 responses.
  // The chosen accept's pay_to and amount come from the gateway, which is
  // attacker-controllable (DNS hijack, MITM, misconfigured host config). The
  // client MUST refuse to sign when those values diverge from caller
  // expectations and MUST refund the budget reservation it made before
  // attempting to sign.
  // -------------------------------------------------------------------------

  it('SEC-H1: rejects 402 whose pay_to does not match expectedRecipient (and refunds budget)', async () => {
    const phishing402 = {
      ...payment402,
      accepts: [{
        ...payment402.accepts[0],
        pay_to: 'AttackerWa11et1111111111111111111111111111111', // 43 chars, valid base58 shape
      }],
    };

    const restore = mockFetch([
      { status: 402, body: { error: { message: JSON.stringify(phishing402) } } },
    ]);

    try {
      const c = new GatewayClient({
        apiUrl: 'http://test.local',
        expectedRecipient: '11111111111111111111111111111111', // what the caller pinned
      });
      await assert.rejects(
        () => c.chat('openai/gpt-4o', [{ role: 'user', content: 'Hi' }]),
        /Payment signing failed:.*recipient mismatch/,
      );
      // Budget reservation made before signing must be refunded after refusal.
      assert.equal(c.spendSummary().session_usdc_spent, '0.000000');
      assert.equal(c.spendSummary().total_requests, 0);
    } finally {
      restore();
    }
  });

  it('SEC-H1: rejects 402 whose amount exceeds the cost_breakdown.total cap (and refunds budget)', async () => {
    // Gateway returns a believable cost_breakdown.total but an inflated
    // accepts[0].amount — the SDK must catch this via maxAmount: BigInt(costMicro)
    // because the budget mutex authorized only the cost_breakdown amount.
    const inflated402 = {
      ...payment402,
      accepts: [{
        ...payment402.accepts[0],
        amount: '99999999999', // way more than the 2500 micro-USDC in cost_breakdown
      }],
      // cost_breakdown.total stays at '0.002500' so the budget reservation
      // computes costMicro = 2500. The signer must refuse the 99999999999.
    };

    const restore = mockFetch([
      { status: 402, body: { error: { message: JSON.stringify(inflated402) } } },
    ]);

    try {
      const c = new GatewayClient({ apiUrl: 'http://test.local' });
      await assert.rejects(
        () => c.chat('openai/gpt-4o', [{ role: 'user', content: 'Hi' }]),
        /Payment signing failed:.*amount cap exceeded/,
      );
      assert.equal(c.spendSummary().session_usdc_spent, '0.000000');
      assert.equal(c.spendSummary().total_requests, 0);
    } finally {
      restore();
    }
  });

  it('SEC-H1: matching expectedRecipient passes through (stub-mode 200)', async () => {
    // Sanity check the positive path: when pay_to matches, the SDK proceeds
    // normally. Without this we couldn't tell whether expectedRecipient
    // works at all vs. always rejects.
    const restore = mockFetch([
      { status: 402, body: { error: { message: JSON.stringify(payment402) } } },
      { status: 200, body: chatResp },
    ]);

    try {
      const c = new GatewayClient({
        apiUrl: 'http://test.local',
        expectedRecipient: payment402.accepts[0].pay_to,
      });
      const resp = await c.chat('openai/gpt-4o', [{ role: 'user', content: 'Hi' }]);
      assert.equal(resp.choices[0].message.content, 'Paid reply');
      assert.equal(c.spendSummary().session_usdc_spent, '0.002500');
    } finally {
      restore();
    }
  });

  // -------------------------------------------------------------------------
  // HF12: malformed-JSON 200 must refund the committed budget reservation and
  // surface a descriptive error (not a raw SyntaxError). Audit flagged this
  // as a silent over-accounting bug: budget was committed at 402 reservation
  // time but never refunded on parse failure.
  // -------------------------------------------------------------------------

  it('HF12: chat retry returns 200 with non-JSON body → refunds budget + throws descriptive error', async () => {
    // Custom mock — mockFetch helper always returns valid JSON; we need a
    // .json() that throws to simulate a proxy timeout HTML page or a
    // truncated response body.
    let callCount = 0;
    const originalFetch = globalThis.fetch;
    // @ts-expect-error — intentional mock override
    globalThis.fetch = async (_url: string, _init?: RequestInit): Promise<Response> => {
      callCount++;
      if (callCount === 1) {
        const bodyStr = JSON.stringify({ error: { message: JSON.stringify(payment402) } });
        return {
          status: 402,
          ok: false,
          json: () => Promise.resolve(JSON.parse(bodyStr)),
          text: () => Promise.resolve(bodyStr),
        } as unknown as Response;
      }
      // Retry returns 200 with a malformed body — simulates an HTML proxy page.
      return {
        status: 200,
        ok: true,
        json: () => Promise.reject(new SyntaxError('Unexpected token < in JSON at position 0')),
        text: () => Promise.resolve('<html>504 Gateway Timeout</html>'),
      } as unknown as Response;
    };

    try {
      const c = new GatewayClient({ apiUrl: 'http://test.local' });
      await assert.rejects(
        () => c.chat('openai/gpt-4o', [{ role: 'user', content: 'Hi' }]),
        /Gateway returned malformed JSON for chat.*Unexpected token/,
      );
      // Budget reservation was committed during 402 handling — must be refunded.
      assert.equal(c.spendSummary().session_usdc_spent, '0.000000');
      // requestCount was bumped just before parse — must be rolled back.
      assert.equal(c.spendSummary().total_requests, 0);
    } finally {
      globalThis.fetch = originalFetch;
    }
  });

  it('HF12: chat retry returns non-2xx → budget is refunded (regression for the existing refund path)', async () => {
    // The refund at client.ts (non-2xx retry) has been live since HF2 but
    // had no test. Lock the behavior in: 402 → sign → retry 500 → throw + refund.
    const restore = mockFetch([
      { status: 402, body: { error: { message: JSON.stringify(payment402) } } },
      { status: 500, body: { error: 'internal error' } },
    ]);

    try {
      const c = new GatewayClient({ apiUrl: 'http://test.local' });
      await assert.rejects(
        () => c.chat('openai/gpt-4o', [{ role: 'user', content: 'Hi' }]),
        /Gateway error 500/,
      );
      assert.equal(c.spendSummary().session_usdc_spent, '0.000000');
      assert.equal(c.spendSummary().total_requests, 0);
    } finally {
      restore();
    }
  });

  it('HF12: listModels with non-JSON body → throws descriptive error (not raw SyntaxError)', async () => {
    const originalFetch = globalThis.fetch;
    globalThis.fetch = async (): Promise<Response> => ({
      status: 200,
      ok: true,
      json: () => Promise.reject(new SyntaxError('Unexpected end of JSON input')),
      text: () => Promise.resolve(''),
    }) as unknown as Response;

    try {
      const c = new GatewayClient({ apiUrl: 'http://test.local' });
      await assert.rejects(
        () => c.listModels(),
        /Gateway returned malformed JSON for \/v1\/models.*Unexpected end of JSON input/,
      );
    } finally {
      globalThis.fetch = originalFetch;
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

// ---------------------------------------------------------------------------
// validateArgs — runtime guard for MCP tool dispatch (T-3A)
// ---------------------------------------------------------------------------

describe('validateArgs', () => {
  const chatSchema = {
    model: { kind: 'string', required: true },
    prompt: { kind: 'string', required: true },
    system: { kind: 'string', required: false },
    max_tokens: { kind: 'number', required: false },
    temperature: { kind: 'number', required: false },
  } as const;

  const smartSchema = {
    prompt: { kind: 'string', required: true },
    profile: { kind: 'stringEnum', required: false, values: ['eco', 'auto', 'premium', 'free'] },
  } as const;

  it('passes through valid args (chat)', () => {
    const out = validateArgs<{ model: string; prompt: string }>(
      'chat',
      { model: 'openai/gpt-4o', prompt: 'Hi' },
      chatSchema,
    );
    assert.equal(out.model, 'openai/gpt-4o');
    assert.equal(out.prompt, 'Hi');
  });

  it('rejects missing required field with InvalidParams', () => {
    try {
      validateArgs('chat', { model: 'openai/gpt-4o' }, chatSchema);
      assert.fail('expected throw');
    } catch (err) {
      assert.ok(err instanceof McpError);
      assert.equal((err as McpError).code, ErrorCode.InvalidParams);
      assert.match((err as McpError).message, /missing required field 'prompt'/);
    }
  });

  it('rejects wrong-type required field (model: number) with InvalidParams', () => {
    // The audit's example: args = { model: 42 } would flow into client.chat(42, ...)
    // without this guard.
    try {
      validateArgs('chat', { model: 42, prompt: 'Hi' }, chatSchema);
      assert.fail('expected throw');
    } catch (err) {
      assert.ok(err instanceof McpError);
      assert.equal((err as McpError).code, ErrorCode.InvalidParams);
      assert.match((err as McpError).message, /field 'model' must be a string, got number/);
    }
  });

  it('rejects wrong-type optional field (max_tokens: string) with InvalidParams', () => {
    try {
      validateArgs('chat', { model: 'x', prompt: 'y', max_tokens: '100' }, chatSchema);
      assert.fail('expected throw');
    } catch (err) {
      assert.match((err as McpError).message, /field 'max_tokens' must be a finite number, got string/);
    }
  });

  it('rejects NaN as a number field', () => {
    try {
      validateArgs('chat', { model: 'x', prompt: 'y', temperature: NaN }, chatSchema);
      assert.fail('expected throw');
    } catch (err) {
      assert.match((err as McpError).message, /temperature.*finite number/);
    }
  });

  it('allows omitted optional fields', () => {
    const out = validateArgs<{ model: string; prompt: string }>(
      'chat',
      { model: 'x', prompt: 'y' },
      chatSchema,
    );
    assert.equal(out.model, 'x');
  });

  it('rejects bad enum value (smart_chat profile = "premium!!")', () => {
    try {
      validateArgs('smart_chat', { prompt: 'hi', profile: 'premium!!' }, smartSchema);
      assert.fail('expected throw');
    } catch (err) {
      assert.ok(err instanceof McpError);
      assert.equal((err as McpError).code, ErrorCode.InvalidParams);
      assert.match((err as McpError).message, /profile.*must be one of 'eco'\|'auto'\|'premium'\|'free'/);
    }
  });

  it('accepts each declared enum value', () => {
    for (const p of ['eco', 'auto', 'premium', 'free']) {
      const out = validateArgs<{ prompt: string; profile?: string }>(
        'smart_chat',
        { prompt: 'hi', profile: p },
        smartSchema,
      );
      assert.equal(out.profile, p);
    }
  });

  it('rejects non-object args (null, array, primitive)', () => {
    for (const bad of [null, [], 'string', 42, true]) {
      try {
        validateArgs('chat', bad, chatSchema);
        assert.fail(`expected throw for ${JSON.stringify(bad)}`);
      } catch (err) {
        assert.ok(err instanceof McpError, `not an McpError: ${err}`);
        assert.match((err as McpError).message, /arguments must be an object/);
      }
    }
  });

  it('boolean field — accepts true/false, rejects "true" string', () => {
    const out = validateArgs<{ reset?: boolean }>(
      'spending',
      { reset: true },
      { reset: { kind: 'boolean', required: false } },
    );
    assert.equal(out.reset, true);

    try {
      validateArgs(
        'spending',
        { reset: 'true' },
        { reset: { kind: 'boolean', required: false } },
      );
      assert.fail('expected throw');
    } catch (err) {
      assert.match((err as McpError).message, /reset.*must be a boolean, got string/);
    }
  });
});
