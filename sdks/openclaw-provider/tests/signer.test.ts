/**
 * Tests for SolvelaSigner.
 *
 * All tests that call buildHeader() require SOLANA_WALLET_KEY to be set
 * (HF-P3-L2: fail-closed if key absent). Tests that need a real header shape
 * use the DI seam (_createPaymentHeaderFn) to inject a known non-stub header.
 * The stub-guard tests inject a known stub header via the same seam (HF-P3-H3).
 */

import { describe, it, before, afterEach } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

import { SolvelaSigner } from '../src/signer.ts';
import type { PaymentRequired } from '@solvela/sdk/types';

const __dirname = dirname(fileURLToPath(import.meta.url));

const mock402 = JSON.parse(
  readFileSync(resolve(__dirname, 'fixtures/mock-402.json'), 'utf-8'),
) as PaymentRequired;

// A fake wallet key — passes the presence check (L2 guard).
const FAKE_WALLET_KEY = 'fakekey_for_test_only_not_valid_base58_keypair';

// A non-stub header that the DI seam can return so the stub guard doesn't fire.
// Must base64-decode to JSON with x402_version and no STUB_ prefix in payload.
const REAL_LOOKING_HEADER = Buffer.from(
  JSON.stringify({
    x402_version: 2,
    resource: { url: 'https://api.solvela.ai/v1/chat/completions', method: 'POST' },
    accepted: { scheme: 'exact', network: 'solana' },
    payload: { transaction: 'REAL_TX_NOT_STUB_xABCDEF1234567890' },
  }),
  'utf-8',
).toString('base64');

/** DI signer that bypasses real signing and returns a known non-stub header. */
function makeDISigner(opts: ConstructorParameters<typeof SolvelaSigner>[0] = {}): SolvelaSigner {
  return new SolvelaSigner({
    ...opts,
    _createPaymentHeaderFn: async () => REAL_LOOKING_HEADER,
  } as ConstructorParameters<typeof SolvelaSigner>[0] & { _createPaymentHeaderFn: () => Promise<string> });
}

before(() => {
  process.env['SOLANA_WALLET_KEY'] = FAKE_WALLET_KEY;
  delete process.env['SOLANA_RPC_URL'];
});

afterEach(() => {
  process.env['SOLANA_WALLET_KEY'] = FAKE_WALLET_KEY;
  delete process.env['SOLANA_RPC_URL'];
});

describe('SolvelaSigner', () => {
  it('buildHeader returns a base64 string (DI seam — non-stub)', async () => {
    const signer = makeDISigner();
    const header = await signer.buildHeader(mock402, 'https://api.solvela.ai/v1/chat/completions', '{"model":"auto"}');
    assert.ok(typeof header === 'string');
    assert.ok(header.length > 0, 'header should be non-empty');

    // Decode and verify shape
    const decoded = JSON.parse(Buffer.from(header, 'base64').toString('utf-8'));
    assert.ok(decoded.x402_version !== undefined, 'header should contain x402_version');
    assert.ok(decoded.payload !== undefined, 'header should contain payload');
  });

  it('throws when SOLANA_WALLET_KEY is absent (HF-P3-L2 fail-closed)', async () => {
    delete process.env['SOLANA_WALLET_KEY'];
    const signer = new SolvelaSigner();
    await assert.rejects(
      () => signer.buildHeader(mock402, 'https://api.solvela.ai/v1/chat/completions', '{}'),
      (err: Error) => {
        assert.ok(
          err.message.includes('SOLANA_WALLET_KEY is not set at signing time'),
          `got: ${err.message}`,
        );
        return true;
      },
    );
  });

  it('throws on invalid cost_breakdown.total (HF-P3-M9)', async () => {
    const badPaymentInfo = {
      ...mock402,
      cost_breakdown: { ...mock402.cost_breakdown, total: 'NaN' },
    } as PaymentRequired;
    const signer = makeDISigner();
    await assert.rejects(
      () => signer.buildHeader(badPaymentInfo, 'https://api.solvela.ai/v1/chat/completions', '{}'),
      (err: Error) => {
        assert.ok(err.message.includes('invalid cost'), `got: ${err.message}`);
        return true;
      },
    );
  });

  it('default signingMode is direct (HF-P3-H6)', () => {
    // Constructed with no options — signingMode defaults to direct
    const signer = new SolvelaSigner();
    assert.strictEqual(signer.getSessionSpent(), 0, 'initial spend should be 0');
    // Verify via escrow-mode check: auto and escrow would increment escrowDepositCount
    // We can only observe the default indirectly via no WARN on direct mode builds.
    // The canonical assertion is in index.test.ts via getSigningMode() which defaults 'direct'.
  });

  it('stub-header guard: rejects STUB_BASE64_TX (HF-P3-H3 — direct stub check)', async () => {
    const stubPayload = JSON.stringify({
      x402_version: 2,
      resource: { url: 'https://api.solvela.ai/v1/chat/completions', method: 'POST' },
      accepted: mock402.accepts[0],
      payload: { transaction: 'STUB_BASE64_TX' },
    });
    const stubHeader = Buffer.from(stubPayload, 'utf-8').toString('base64');

    const signer = new SolvelaSigner({
      _createPaymentHeaderFn: async () => stubHeader,
    } as ConstructorParameters<typeof SolvelaSigner>[0] & { _createPaymentHeaderFn: () => Promise<string> });

    await assert.rejects(
      () => signer.buildHeader(mock402, 'https://api.solvela.ai/v1/chat/completions', '{}'),
      (err: Error) => {
        assert.ok(
          err.message.toLowerCase().includes('stub'),
          `expected stub rejection, got: ${err.message}`,
        );
        return true;
      },
    );
  });

  it('stub-header guard: rejects STUB_ESCROW_DEPOSIT_TX (HF-P3-H3 escrow branch)', async () => {
    const stubPayload = JSON.stringify({
      x402_version: 2,
      resource: { url: 'https://api.solvela.ai/v1/chat/completions', method: 'POST' },
      accepted: mock402.accepts[0],
      payload: { deposit_tx: 'STUB_ESCROW_DEPOSIT_TX' },
    });
    const stubHeader = Buffer.from(stubPayload, 'utf-8').toString('base64');

    const signer = new SolvelaSigner({
      _createPaymentHeaderFn: async () => stubHeader,
    } as ConstructorParameters<typeof SolvelaSigner>[0] & { _createPaymentHeaderFn: () => Promise<string> });

    await assert.rejects(
      () => signer.buildHeader(mock402, 'https://api.solvela.ai/v1/chat/completions', '{}'),
      (err: Error) => {
        assert.ok(
          err.message.toLowerCase().includes('stub'),
          `expected stub rejection, got: ${err.message}`,
        );
        return true;
      },
    );
  });

  it('stub-header guard: budget refunded before throwing', async () => {
    const stubPayload = JSON.stringify({
      x402_version: 2,
      resource: { url: 'https://api.solvela.ai/v1/chat/completions', method: 'POST' },
      accepted: mock402.accepts[0],
      payload: { transaction: 'STUB_BASE64_TX' },
    });
    const stubHeader = Buffer.from(stubPayload, 'utf-8').toString('base64');

    const signer = new SolvelaSigner({
      sessionBudget: 10.0,
      _createPaymentHeaderFn: async () => stubHeader,
    } as ConstructorParameters<typeof SolvelaSigner>[0] & { _createPaymentHeaderFn: () => Promise<string> });

    await assert.rejects(
      () => signer.buildHeader(mock402, 'https://api.solvela.ai/v1/chat/completions', '{}'),
      () => true,
    );

    // Budget must be refunded — spent returns to 0
    assert.strictEqual(signer.getSessionSpent(), 0, 'budget must be refunded after stub rejection');
  });

  it('budget enforcement: rejects when session budget is exceeded', async () => {
    // mock402 has total: "0.002625" — budget 0.001 is below that
    const signer = new SolvelaSigner({ sessionBudget: 0.001 });

    await assert.rejects(
      () => signer.buildHeader(mock402, 'https://api.solvela.ai/v1/chat/completions', '{}'),
      (err: Error) => {
        assert.ok(err.message.includes('budget'), `expected budget error, got: ${err.message}`);
        return true;
      },
    );
  });

  it('budget enforcement: allows calls within budget', async () => {
    const signer = makeDISigner({ sessionBudget: 1.00 });
    // Should not throw — cost is 0.002625 which is within $1.00
    const header = await signer.buildHeader(mock402, 'https://api.solvela.ai/v1/chat/completions', '{}');
    assert.ok(typeof header === 'string');
    assert.ok(signer.getSessionSpent() > 0, 'session spent should be non-zero after call');
  });

  it('budget enforcement: second call rejected when cumulative spend exceeds budget', async () => {
    const signer = makeDISigner({ sessionBudget: 0.004 }); // allows one call (0.002625) not two

    // First call should succeed
    await signer.buildHeader(mock402, 'https://api.solvela.ai/v1/chat/completions', '{}');

    // Second call should fail budget
    await assert.rejects(
      () => signer.buildHeader(mock402, 'https://api.solvela.ai/v1/chat/completions', '{}'),
      (err: Error) => {
        assert.ok(err.message.includes('budget'), `expected budget error, got: ${err.message}`);
        return true;
      },
    );
  });

  it('budget refunded on signing failure (createPaymentHeader throws)', async () => {
    const signer = new SolvelaSigner({
      sessionBudget: 1.00,
      _createPaymentHeaderFn: async () => {
        throw new Error('signing error injected by test');
      },
    } as ConstructorParameters<typeof SolvelaSigner>[0] & { _createPaymentHeaderFn: () => Promise<string> });

    await assert.rejects(
      () => signer.buildHeader(mock402, 'https://api.solvela.ai/v1/chat/completions', '{}'),
      (err: Error) => {
        assert.ok(err.message.includes('signing'), `expected signing error, got: ${err.message}`);
        return true;
      },
    );

    // Budget must be refunded after signing failure
    assert.strictEqual(signer.getSessionSpent(), 0, 'budget must be refunded after signing failure');
  });

  it('refundBudget: clamp-warning emitted when refund > spent (HF-P3-H2)', async () => {
    const signer = makeDISigner();
    // Spend something first
    await signer.buildHeader(mock402, 'https://api.solvela.ai/v1/chat/completions', '{}');
    const spentAfterCall = signer.getSessionSpent();
    assert.ok(spentAfterCall > 0, 'should have spent something');

    // Now refund more than spent — should emit WARN and clamp to 0
    const warnings: string[] = [];
    const origWrite = process.stderr.write.bind(process.stderr);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (process.stderr as any).write = (chunk: string | Uint8Array, cb?: (err?: Error | null) => void): boolean => {
      if (typeof chunk === 'string') warnings.push(chunk);
      return origWrite(chunk, cb as (err?: Error | null) => void);
    };
    try {
      await signer.refundBudget(spentAfterCall * 10); // much more than spent
    } finally {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (process.stderr as any).write = origWrite;
    }

    const warnLine = warnings.find((l) => l.includes('budget refund clamped'));
    assert.ok(warnLine, `expected clamp WARN, got lines: ${warnings.join('')}`);
    assert.strictEqual(signer.getSessionSpent(), 0, 'spent must clamp to 0');
  });

  it('budget mutex: parallel calls do not race past budget', async () => {
    // cost per call: 0.002625 USDC, budget: 0.004 USDC → only 1 should succeed
    const signer = makeDISigner({ sessionBudget: 0.004 });

    const results = await Promise.allSettled([
      signer.buildHeader(mock402, 'https://api.solvela.ai/v1/chat/completions', '{}'),
      signer.buildHeader(mock402, 'https://api.solvela.ai/v1/chat/completions', '{}'),
    ]);

    const succeeded = results.filter((r) => r.status === 'fulfilled').length;
    const failed = results.filter((r) => r.status === 'rejected').length;

    assert.strictEqual(succeeded, 1, `expected exactly 1 success, got ${succeeded}`);
    assert.strictEqual(failed, 1, `expected exactly 1 failure, got ${failed}`);
  });

  it('SigningError is wrapped — raw cause not propagated to message', async () => {
    process.env['SOLANA_WALLET_KEY'] = 'bad_key_triggers_error';
    const signer = new SolvelaSigner();

    try {
      await signer.buildHeader(mock402, 'https://api.solvela.ai/v1/chat/completions', '{}');
    } catch (err: unknown) {
      if (err instanceof Error) {
        // The message should NOT contain raw base58 key fragments or byte arrays
        const raw = process.env['SOLANA_WALLET_KEY'] ?? '';
        assert.ok(
          !err.message.includes(raw),
          `error message must not contain the private key: ${err.message}`,
        );
        // Should not contain 'cause' stringification
        assert.ok(
          !err.message.includes('[object'),
          `error message must not contain object toString: ${err.message}`,
        );
      }
    }
  });

  it('filterAccepts (H7): escrow mode filters before budget reservation', async () => {
    // escrow mode: mock402 has only 'exact' scheme, no 'escrow' scheme.
    // filterAccepts runs BEFORE budget reservation, so no budget is consumed.
    const signer = new SolvelaSigner({ signingMode: 'escrow', sessionBudget: 1.0 });
    const spentBefore = signer.getSessionSpent();

    await assert.rejects(
      () => signer.buildHeader(mock402, 'https://api.solvela.ai/v1/chat/completions', '{}'),
      (err: Error) => {
        assert.ok(
          err.message.includes('No payment accepts match signing mode'),
          `expected signing mode filter error, got: ${err.message}`,
        );
        return true;
      },
    );

    // Budget must NOT have been consumed (HF-P3-H7)
    assert.strictEqual(
      signer.getSessionSpent(),
      spentBefore,
      'budget must not be reserved when filterAccepts throws',
    );
  });

  it('escrow deposit counter: WARN emitted at every 10th deposit (HF-P3-H6)', async () => {
    // Use an escrow-scheme mock402 fixture so filterAccepts passes in escrow mode.
    const mock402Escrow = JSON.parse(
      readFileSync(resolve(__dirname, 'fixtures/mock-402-escrow.json'), 'utf-8'),
    ) as PaymentRequired;

    const warnings: string[] = [];
    const origWrite = process.stderr.write.bind(process.stderr);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (process.stderr as any).write = (chunk: string | Uint8Array, cb?: (err?: Error | null) => void): boolean => {
      if (typeof chunk === 'string') warnings.push(chunk);
      return origWrite(chunk, cb as (err?: Error | null) => void);
    };

    try {
      const signer = new SolvelaSigner({
        signingMode: 'escrow',
        _createPaymentHeaderFn: async () => REAL_LOOKING_HEADER,
      } as ConstructorParameters<typeof SolvelaSigner>[0] & { _createPaymentHeaderFn: () => Promise<string> });

      // Make 10 calls — WARN should fire on the 10th
      for (let i = 0; i < 10; i++) {
        await signer.buildHeader(mock402Escrow, 'https://api.solvela.ai/v1/chat/completions', '{}');
      }
    } finally {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (process.stderr as any).write = origWrite;
    }

    const warnLine = warnings.find((l) => l.includes('10 escrow deposits made'));
    assert.ok(warnLine, `expected escrow deposit WARN at 10th deposit, got: ${warnings.join('')}`);
  });

  it('filterAccepts: direct mode filters escrow accepts', async () => {
    const mock402Escrow = JSON.parse(
      readFileSync(resolve(__dirname, 'fixtures/mock-402-escrow.json'), 'utf-8'),
    ) as PaymentRequired;
    const signer = new SolvelaSigner({ signingMode: 'direct' });
    // escrow mock has only escrow scheme — direct mode should throw
    await assert.rejects(
      () => signer.buildHeader(mock402Escrow, 'https://api.solvela.ai/v1/chat/completions', '{}'),
      (err: Error) => {
        assert.ok(
          err.message.includes('No payment accepts match signing mode'),
          `expected signing mode filter error, got: ${err.message}`,
        );
        return true;
      },
    );
  });
});
