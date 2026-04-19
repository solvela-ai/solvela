/**
 * Tests for the session persistence module (src/session.ts) and
 * GatewayClient budget persistence integration.
 *
 * All tests use temp directories so they never touch ~/.solvela/mcp-session.json.
 */

import { describe, it, before } from 'node:test';
import assert from 'node:assert/strict';
import * as fs from 'node:fs/promises';
import * as os from 'node:os';
import * as path from 'node:path';

import { createSessionStore } from '../src/session.ts';
import type { SessionState } from '../src/session.ts';
import { GatewayClient } from '../src/client.ts';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async function makeTempDir(): Promise<string> {
  return fs.mkdtemp(path.join(os.tmpdir(), 'solvela-session-test-'));
}

async function cleanupDir(dir: string): Promise<void> {
  await fs.rm(dir, { recursive: true, force: true });
}

function sessionPath(dir: string): string {
  return path.join(dir, 'mcp-session.json');
}

before(() => {
  delete process.env['SOLANA_WALLET_KEY'];
  delete process.env['SOLVELA_API_URL'];
});

// ---------------------------------------------------------------------------
// Fetch mock helpers (reused from server.test.ts pattern)
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

// Shared fixture: 402 payment response with $0.002500 cost
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
  choices: [{ index: 0, message: { role: 'assistant', content: 'Reply' }, finish_reason: 'stop' }],
  usage: { prompt_tokens: 20, completion_tokens: 10, total_tokens: 30 },
};

// ---------------------------------------------------------------------------
// SessionStore unit tests
// ---------------------------------------------------------------------------

describe('SessionStore', () => {
  it('load returns defaults when file is missing', async () => {
    const dir = await makeTempDir();
    try {
      const store = createSessionStore({ path: sessionPath(dir) });
      const state = await store.load();
      assert.equal(state.session_spent, 0);
      assert.equal(state.escrow_deposits_session, 0);
      assert.equal(state.request_count, 0);
      assert.equal(state.version, 1);
      assert.ok(typeof state.last_updated === 'string');
    } finally {
      await cleanupDir(dir);
    }
  });

  it('save writes atomically and load returns saved state', async () => {
    const dir = await makeTempDir();
    try {
      const store = createSessionStore({ path: sessionPath(dir) });
      const toSave: SessionState = {
        session_spent: 1.5,
        escrow_deposits_session: 0.5,
        request_count: 7,
        last_updated: '2026-04-18T00:00:00.000Z',
        version: 1,
      };
      await store.save(toSave);

      const loaded = await store.load();
      assert.equal(loaded.session_spent, 1.5);
      assert.equal(loaded.escrow_deposits_session, 0.5);
      assert.equal(loaded.request_count, 7);
      assert.equal(loaded.last_updated, '2026-04-18T00:00:00.000Z');
      assert.equal(loaded.version, 1);
    } finally {
      await cleanupDir(dir);
    }
  });

  it('save sets file permissions to 0600 on Unix', async () => {
    if (process.platform === 'win32') return;

    const dir = await makeTempDir();
    try {
      const filePath = sessionPath(dir);
      const store = createSessionStore({ path: filePath });
      await store.save({
        session_spent: 0,
        escrow_deposits_session: 0,
        request_count: 0,
        last_updated: new Date().toISOString(),
        version: 1,
      });

      const stat = await fs.stat(filePath);
      // eslint-disable-next-line no-bitwise
      const mode = stat.mode & 0o777;
      assert.equal(mode, 0o600, `Expected 0600, got ${mode.toString(8)}`);
    } finally {
      await cleanupDir(dir);
    }
  });

  it('load returns defaults when file is not valid JSON (logs WARN)', async () => {
    const dir = await makeTempDir();
    try {
      const filePath = sessionPath(dir);
      await fs.mkdir(path.dirname(filePath), { recursive: true });
      await fs.writeFile(filePath, 'not-json', 'utf-8');

      const store = createSessionStore({ path: filePath });
      const state = await store.load();

      // Should fall back to defaults, not throw
      assert.equal(state.session_spent, 0);
      assert.equal(state.version, 1);
    } finally {
      await cleanupDir(dir);
    }
  });

  it('load returns defaults when version is unexpected (schema migration)', async () => {
    const dir = await makeTempDir();
    try {
      const filePath = sessionPath(dir);
      await fs.mkdir(path.dirname(filePath), { recursive: true });
      // Write a file with wrong version
      await fs.writeFile(
        filePath,
        JSON.stringify({ session_spent: 99, escrow_deposits_session: 0, request_count: 0, last_updated: '2026-01-01T00:00:00.000Z', version: 99 }),
        'utf-8',
      );

      const store = createSessionStore({ path: filePath });
      const state = await store.load();

      assert.equal(state.session_spent, 0, 'Should reset on unknown version');
      assert.equal(state.version, 1);
    } finally {
      await cleanupDir(dir);
    }
  });

  it('load returns defaults when numeric field is NaN', async () => {
    const dir = await makeTempDir();
    try {
      const filePath = sessionPath(dir);
      await fs.mkdir(path.dirname(filePath), { recursive: true });
      await fs.writeFile(
        filePath,
        JSON.stringify({ session_spent: 'not-a-number', escrow_deposits_session: 0, request_count: 0, last_updated: '2026-01-01T00:00:00.000Z', version: 1 }),
        'utf-8',
      );

      const store = createSessionStore({ path: filePath });
      const state = await store.load();
      assert.equal(state.session_spent, 0, 'Should reset on NaN session_spent');
    } finally {
      await cleanupDir(dir);
    }
  });

  it('load returns defaults when numeric field is negative', async () => {
    const dir = await makeTempDir();
    try {
      const filePath = sessionPath(dir);
      await fs.mkdir(path.dirname(filePath), { recursive: true });
      await fs.writeFile(
        filePath,
        JSON.stringify({ session_spent: -5, escrow_deposits_session: 0, request_count: 0, last_updated: '2026-01-01T00:00:00.000Z', version: 1 }),
        'utf-8',
      );

      const store = createSessionStore({ path: filePath });
      const state = await store.load();
      assert.equal(state.session_spent, 0, 'Should reset on negative session_spent');
    } finally {
      await cleanupDir(dir);
    }
  });

  it('reset deletes the session file', async () => {
    const dir = await makeTempDir();
    try {
      const filePath = sessionPath(dir);
      const store = createSessionStore({ path: filePath });
      await store.save({
        session_spent: 5,
        escrow_deposits_session: 0,
        request_count: 2,
        last_updated: new Date().toISOString(),
        version: 1,
      });

      // Verify file exists
      await fs.access(filePath); // throws if missing

      await store.reset();

      // File should be gone
      await assert.rejects(
        () => fs.access(filePath),
        'File should not exist after reset',
      );
    } finally {
      await cleanupDir(dir);
    }
  });

  it('reset is idempotent when file does not exist', async () => {
    const dir = await makeTempDir();
    try {
      const store = createSessionStore({ path: sessionPath(dir) });
      // Should not throw
      await store.reset();
      await store.reset();
    } finally {
      await cleanupDir(dir);
    }
  });

  it('path() returns the configured file path', async () => {
    const dir = await makeTempDir();
    try {
      const filePath = sessionPath(dir);
      const store = createSessionStore({ path: filePath });
      assert.equal(store.path(), filePath);
    } finally {
      await cleanupDir(dir);
    }
  });
});

// ---------------------------------------------------------------------------
// GatewayClient + SessionStore integration tests
// ---------------------------------------------------------------------------

describe('GatewayClient session persistence', () => {
  it('sessionSpent survives restart — persists after spend and reloads on new client', async () => {
    const dir = await makeTempDir();
    try {
      const filePath = sessionPath(dir);
      const store1 = createSessionStore({ path: filePath });

      const restore = mockFetch([
        { status: 402, body: { error: { message: JSON.stringify(payment402) } } },
        { status: 200, body: chatResp },
      ]);

      try {
        const client1 = new GatewayClient({ apiUrl: 'http://test.local', sessionStore: store1 });
        await client1.chat('openai/gpt-4o', [{ role: 'user', content: 'Hi' }]);
        const spent1 = client1.spendSummary().session_usdc_spent;
        assert.equal(spent1, '0.002500');
      } finally {
        restore();
      }

      // Create a second client with the same session file — should restore spent amount
      const store2 = createSessionStore({ path: filePath });
      const client2 = new GatewayClient({ apiUrl: 'http://test.local', sessionStore: store2 });

      // Force the state to be loaded by triggering a mock fetch that returns non-402
      // We don't make a real call — instead load directly via the store
      const reloaded = await store2.load();
      assert.equal(reloaded.session_spent, 0.0025, `Expected 0.0025 USDC, got ${reloaded.session_spent}`);

      // Verify client2 uses persisted state on first budget check
      // by making a chat that will be blocked by a tight budget derived from persistence
      const restore2 = mockFetch([
        { status: 402, body: { error: { message: JSON.stringify(payment402) } } },
      ]);
      try {
        // Budget is $0.003 — first client spent $0.0025, so $0.0005 left.
        // A new $0.0025 call should be rejected.
        const client3 = new GatewayClient({
          apiUrl: 'http://test.local',
          sessionBudget: 0.003,
          sessionStore: createSessionStore({ path: filePath }),
        });
        await assert.rejects(
          () => client3.chat('openai/gpt-4o', [{ role: 'user', content: 'Hi again' }]),
          /Session budget.*exceeded/,
          'Second client should see the persisted spend and reject over-budget call',
        );
      } finally {
        restore2();
      }

      // Suppress unused warning
      void client2;
    } finally {
      await cleanupDir(dir);
    }
  });

  it('resetSession clears file and in-memory counters', async () => {
    const dir = await makeTempDir();
    try {
      const filePath = sessionPath(dir);
      const store = createSessionStore({ path: filePath });

      const restore = mockFetch([
        { status: 402, body: { error: { message: JSON.stringify(payment402) } } },
        { status: 200, body: chatResp },
      ]);

      try {
        const client = new GatewayClient({ apiUrl: 'http://test.local', sessionStore: store });
        await client.chat('openai/gpt-4o', [{ role: 'user', content: 'Hi' }]);
        assert.equal(client.spendSummary().session_usdc_spent, '0.002500');

        await client.resetSession();

        // In-memory reset
        assert.equal(client.spendSummary().session_usdc_spent, '0.000000');
        assert.equal(client.spendSummary().total_requests, 0);

        // File should be gone
        await assert.rejects(
          () => fs.access(filePath),
          'Session file should be deleted after reset',
        );
      } finally {
        restore();
      }
    } finally {
      await cleanupDir(dir);
    }
  });

  it('concurrency — 20 parallel chats with $1 budget + $0.10 cost each → exactly 10 succeed, file reflects 10 successes', async () => {
    const dir = await makeTempDir();
    try {
      const filePath = sessionPath(dir);
      const store = createSessionStore({ path: filePath });

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
        const bodyStr = JSON.stringify(chatResp);
        return {
          status: 200,
          ok: true,
          json: () => Promise.resolve(JSON.parse(bodyStr)),
          text: () => Promise.resolve(bodyStr),
        } as unknown as Response;
      };

      try {
        const client = new GatewayClient({
          apiUrl: 'http://test.local',
          sessionBudget: 1.00,
          sessionStore: store,
        });

        const promises = Array.from({ length: 20 }, (_, i) =>
          client.chat('openai/gpt-4o', [{ role: 'user', content: `call ${i + 1}` }]),
        );
        const results = await Promise.allSettled(promises);

        const successes = results.filter((r) => r.status === 'fulfilled');
        const failures = results.filter((r) => r.status === 'rejected');

        assert.equal(successes.length, 10, `Expected 10 successes, got ${successes.length}`);
        assert.equal(failures.length, 10, `Expected 10 failures, got ${failures.length}`);

        // All failures should be budget-exceeded
        for (const f of failures) {
          const reason = (f as PromiseRejectedResult).reason as Error;
          assert.match(reason.message, /Session budget.*exceeded/);
        }

        // Verify session file reflects exactly 10 × $0.10 = $1.00
        const persisted = await store.load();
        assert.ok(
          persisted.session_spent <= 1.0,
          `Persisted session_spent ${persisted.session_spent} exceeds budget 1.0`,
        );
        assert.ok(
          Math.abs(persisted.session_spent - 1.0) < 0.001,
          `Expected ~1.0 session_spent in file, got ${persisted.session_spent}`,
        );
      } finally {
        globalThis.fetch = originalFetch;
      }
    } finally {
      await cleanupDir(dir);
    }
  });
});
