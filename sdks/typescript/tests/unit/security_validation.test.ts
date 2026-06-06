import { describe, it, expect, vi, afterEach } from 'vitest';
import { SolvelaClient } from '../../src/client.js';
import {
  ChatRequest,
  ChatMessage,
  PaymentAccept,
  PaymentRequired,
} from '../../src/types.js';
import {
  ClientError,
  SignerError,
  AmountExceedsMaxError,
  EscrowProgramMismatchError,
} from '../../src/errors.js';
import type { Signer } from '../../src/signer.js';
import type { Resource } from '../../src/types.js';
import { PaymentPayload, SolanaPayload } from '../../src/types.js';
import { SOLANA_NETWORK, USDC_MINT, MAINNET_ESCROW_PROGRAM_ID } from '../../src/constants.js';

/**
 * Coverage for security audit fixes:
 *  - HIGH-1  parseInt NaN bypass (validatePayment + handlePaymentRequired)
 *  - NEW-HIGH validate network/asset/pay_to before signing
 *  - HIGH-2  http:// gatewayUrl rejected for non-loopback hosts
 */

class StubSigner implements Signer {
  public called = false;
  async signPayment(
    _amount: number,
    _recipient: string,
    resource: Resource,
    accepted: PaymentAccept,
  ): Promise<PaymentPayload> {
    this.called = true;
    return new PaymentPayload(
      2,
      resource,
      accepted,
      new SolanaPayload('fakeTx=='),
    );
  }
}

function pr(overrides: Partial<{
  amount: string;
  network: string;
  asset?: string;
  payTo: string;
  scheme: string;
  escrowProgramId?: string;
}>): Record<string, unknown> {
  const accepted: Record<string, unknown> = {
    scheme: overrides.scheme ?? 'exact',
    network: overrides.network ?? SOLANA_NETWORK,
    amount: overrides.amount ?? '1000000',
    asset: overrides.asset ?? USDC_MINT,
    pay_to: overrides.payTo ?? '11111111111111111111111111111111',
    max_timeout_seconds: 300,
  };
  if (overrides.escrowProgramId !== undefined) {
    accepted.escrow_program_id = overrides.escrowProgramId;
  }
  return {
    x402_version: 2,
    resource: { url: '/v1/chat/completions', method: 'POST' },
    accepts: [accepted],
    cost_breakdown: {
      provider_cost: '0.0005',
      platform_fee: '0.000025',
      total: '0.000525',
      currency: 'USDC',
      fee_percent: 5,
    },
    error: 'Payment required',
  };
}

function mockFetchOnce(
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

describe('Security: parseAtomicAmount NaN guard', () => {
  const originalFetch = globalThis.fetch;
  afterEach(() => {
    globalThis.fetch = originalFetch;
  });

  it('rejects non-numeric amount', async () => {
    mockFetchOnce(402, pr({ amount: 'free' }));

    const signer = new StubSigner();
    const client = new SolvelaClient({ signer });
    const request = new ChatRequest('gpt-4', [new ChatMessage('user', 'Hi')]);

    await expect(client.chat(request)).rejects.toThrow(SignerError);
    expect(signer.called).toBe(false);
  });

  it('rejects negative amount', async () => {
    mockFetchOnce(402, pr({ amount: '-5' }));

    const signer = new StubSigner();
    const client = new SolvelaClient({ signer });
    const request = new ChatRequest('gpt-4', [new ChatMessage('user', 'Hi')]);

    await expect(client.chat(request)).rejects.toThrow(SignerError);
    expect(signer.called).toBe(false);
  });

  it('rejects zero amount', async () => {
    mockFetchOnce(402, pr({ amount: '0' }));

    const signer = new StubSigner();
    const client = new SolvelaClient({ signer });
    const request = new ChatRequest('gpt-4', [new ChatMessage('user', 'Hi')]);

    await expect(client.chat(request)).rejects.toThrow(SignerError);
    expect(signer.called).toBe(false);
  });

  it('rejects fractional amount (parseInt would silently truncate)', async () => {
    // Pre-fix: parseInt("1500000.99", 10) === 1500000, sneaking past the cap
    // and signing a smaller value than the gateway's literal string would imply.
    mockFetchOnce(402, pr({ amount: '1500000.99' }));

    const signer = new StubSigner();
    const client = new SolvelaClient({ signer });
    await expect(
      client.chat(new ChatRequest('gpt-4', [new ChatMessage('user', 'Hi')])),
    ).rejects.toThrow(SignerError);
    expect(signer.called).toBe(false);
  });

  it('rejects amount with trailing garbage', async () => {
    // Pre-fix: parseInt("1000000abc", 10) === 1000000.
    mockFetchOnce(402, pr({ amount: '1000000abc' }));

    const signer = new StubSigner();
    const client = new SolvelaClient({ signer });
    await expect(
      client.chat(new ChatRequest('gpt-4', [new ChatMessage('user', 'Hi')])),
    ).rejects.toThrow(SignerError);
    expect(signer.called).toBe(false);
  });

  it('rejects scientific notation', async () => {
    // Pre-fix: parseInt("1e6", 10) === 1.
    mockFetchOnce(402, pr({ amount: '1e6' }));

    const signer = new StubSigner();
    const client = new SolvelaClient({ signer });
    await expect(
      client.chat(new ChatRequest('gpt-4', [new ChatMessage('user', 'Hi')])),
    ).rejects.toThrow(SignerError);
    expect(signer.called).toBe(false);
  });

  it('rejects amount above Number.MAX_SAFE_INTEGER (precision loss)', async () => {
    // 2^53 + 1 — Number(...) would round and return an inexact integer.
    mockFetchOnce(402, pr({ amount: '9007199254740993' }));

    const signer = new StubSigner();
    const client = new SolvelaClient({ signer });
    await expect(
      client.chat(new ChatRequest('gpt-4', [new ChatMessage('user', 'Hi')])),
    ).rejects.toThrow(SignerError);
    expect(signer.called).toBe(false);
  });

  it('enforces default maxPaymentAmount cap (10 USDC atomic)', async () => {
    // 11 USDC > 10 USDC default cap
    mockFetchOnce(402, pr({ amount: '11000000' }));

    const signer = new StubSigner();
    const client = new SolvelaClient({ signer });
    const request = new ChatRequest('gpt-4', [new ChatMessage('user', 'Hi')]);

    await expect(client.chat(request)).rejects.toThrow(AmountExceedsMaxError);
    expect(signer.called).toBe(false);
  });

  it('accepts valid atomic amounts (positive-path regression guard)', async () => {
    // Lock in that the validator still ACCEPTS canonical decimal-integer
    // strings — guards against a future regex tightening that would silently
    // reject legitimate values.
    let call = 0;
    globalThis.fetch = vi.fn().mockImplementation(() => {
      call++;
      if (call === 1) {
        return Promise.resolve({
          status: 402,
          json: async () => pr({ amount: '1' }), // 1 atomic unit (smallest valid)
          body: null,
          statusText: 'Payment Required',
        });
      }
      return Promise.resolve({
        status: 200,
        json: async () => ({
          id: 'c1',
          object: 'chat.completion',
          created: 1,
          model: 'gpt-4',
          choices: [{ index: 0, message: { role: 'assistant', content: 'ok' }, finish_reason: 'stop' }],
          usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
        }),
        body: null,
        statusText: 'OK',
      });
    }) as unknown as typeof fetch;

    const signer = new StubSigner();
    const client = new SolvelaClient({ signer });
    await client.chat(new ChatRequest('gpt-4', [new ChatMessage('user', 'Hi')]));
    expect(signer.called).toBe(true);
  });

  it('rejects leading +/- sign and surrounding whitespace', async () => {
    // Regex must reject these even though Number(...) would accept some of
    // them — these inputs are not canonical wire format and may indicate a
    // malformed gateway response. (Leading zeros like "01" are intentionally
    // accepted: they parse unambiguously to the same integer.)
    const malformedInputs = ['+1', '-0', ' 100 ', '\t100', '100\n'];

    for (const amount of malformedInputs) {
      mockFetchOnce(402, pr({ amount }));
      const signer = new StubSigner();
      const client = new SolvelaClient({ signer });
      await expect(
        client.chat(new ChatRequest('gpt-4', [new ChatMessage('user', 'Hi')])),
      ).rejects.toThrow(SignerError);
      expect(signer.called).toBe(false);
    }
  });
});

describe('Security: network/asset validation before signing', () => {
  const originalFetch = globalThis.fetch;
  afterEach(() => {
    globalThis.fetch = originalFetch;
  });

  it('rejects unexpected network', async () => {
    mockFetchOnce(402, pr({ network: 'evm:1' }));

    const signer = new StubSigner();
    const client = new SolvelaClient({ signer });
    const request = new ChatRequest('gpt-4', [new ChatMessage('user', 'Hi')]);

    await expect(client.chat(request)).rejects.toThrow(ClientError);
    expect(signer.called).toBe(false);
  });

  it('rejects unexpected asset (wrong mint)', async () => {
    mockFetchOnce(402, pr({ asset: '11111111111111111111111111111111' }));

    const signer = new StubSigner();
    const client = new SolvelaClient({ signer });
    const request = new ChatRequest('gpt-4', [new ChatMessage('user', 'Hi')]);

    await expect(client.chat(request)).rejects.toThrow(ClientError);
    expect(signer.called).toBe(false);
  });

  it('accepts the correct USDC mint asset', async () => {
    // First fetch returns 402 with correct asset; second is the post-signature retry.
    let call = 0;
    globalThis.fetch = vi.fn().mockImplementation(() => {
      call++;
      if (call === 1) {
        return Promise.resolve({
          status: 402,
          json: async () => pr({ asset: USDC_MINT }),
          body: null,
          statusText: 'Payment Required',
        });
      }
      return Promise.resolve({
        status: 200,
        json: async () => ({
          id: 'c1',
          object: 'chat.completion',
          created: 1,
          model: 'gpt-4',
          choices: [
            {
              index: 0,
              message: { role: 'assistant', content: 'ok' },
              finish_reason: 'stop',
            },
          ],
          usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
        }),
        body: null,
        statusText: 'OK',
      });
    }) as unknown as typeof fetch;

    const signer = new StubSigner();
    const client = new SolvelaClient({ signer });
    const request = new ChatRequest('gpt-4', [new ChatMessage('user', 'Hi')]);
    const resp = await client.chat(request);
    expect(signer.called).toBe(true);
    expect(resp.choices[0].message.content).toBe('ok');
  });
});

describe('Security: HTTP gateway URL validation', () => {
  it('throws when constructing client with non-loopback http:// gateway', () => {
    expect(
      () => new SolvelaClient({ config: { gatewayUrl: 'http://evil.example.com' } }),
    ).toThrow();
  });

  it('allows http://localhost', () => {
    expect(
      () => new SolvelaClient({ config: { gatewayUrl: 'http://localhost:8402' } }),
    ).not.toThrow();
  });

  it('allows https:// for any host', () => {
    expect(
      () => new SolvelaClient({ config: { gatewayUrl: 'https://api.solvela.ai' } }),
    ).not.toThrow();
  });
});

describe('Security: PaymentRequired wire-format includes asset', () => {
  it('PaymentAccept.fromJSON preserves asset field', () => {
    const data = pr({ asset: USDC_MINT });
    const parsed = PaymentRequired.fromJSON(data);
    expect(parsed.accepts[0].asset).toBe(USDC_MINT);
  });

  it('PaymentAccept.toJSON omits escrow_program_id when undefined', () => {
    const accept = new PaymentAccept(
      'exact',
      SOLANA_NETWORK,
      '1000',
      USDC_MINT,
      '11111111111111111111111111111111',
      300,
    );
    const json = accept.toJSON();
    expect('escrow_program_id' in json).toBe(false);
  });
});

describe('Security: Scheme literal validation', () => {
  const validBase = {
    network: SOLANA_NETWORK,
    amount: '1000',
    asset: USDC_MINT,
    pay_to: '11111111111111111111111111111111',
    max_timeout_seconds: 300,
  };

  it('PaymentAccept.fromJSON rejects unknown scheme', () => {
    // A gateway-supplied unknown scheme is treated as a protocol upgrade the
    // SDK has not been taught yet. Failing here prevents downstream code from
    // silently mis-branching at scheme-matching call sites.
    expect(() =>
      PaymentAccept.fromJSON({ ...validBase, scheme: 'free-trial' }),
    ).toThrow(ClientError);
  });

  it('PaymentAccept.fromJSON rejects non-string scheme', () => {
    expect(() => PaymentAccept.fromJSON({ ...validBase, scheme: 42 })).toThrow(
      ClientError,
    );
    expect(() => PaymentAccept.fromJSON({ ...validBase, scheme: null })).toThrow(
      ClientError,
    );
  });

  it('PaymentAccept.fromJSON accepts both known schemes', () => {
    expect(() =>
      PaymentAccept.fromJSON({ ...validBase, scheme: 'exact' }),
    ).not.toThrow();
    expect(() =>
      PaymentAccept.fromJSON({ ...validBase, scheme: 'escrow' }),
    ).not.toThrow();
  });
});

describe('Security: escrow program pin before signing', () => {
  const originalFetch = globalThis.fetch;
  afterEach(() => {
    globalThis.fetch = originalFetch;
  });

  const ATTACKER = 'AttackerProgram1111111111111111111111111111';

  it('rejects an escrow 402 whose program differs from the default pin, before signing', async () => {
    mockFetchOnce(402, pr({ scheme: 'escrow', escrowProgramId: ATTACKER }));

    const signer = new StubSigner();
    // preferEscrow so findCompatibleScheme selects the escrow accept under test.
    const client = new SolvelaClient({ signer, config: { preferEscrow: true } });
    const request = new ChatRequest('gpt-4', [new ChatMessage('user', 'Hi')]);

    await expect(client.chat(request)).rejects.toThrow(EscrowProgramMismatchError);
    expect(signer.called).toBe(false);
  });

  it('fails closed when the escrow program is missing from the 402', async () => {
    // No escrow_program_id field at all -> treated as '' -> mismatch vs pin.
    mockFetchOnce(402, pr({ scheme: 'escrow' }));

    const signer = new StubSigner();
    const client = new SolvelaClient({ signer, config: { preferEscrow: true } });
    const request = new ChatRequest('gpt-4', [new ChatMessage('user', 'Hi')]);

    await expect(client.chat(request)).rejects.toThrow(EscrowProgramMismatchError);
    expect(signer.called).toBe(false);
  });

  it('proceeds to sign when the escrow program matches the default mainnet pin', async () => {
    mockFetchOnce(402, pr({ scheme: 'escrow', escrowProgramId: MAINNET_ESCROW_PROGRAM_ID }));

    const signer = new StubSigner();
    const client = new SolvelaClient({ signer, config: { preferEscrow: true } });
    const request = new ChatRequest('gpt-4', [new ChatMessage('user', 'Hi')]);

    // The stub signer returns a payload without real RPC; reaching it proves the
    // pin did NOT reject a legitimate program. The retried request then resolves
    // against the mocked fetch (still 402), but the signer must have been called.
    await client.chat(request).catch(() => {});
    expect(signer.called).toBe(true);
  });

  it('proceeds to sign for any program when the pin is disabled', async () => {
    mockFetchOnce(402, pr({ scheme: 'escrow', escrowProgramId: ATTACKER }));

    const signer = new StubSigner();
    const client = new SolvelaClient({
      signer,
      config: { preferEscrow: true, expectedEscrowProgram: undefined },
    });
    const request = new ChatRequest('gpt-4', [new ChatMessage('user', 'Hi')]);

    await client.chat(request).catch(() => {});
    expect(signer.called).toBe(true);
  });

  it('does not apply the escrow pin to an exact-scheme 402 (back-compat)', async () => {
    // An exact accept has no escrow program; the pin must not interfere.
    mockFetchOnce(402, pr({ scheme: 'exact' }));

    const signer = new StubSigner();
    const client = new SolvelaClient({ signer });
    const request = new ChatRequest('gpt-4', [new ChatMessage('user', 'Hi')]);

    await client.chat(request).catch(() => {});
    expect(signer.called).toBe(true);
  });
});
