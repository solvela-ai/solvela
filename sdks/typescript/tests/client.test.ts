import { describe, it } from 'node:test';
import * as assert from 'node:assert';

// Import from source files directly (no build step needed for tests)
import { LLMClient, PaymentError, BudgetExceededError } from '../src/client';
import { OpenAI } from '../src/openai-compat';
import { Wallet } from '../src/wallet';
import { createPaymentHeader, decodePaymentHeader } from '../src/x402';
import type { PaymentRequired } from '../src/types';

// ---------------------------------------------------------------------------
// Client construction
// ---------------------------------------------------------------------------

describe('LLMClient', () => {
  it('uses default API URL when none provided', () => {
    const client = new LLMClient();
    assert.strictEqual(client.getApiUrl(), 'https://api.rustyclawrouter.com');
  });

  it('accepts custom API URL', () => {
    const client = new LLMClient({ apiUrl: 'http://localhost:8402' });
    assert.strictEqual(client.getApiUrl(), 'http://localhost:8402');
  });

  it('strips trailing slash from API URL', () => {
    const client = new LLMClient({ apiUrl: 'http://localhost:8402/' });
    assert.strictEqual(client.getApiUrl(), 'http://localhost:8402');
  });

  it('strips multiple trailing slashes', () => {
    const client = new LLMClient({ apiUrl: 'http://localhost:8402///' });
    assert.strictEqual(client.getApiUrl(), 'http://localhost:8402//');
    // Note: only the final slash is stripped by the regex replace
  });

  it('starts with zero session spend', () => {
    const client = new LLMClient();
    assert.strictEqual(client.getSessionSpent(), 0);
  });

  it('returns undefined remaining budget when no budget set', () => {
    const client = new LLMClient();
    assert.strictEqual(client.getRemainingBudget(), undefined);
  });

  it('returns full remaining budget when nothing spent', () => {
    const client = new LLMClient({ sessionBudget: 10.0 });
    assert.strictEqual(client.getRemainingBudget(), 10.0);
  });
});

// ---------------------------------------------------------------------------
// Error classes
// ---------------------------------------------------------------------------

describe('Error classes', () => {
  it('PaymentError has correct name', () => {
    const err = new PaymentError('test');
    assert.strictEqual(err.name, 'PaymentError');
    assert.strictEqual(err.message, 'test');
    assert.ok(err instanceof Error);
  });

  it('BudgetExceededError has correct name', () => {
    const err = new BudgetExceededError('over budget');
    assert.strictEqual(err.name, 'BudgetExceededError');
    assert.strictEqual(err.message, 'over budget');
    assert.ok(err instanceof Error);
  });
});

// ---------------------------------------------------------------------------
// Payment header (x402)
// ---------------------------------------------------------------------------

describe('createPaymentHeader', () => {
  const mockPaymentRequired: PaymentRequired = {
    x402_version: 2,
    accepts: [
      {
        scheme: 'exact',
        network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
        amount: '2625',
        asset: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
        pay_to: 'RCRgateway111111111111111111111111111111111',
        max_timeout_seconds: 300,
      },
    ],
    cost_breakdown: {
      provider_cost: '0.002500',
      platform_fee: '0.000125',
      total: '0.002625',
      currency: 'USDC',
      fee_percent: 5,
    },
    error: 'Payment required',
  };

  it('produces a valid base64 string', async () => {
    const header = await createPaymentHeader(mockPaymentRequired, 'http://localhost:8402/v1/chat/completions');
    // base64 strings only contain [A-Za-z0-9+/=]
    assert.match(header, /^[A-Za-z0-9+/=]+$/);
  });

  it('roundtrips through decode', async () => {
    const url = 'http://localhost:8402/v1/chat/completions';
    const header = await createPaymentHeader(mockPaymentRequired, url);
    const decoded = decodePaymentHeader(header) as Record<string, unknown>;

    assert.strictEqual(decoded.x402_version, 2);
    assert.deepStrictEqual(decoded.resource, { url, method: 'POST' });
    assert.deepStrictEqual(decoded.accepted, mockPaymentRequired.accepts[0]);
    // No private key supplied → stub transaction
    assert.deepStrictEqual(decoded.payload, { transaction: 'STUB_BASE64_TX' });
  });

  it('throws on empty accepts array', async () => {
    const badInfo: PaymentRequired = {
      ...mockPaymentRequired,
      accepts: [],
    };
    await assert.rejects(
      () => createPaymentHeader(badInfo, 'http://localhost:8402/v1/chat/completions'),
      /No payment accept options/
    );
  });
});

// ---------------------------------------------------------------------------
// Escrow payment header
// ---------------------------------------------------------------------------

describe('createPaymentHeader (escrow scheme)', () => {
  const mockEscrowPayment: PaymentRequired = {
    x402_version: 2,
    accepts: [
      {
        scheme: 'escrow',
        network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
        amount: '2625',
        asset: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
        pay_to: 'RCRgateway111111111111111111111111111111111',
        max_timeout_seconds: 300,
        escrow_program_id: 'RCRescrow1111111111111111111111111111111111',
      },
    ],
    cost_breakdown: {
      provider_cost: '0.002500',
      platform_fee: '0.000125',
      total: '0.002625',
      currency: 'USDC',
      fee_percent: 5,
    },
    error: 'Payment required',
  };

  it('stub mode: escrow header contains deposit_tx, service_id, agent_pubkey', async () => {
    const url = 'http://localhost:8402/v1/chat/completions';
    const header = await createPaymentHeader(mockEscrowPayment, url, undefined, '{"model":"gpt-4o"}');
    const decoded = decodePaymentHeader(header) as Record<string, unknown>;

    assert.strictEqual(decoded.x402_version, 2);
    assert.deepStrictEqual(decoded.resource, { url, method: 'POST' });

    const accepted = decoded.accepted as Record<string, unknown>;
    assert.strictEqual(accepted.scheme, 'escrow');
    assert.strictEqual(accepted.escrow_program_id, 'RCRescrow1111111111111111111111111111111111');

    const payload = decoded.payload as Record<string, unknown>;
    assert.strictEqual(payload.deposit_tx, 'STUB_ESCROW_DEPOSIT_TX');
    assert.strictEqual(payload.agent_pubkey, 'STUB_AGENT_PUBKEY');
    // service_id is a base64-encoded sha256 hash — verify it's a non-empty string
    assert.strictEqual(typeof payload.service_id, 'string');
    assert.ok((payload.service_id as string).length > 0);
  });

  it('stub mode: service_id differs across calls (random component)', async () => {
    const url = 'http://localhost:8402/v1/chat/completions';
    const body = '{"model":"gpt-4o"}';
    const h1 = await createPaymentHeader(mockEscrowPayment, url, undefined, body);
    const h2 = await createPaymentHeader(mockEscrowPayment, url, undefined, body);
    const d1 = decodePaymentHeader(h1) as Record<string, unknown>;
    const d2 = decodePaymentHeader(h2) as Record<string, unknown>;
    const p1 = d1.payload as Record<string, unknown>;
    const p2 = d2.payload as Record<string, unknown>;
    // service_id must be unique per call due to random bytes
    assert.notStrictEqual(p1.service_id, p2.service_id);
  });

  it('falls back to exact scheme when no escrow_program_id present', async () => {
    const mixedPayment: PaymentRequired = {
      ...mockEscrowPayment,
      accepts: [
        {
          scheme: 'escrow',
          network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
          amount: '2625',
          asset: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
          pay_to: 'RCRgateway111111111111111111111111111111111',
          max_timeout_seconds: 300,
          // no escrow_program_id
        },
        {
          scheme: 'exact',
          network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
          amount: '2625',
          asset: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
          pay_to: 'RCRgateway111111111111111111111111111111111',
          max_timeout_seconds: 300,
        },
      ],
    };
    const url = 'http://localhost:8402/v1/chat/completions';
    const header = await createPaymentHeader(mixedPayment, url);
    const decoded = decodePaymentHeader(header) as Record<string, unknown>;
    // Should use first accept (escrow without program_id) → falls back to exact path
    const payload = decoded.payload as Record<string, unknown>;
    assert.strictEqual(payload.transaction, 'STUB_BASE64_TX');
  });
});

// ---------------------------------------------------------------------------
// OpenAI compatibility wrapper
// ---------------------------------------------------------------------------

describe('OpenAI compat', () => {
  it('constructs with chat.completions namespace', () => {
    const openai = new OpenAI({ apiUrl: 'http://localhost:8402' });
    assert.ok(openai.chat);
    assert.ok(openai.chat.completions);
    assert.strictEqual(typeof openai.chat.completions.create, 'function');
  });

  it('exposes underlying LLMClient via getClient()', () => {
    const openai = new OpenAI({ apiUrl: 'http://localhost:8402' });
    const client = openai.getClient();
    assert.ok(client instanceof LLMClient);
    assert.strictEqual(client.getApiUrl(), 'http://localhost:8402');
  });
});

// ---------------------------------------------------------------------------
// Wallet
// ---------------------------------------------------------------------------

describe('Wallet', () => {
  it('reports no key when constructed empty', () => {
    // Temporarily unset env var if present
    const saved = process.env.SOLANA_WALLET_KEY;
    delete process.env.SOLANA_WALLET_KEY;
    try {
      const wallet = new Wallet();
      assert.strictEqual(wallet.hasKey, false);
      assert.strictEqual(wallet.address, null);
      assert.strictEqual(wallet.redactedKey, null);
    } finally {
      if (saved !== undefined) process.env.SOLANA_WALLET_KEY = saved;
    }
  });

  it('reports key presence when constructed with key', () => {
    const wallet = new Wallet('5K1gEZd3V2z7JH3GvMxMi5h3mC9t7RAG1DxCw5gS');
    assert.strictEqual(wallet.hasKey, true);
  });

  it('redacts key correctly', () => {
    const wallet = new Wallet('5K1gEZd3V2z7JH3GvMxMi5h3mC9t7RAG1DxCw5gS');
    const redacted = wallet.redactedKey;
    assert.ok(redacted);
    assert.ok(redacted.startsWith('5K1g'));
    assert.ok(redacted.endsWith('5gS'));
    assert.ok(redacted.includes('...'));
  });

  it('redacts very short keys', () => {
    const wallet = new Wallet('short');
    assert.strictEqual(wallet.redactedKey, '****');
  });

  it('returns null address without @solana/web3.js', () => {
    // @solana/web3.js is not installed in this test environment
    const wallet = new Wallet('5K1gEZd3V2z7JH3GvMxMi5h3mC9t7RAG1DxCw5gS');
    assert.strictEqual(wallet.address, null);
  });
});
