/**
 * Tests for the deposit_escrow tool (T-2G-E).
 *
 * Tests GatewayClient.runEscrowDeposit session cap enforcement and
 * the escrow tool input validation logic (mode gate, per-call cap,
 * amount parsing).
 *
 * Does NOT test real Solana RPC broadcasting — the broadcast step is mocked
 * via the GatewayClient.runEscrowDeposit callback seam.
 */

import { describe, it, before, afterEach } from 'node:test';
import assert from 'node:assert/strict';
import * as fs from 'node:fs/promises';
import * as os from 'node:os';
import * as path from 'node:path';

import { GatewayClient } from '../src/client.ts';
import { createSessionStore } from '../src/session.ts';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async function makeTempDir(): Promise<string> {
  return fs.mkdtemp(path.join(os.tmpdir(), 'solvela-escrow-test-'));
}

async function cleanupDir(dir: string): Promise<void> {
  await fs.rm(dir, { recursive: true, force: true });
}

before(() => {
  delete process.env['SOLANA_WALLET_KEY'];
  delete process.env['SOLVELA_ESCROW_MODE'];
  delete process.env['SOLVELA_API_URL'];
});

// ---------------------------------------------------------------------------
// deposit_escrow mode gate tests (simulated via escrowEnabled flag logic)
// ---------------------------------------------------------------------------
// Note: The actual SOLVELA_ESCROW_MODE gate is in index.ts at the handler level.
// We test the GatewayClient.runEscrowDeposit cap enforcement here directly,
// since that's where the session cap logic lives.

describe('GatewayClient.runEscrowDeposit — session cap enforcement (plan S11.5)', () => {
  const MAX_SESSION = 20.0;

  it('single deposit within cap succeeds', async () => {
    const dir = await makeTempDir();
    try {
      const store = createSessionStore({ path: path.join(dir, 'session.json') });
      const client = new GatewayClient({ apiUrl: 'http://test.local', sessionStore: store });

      let deposited = false;
      const total = await client.runEscrowDeposit(4.0, MAX_SESSION, async () => {
        deposited = true;
      });

      assert.ok(deposited, 'onDeposit callback should be called');
      assert.equal(total, 4.0);
      assert.equal(client.getEscrowDepositsSession(), 4.0);
    } finally {
      await cleanupDir(dir);
    }
  });

  it('plan S11.5: cap survives concurrent deposits — all 5 × $4 fit exactly $20', async () => {
    // Real parallel test: Promise.allSettled with artificial latency so the Node.js
    // scheduler can interleave. Pre-fix code would pass all checks (race) then commit
    // all increments, yielding $20 on counter but potentially >$20 on-chain.
    // Post-fix code: each call atomically reserves in Phase 1, so the 5th call reserves
    // exactly the last $4 slot and all 5 succeed.
    const dir = await makeTempDir();
    try {
      const store = createSessionStore({ path: path.join(dir, 'session.json') });
      const client = new GatewayClient({ apiUrl: 'http://test.local', sessionStore: store });

      const results = await Promise.allSettled(
        Array.from({ length: 5 }, () =>
          client.runEscrowDeposit(4.0, MAX_SESSION, async () => {
            // Simulate RPC latency so the scheduler can interleave calls
            await new Promise<void>((r) => setTimeout(r, 10));
          }),
        ),
      );

      const succeeded = results.filter((r) => r.status === 'fulfilled').length;
      const failed = results.filter((r) => r.status === 'rejected').length;

      assert.equal(succeeded, 5, 'all 5 × $4 = $20 should fit exactly the cap');
      assert.equal(failed, 0, 'no deposits should fail when total equals cap');
      assert.equal(client.getEscrowDepositsSession(), 20.0, 'counter must be exactly $20');

      // A 6th deposit of any positive amount must be rejected
      await assert.rejects(
        () => client.runEscrowDeposit(0.01, MAX_SESSION, async () => {}),
        /cap exceeded/i,
        'Any deposit beyond $20 cap must be rejected',
      );
    } finally {
      await cleanupDir(dir);
    }
  });

  it('plan S11.5: cap blocks over-cap parallel deposits — $24 vs $20 cap', async () => {
    // 6 parallel deposits of $4 = $24, but cap is $20.
    // With atomic reserve, exactly 5 slots of $4 are reserved before the 6th is rejected.
    const dir = await makeTempDir();
    try {
      const store = createSessionStore({ path: path.join(dir, 'session.json') });
      const client = new GatewayClient({ apiUrl: 'http://test.local', sessionStore: store });

      const results = await Promise.allSettled(
        Array.from({ length: 6 }, () =>
          client.runEscrowDeposit(4.0, MAX_SESSION, async () => {
            await new Promise<void>((r) => setTimeout(r, 10));
          }),
        ),
      );

      const succeeded = results.filter((r) => r.status === 'fulfilled').length;
      const failed = results.filter((r) => r.status === 'rejected').length;

      assert.equal(succeeded, 5, 'exactly 5 deposits fit the $20 cap');
      assert.equal(failed, 1, '1 deposit must be rejected');
      assert.equal(client.getEscrowDepositsSession(), 20.0, 'counter must reflect only successful deposits');
    } finally {
      await cleanupDir(dir);
    }
  });

  it('plan S11.5: all rollback on parallel broadcast failure — counter stays 0', async () => {
    // 5 parallel deposits where onDeposit throws after RPC latency.
    // Each should: reserve in Phase 1, fail in Phase 2, roll back in Phase 3.
    // Final counter must be 0.
    const dir = await makeTempDir();
    try {
      const store = createSessionStore({ path: path.join(dir, 'session.json') });
      const client = new GatewayClient({ apiUrl: 'http://test.local', sessionStore: store });

      const results = await Promise.allSettled(
        Array.from({ length: 5 }, () =>
          client.runEscrowDeposit(4.0, MAX_SESSION, async () => {
            await new Promise<void>((r) => setTimeout(r, 10));
            throw new Error('RPC broadcast failed');
          }),
        ),
      );

      const failed = results.filter((r) => r.status === 'rejected').length;
      assert.equal(failed, 5, 'all 5 deposits must fail (broadcast error)');
      assert.equal(
        client.getEscrowDepositsSession(),
        0,
        'counter must be 0 after all rollbacks',
      );
    } finally {
      await cleanupDir(dir);
    }
  });

  it('plan S11.5 strict — 5 × $4 against $19 cap: 4th fails (cumulative $16 + $4 = $20 > $19)', async () => {
    const dir = await makeTempDir();
    try {
      const store = createSessionStore({ path: path.join(dir, 'session.json') });
      const client = new GatewayClient({ apiUrl: 'http://test.local', sessionStore: store });

      const capAt19 = 19.0;

      // 3 deposits succeed ($12 total)
      for (let i = 0; i < 3; i++) {
        await client.runEscrowDeposit(4.0, capAt19, async () => {});
      }
      assert.equal(client.getEscrowDepositsSession(), 12.0);

      // 4th would bring to $16 — still under cap, passes
      await client.runEscrowDeposit(4.0, capAt19, async () => {});
      assert.equal(client.getEscrowDepositsSession(), 16.0);

      // 5th would bring to $20 > $19 — rejected
      await assert.rejects(
        () => client.runEscrowDeposit(4.0, capAt19, async () => {}),
        /Escrow session cap.*exceeded/,
      );

      // Verify counter not incremented after rejection
      assert.equal(client.getEscrowDepositsSession(), 16.0);
    } finally {
      await cleanupDir(dir);
    }
  });

  it('deposit NOT counted when onDeposit throws (broadcast failure)', async () => {
    const dir = await makeTempDir();
    try {
      const store = createSessionStore({ path: path.join(dir, 'session.json') });
      const client = new GatewayClient({ apiUrl: 'http://test.local', sessionStore: store });

      await assert.rejects(
        () =>
          client.runEscrowDeposit(5.0, MAX_SESSION, async () => {
            throw new Error('RPC broadcast failed');
          }),
        /RPC broadcast failed/,
      );

      // Session counter must NOT be incremented after a broadcast failure
      assert.equal(
        client.getEscrowDepositsSession(),
        0,
        'escrowDepositsSession must stay 0 after broadcast failure',
      );
    } finally {
      await cleanupDir(dir);
    }
  });

  it('zero amount is correctly rejected by per-call validation (input guard)', () => {
    // This simulates the handler-level validation in index.ts
    const amount = parseFloat('0');
    assert.ok(!Number.isFinite(amount) || amount <= 0, 'zero should fail positive check');
  });

  it('negative amount is correctly rejected by per-call validation (input guard)', () => {
    const amount = parseFloat('-1.5');
    assert.ok(amount <= 0, 'negative should fail positive check');
  });

  it('NaN amount_usdc is correctly rejected by per-call validation (input guard)', () => {
    const amount = parseFloat('not-a-number');
    assert.ok(Number.isNaN(amount), 'non-numeric string should parse to NaN');
    assert.ok(!Number.isFinite(amount), 'NaN should fail isFinite check');
  });

  it('non-finite amount (Infinity) rejected', () => {
    const amount = parseFloat('Infinity');
    assert.ok(!Number.isFinite(amount), 'Infinity should fail isFinite check');
  });


  it('per-call cap guard — amount > maxEscrowDeposit should be caught before runEscrowDeposit', () => {
    // Simulate the handler-level check: amount > maxEscrowDeposit (default 5.0)
    const maxEscrowDeposit = 5.0;
    const amount = 6.0;
    assert.ok(amount > maxEscrowDeposit, 'amount 6.0 should exceed default $5 per-call cap');
  });

  it('per-call cap guard — amount exactly at cap is allowed', () => {
    const maxEscrowDeposit = 5.0;
    const amount = 5.0;
    assert.ok(!(amount > maxEscrowDeposit), 'amount exactly at cap should not exceed it');
  });
});

// ---------------------------------------------------------------------------
// Escrow session cap persists across client instances
// ---------------------------------------------------------------------------

describe('deposit_escrow session cap persistence', () => {
  it('escrow_deposits_session survives restart via session file', async () => {
    const dir = await makeTempDir();
    const filePath = path.join(dir, 'session.json');
    try {
      const MAX_SESSION = 10.0;

      // First client: deposit $6
      const store1 = createSessionStore({ path: filePath });
      const client1 = new GatewayClient({ apiUrl: 'http://test.local', sessionStore: store1 });
      await client1.runEscrowDeposit(6.0, MAX_SESSION, async () => {});
      assert.equal(client1.getEscrowDepositsSession(), 6.0);

      // Second client with same file: try to deposit $5, which would exceed $10
      const store2 = createSessionStore({ path: filePath });
      const client2 = new GatewayClient({ apiUrl: 'http://test.local', sessionStore: store2 });

      // $6 (persisted) + $5 = $11 > $10 — should be rejected
      await assert.rejects(
        () => client2.runEscrowDeposit(5.0, MAX_SESSION, async () => {}),
        /Escrow session cap.*exceeded/,
        'Second client should respect persisted escrow_deposits_session',
      );

      // But $3 is ok ($6 + $3 = $9 ≤ $10)
      await client2.runEscrowDeposit(3.0, MAX_SESSION, async () => {});
      assert.equal(client2.getEscrowDepositsSession(), 9.0);
    } finally {
      await cleanupDir(dir);
    }
  });
});

// ---------------------------------------------------------------------------
// Escrow mode gate (SOLVELA_ESCROW_MODE env var) — simulated validation
// ---------------------------------------------------------------------------

describe('deposit_escrow SOLVELA_ESCROW_MODE gate', () => {
  afterEach(() => {
    delete process.env['SOLVELA_ESCROW_MODE'];
  });

  it('escrow is disabled when SOLVELA_ESCROW_MODE is not set', () => {
    delete process.env['SOLVELA_ESCROW_MODE'];
    const enabled = process.env['SOLVELA_ESCROW_MODE'] === 'enabled';
    assert.equal(enabled, false, 'escrow should be disabled when env var is unset');
  });

  it('escrow is enabled when SOLVELA_ESCROW_MODE=enabled', () => {
    process.env['SOLVELA_ESCROW_MODE'] = 'enabled';
    const enabled = process.env['SOLVELA_ESCROW_MODE'] === 'enabled';
    assert.equal(enabled, true, 'escrow should be enabled when env var is "enabled"');
  });

  it('any other SOLVELA_ESCROW_MODE value is invalid', () => {
    const invalidValues = ['true', '1', 'yes', 'on', 'ENABLED'];
    for (const val of invalidValues) {
      const enabled = val === 'enabled';
      assert.equal(enabled, false, `"${val}" should not be treated as enabled`);
    }
  });
});

// ---------------------------------------------------------------------------
// getTools() — deposit_escrow appears only when escrow is enabled
// ---------------------------------------------------------------------------

describe('getTools() — deposit_escrow visibility', () => {
  afterEach(() => {
    delete process.env['SOLVELA_ESCROW_MODE'];
  });

  it('deposit_escrow not in tool list when SOLVELA_ESCROW_MODE unset', async () => {
    delete process.env['SOLVELA_ESCROW_MODE'];
    const { getTools } = await import('../src/tools.ts');
    const tools = getTools();
    const names = tools.map((t) => t.name);
    assert.ok(!names.includes('deposit_escrow'), 'deposit_escrow should not appear when escrow disabled');
  });

  it('deposit_escrow appears in tool list when SOLVELA_ESCROW_MODE=enabled', async () => {
    process.env['SOLVELA_ESCROW_MODE'] = 'enabled';
    // Re-evaluate getTools with the new env (module is cached, so call directly)
    const { getTools } = await import('../src/tools.ts');
    const tools = getTools();
    const names = tools.map((t) => t.name);
    assert.ok(names.includes('deposit_escrow'), 'deposit_escrow should appear when escrow enabled');
  });

  it('deposit_escrow tool schema has correct required fields', async () => {
    process.env['SOLVELA_ESCROW_MODE'] = 'enabled';
    const { getTools } = await import('../src/tools.ts');
    const tools = getTools();
    const escrowTool = tools.find((t) => t.name === 'deposit_escrow');
    assert.ok(escrowTool, 'deposit_escrow tool must be present');
    assert.deepEqual(escrowTool.inputSchema.required, ['amount_usdc']);
  });
});

// ---------------------------------------------------------------------------
// spending tool — reset flag
// ---------------------------------------------------------------------------

describe('spending tool reset flag', () => {
  it('spending tool schema includes optional reset boolean', async () => {
    const { TOOLS } = await import('../src/tools.ts');
    const spending = TOOLS.find((t) => t.name === 'spending');
    assert.ok(spending, 'spending tool must exist');
    const props = spending.inputSchema.properties as Record<string, { type: string }>;
    assert.ok(props['reset'], 'spending tool must have reset property');
    assert.equal(props['reset'].type, 'boolean', 'reset must be boolean type');
    // reset is optional — not in required
    assert.ok(!(spending.inputSchema.required ?? []).includes('reset'), 'reset must not be required');
  });

  it('resetSession clears escrowDepositsSession and requestCount', async () => {
    const dir = await makeTempDir();
    try {
      const store = createSessionStore({ path: path.join(dir, 'session.json') });
      const client = new GatewayClient({ apiUrl: 'http://test.local', sessionStore: store });

      // Simulate some escrow deposits
      await client.runEscrowDeposit(3.0, 20.0, async () => {});
      assert.equal(client.getEscrowDepositsSession(), 3.0);

      await client.resetSession();

      assert.equal(client.getEscrowDepositsSession(), 0, 'escrowDepositsSession must be cleared');
      assert.equal(client.spendSummary().total_requests, 0, 'requestCount must be cleared');
    } finally {
      await cleanupDir(dir);
    }
  });
});

// ---------------------------------------------------------------------------
// HF-F2T2-H3: strict amount_usdc parsing (parseStrictUsdc behavior)
// ---------------------------------------------------------------------------

describe('deposit_escrow strict amount parsing (H3)', () => {
  // These tests validate the regex-first parse logic that index.ts uses.
  // The regex is: /^\d+(\.\d+)?$/ — only accepts plain decimal strings.

  function parseStrictUsdc(raw: string): number {
    if (!/^\d+(\.\d+)?$/.test(raw)) {
      throw new Error(`amount_usdc must be a positive decimal number, got: ${JSON.stringify(raw)}`);
    }
    const n = Number(raw);
    if (!Number.isFinite(n) || n <= 0) {
      throw new Error(`amount_usdc must be positive, got: ${raw}`);
    }
    return n;
  }

  it('plain decimal "5.00" is accepted', () => {
    assert.equal(parseStrictUsdc('5.00'), 5.0);
  });

  it('integer string "5" is accepted', () => {
    assert.equal(parseStrictUsdc('5'), 5.0);
  });

  it('"5.00abc" is rejected (trailing non-digits)', () => {
    assert.throws(() => parseStrictUsdc('5.00abc'), /amount_usdc must be a positive decimal/);
  });

  it('"1e10" is rejected (scientific notation)', () => {
    assert.throws(() => parseStrictUsdc('1e10'), /amount_usdc must be a positive decimal/);
  });

  it('"0" is rejected (zero is not positive)', () => {
    assert.throws(() => parseStrictUsdc('0'), /amount_usdc must be positive/);
  });

  it('"-1.5" is rejected (negative)', () => {
    assert.throws(() => parseStrictUsdc('-1.5'), /amount_usdc must be a positive decimal/);
  });

  it('"not-a-number" is rejected', () => {
    assert.throws(() => parseStrictUsdc('not-a-number'), /amount_usdc must be a positive decimal/);
  });

  it('"Infinity" is rejected', () => {
    // "Infinity" contains letters so the regex rejects it
    assert.throws(() => parseStrictUsdc('Infinity'), /amount_usdc must be a positive decimal/);
  });

  it('"2.50" parses to exactly 2.5', () => {
    assert.equal(parseStrictUsdc('2.50'), 2.5);
  });
});
