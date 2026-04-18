/**
 * Unit-9: SolvelaWalletAdapter contract tests
 *
 * Scope (plan §6 Phase 7):
 *   - Custom adapter drives solvelaFetch 402→sign→retry without wrapper
 *     inspecting adapter internals beyond `signPayment` and `label`.
 *   - Private fields on a hand-rolled adapter never surface in logger events.
 *   - createLocalWalletAdapter exposes exactly two keys; JSON.stringify and
 *     util.inspect do not leak key bytes.
 *
 * Framework: vitest (ESM, node environment).
 */

import { inspect } from 'node:util';
import { describe, it, expect, vi, beforeEach } from 'vitest';

import { BudgetState } from '../../src/budget.js';
import {
  createSolvelaFetch,
  type SolvelaFetchLogEvent,
} from '../../src/fetch-wrapper.js';
import { createLocalWalletAdapter } from '../../src/adapters/local.js';
import { SolvelaInvalidConfigError } from '../../src/errors.js';
import type { SolvelaWalletAdapter } from '../../src/wallet-adapter.js';

// ---------------------------------------------------------------------------
// Shared fixtures
// ---------------------------------------------------------------------------

/**
 * Stub Keypair — dummy bytes, never used for real signing.
 * 64 bytes of 0x42; the byte sequence has no base58 wallet address meaning.
 */
const stubKeypair = {
  secretKey: new Uint8Array(64).fill(0x42),
};

/**
 * A minimal valid gateway 402 envelope that parseGateway402 accepts.
 * Matches the shape in tests/fixtures/402-envelope.json.
 */
const FIXTURE_402_BODY = JSON.stringify({
  error: {
    type: 'invalid_payment',
    message: JSON.stringify({
      x402_version: 2,
      resource: { url: '/v1/chat/completions', method: 'POST' },
      accepts: [
        {
          scheme: 'exact',
          network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
          amount: '2625',
          asset: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
          pay_to: 'RecipientWalletPubkeyHere',
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
    }),
  },
});

/** Build a mock baseFetch that returns a 402 then a 200 on consecutive calls. */
function make402Then200Fetch(responseBody = '{}'): typeof globalThis.fetch {
  let callCount = 0;
  return vi.fn().mockImplementation(async () => {
    callCount += 1;
    if (callCount === 1) {
      return new Response(FIXTURE_402_BODY, {
        status: 402,
        headers: { 'content-type': 'application/json' },
      });
    }
    return new Response(responseBody, {
      status: 200,
      headers: { 'content-type': 'application/json' },
    });
  }) as unknown as typeof globalThis.fetch;
}

// ---------------------------------------------------------------------------
// Group 1: Minimal custom adapter — wrapper only touches signPayment + label
// ---------------------------------------------------------------------------

describe('Group 1: minimal custom adapter drives 402→sign→retry', () => {
  it('invokes signPayment on a minimal adapter and completes the 402→200 cycle', async () => {
    const customAdapter: SolvelaWalletAdapter = {
      label: 'custom',
      signPayment: vi.fn().mockResolvedValue('sig=='),
    };

    const baseFetch = make402Then200Fetch();
    const fetch = createSolvelaFetch({
      wallet: customAdapter,
      budget: new BudgetState(undefined),
      maxSignedBodyBytes: 1024 * 1024,
      baseFetch,
    });

    const resp = await fetch('https://api.example.com/v1/chat/completions', {
      method: 'POST',
      body: '{"model":"gpt-4o","messages":[]}',
      headers: { 'content-type': 'application/json' },
    });

    expect(resp.status).toBe(200);
    expect(customAdapter.signPayment).toHaveBeenCalledOnce();
  });

  it('passes paymentRequired as the parsed 402 payload to signPayment', async () => {
    const customAdapter: SolvelaWalletAdapter = {
      label: 'custom',
      signPayment: vi.fn().mockResolvedValue('sig=='),
    };

    const baseFetch = make402Then200Fetch();
    const fetch = createSolvelaFetch({
      wallet: customAdapter,
      budget: new BudgetState(undefined),
      maxSignedBodyBytes: 1024 * 1024,
      baseFetch,
    });

    await fetch('https://api.example.com/v1/chat/completions', {
      method: 'POST',
      body: '{"model":"gpt-4o","messages":[]}',
      headers: { 'content-type': 'application/json' },
    });

    const callArgs = vi.mocked(customAdapter.signPayment).mock.calls[0][0];
    expect(callArgs.paymentRequired).toMatchObject({
      x402_version: 2,
      accepts: expect.arrayContaining([
        expect.objectContaining({ scheme: 'exact', amount: '2625' }),
      ]),
    });
  });

  it('passes the request URL as resourceUrl to signPayment', async () => {
    const targetUrl = 'https://api.example.com/v1/chat/completions';
    const customAdapter: SolvelaWalletAdapter = {
      label: 'custom',
      signPayment: vi.fn().mockResolvedValue('sig=='),
    };

    const baseFetch = make402Then200Fetch();
    const fetch = createSolvelaFetch({
      wallet: customAdapter,
      budget: new BudgetState(undefined),
      maxSignedBodyBytes: 1024 * 1024,
      baseFetch,
    });

    await fetch(targetUrl, {
      method: 'POST',
      body: '{"model":"gpt-4o","messages":[]}',
      headers: { 'content-type': 'application/json' },
    });

    const callArgs = vi.mocked(customAdapter.signPayment).mock.calls[0][0];
    expect(callArgs.resourceUrl).toBe(targetUrl);
  });

  it('passes the original request body string as requestBody to signPayment', async () => {
    const requestBody = '{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}]}';
    const customAdapter: SolvelaWalletAdapter = {
      label: 'custom',
      signPayment: vi.fn().mockResolvedValue('sig=='),
    };

    const baseFetch = make402Then200Fetch();
    const fetch = createSolvelaFetch({
      wallet: customAdapter,
      budget: new BudgetState(undefined),
      maxSignedBodyBytes: 1024 * 1024,
      baseFetch,
    });

    await fetch('https://api.example.com/v1/chat/completions', {
      method: 'POST',
      body: requestBody,
      headers: { 'content-type': 'application/json' },
    });

    const callArgs = vi.mocked(customAdapter.signPayment).mock.calls[0][0];
    expect(callArgs.requestBody).toBe(requestBody);
  });

  it('passes signal from init to signPayment', async () => {
    const controller = new AbortController();
    const customAdapter: SolvelaWalletAdapter = {
      label: 'custom',
      signPayment: vi.fn().mockResolvedValue('sig=='),
    };

    const baseFetch = make402Then200Fetch();
    const fetch = createSolvelaFetch({
      wallet: customAdapter,
      budget: new BudgetState(undefined),
      maxSignedBodyBytes: 1024 * 1024,
      baseFetch,
    });

    await fetch('https://api.example.com/v1/chat/completions', {
      method: 'POST',
      body: '{"model":"gpt-4o","messages":[]}',
      headers: { 'content-type': 'application/json' },
      signal: controller.signal,
    });

    const callArgs = vi.mocked(customAdapter.signPayment).mock.calls[0][0];
    expect(callArgs.signal).toBe(controller.signal);
  });

  it('does not access any property on the adapter other than signPayment and label', async () => {
    // Use a Proxy to record every property access on the adapter object.
    const accessedProperties = new Set<string | symbol>();
    const bare: SolvelaWalletAdapter = {
      label: 'custom',
      signPayment: vi.fn().mockResolvedValue('sig=='),
    };
    const proxied = new Proxy(bare, {
      get(target, prop, receiver) {
        if (typeof prop === 'string' || typeof prop === 'symbol') {
          accessedProperties.add(prop);
        }
        return Reflect.get(target, prop, receiver);
      },
    });

    const baseFetch = make402Then200Fetch();
    const fetch = createSolvelaFetch({
      wallet: proxied,
      budget: new BudgetState(undefined),
      maxSignedBodyBytes: 1024 * 1024,
      baseFetch,
    });

    await fetch('https://api.example.com/v1/chat/completions', {
      method: 'POST',
      body: '{"model":"gpt-4o","messages":[]}',
      headers: { 'content-type': 'application/json' },
    });

    // Filter out well-known runtime property reads (Symbol.toPrimitive, etc.)
    const accessedStrings = [...accessedProperties].filter(
      (p) => typeof p === 'string',
    ) as string[];

    // Wrapper is only permitted to access 'signPayment' and 'label'.
    const unauthorizedAccess = accessedStrings.filter(
      (p) => p !== 'signPayment' && p !== 'label',
    );
    expect(unauthorizedAccess).toHaveLength(0);
  });
});

// ---------------------------------------------------------------------------
// Group 2: Private-field adapter — no private field names in logger events
// ---------------------------------------------------------------------------

describe('Group 2: private fields on hand-rolled adapter never appear in logger events', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  it('logs no event whose JSON representation references private field names', async () => {
    /**
     * Adapter with extra private-ish fields. The wrapper must never log these.
     */
    const adapterWithPrivateFields = {
      label: 'hand-rolled',
      _secretKey: 'VERY_SECRET_KEY_MATERIAL',
      _mnemonic: 'abandon ability able about above absent absorb abstract absurd abuse',
      signPayment: vi.fn().mockResolvedValue('sig=='),
    } as unknown as SolvelaWalletAdapter;

    const logEvents: SolvelaFetchLogEvent[] = [];
    const logger = (event: SolvelaFetchLogEvent): void => {
      logEvents.push(event);
    };

    const baseFetch = make402Then200Fetch();
    const fetch = createSolvelaFetch({
      wallet: adapterWithPrivateFields,
      budget: new BudgetState(undefined),
      maxSignedBodyBytes: 1024 * 1024,
      logger,
      baseFetch,
    });

    await fetch('https://api.example.com/v1/chat/completions', {
      method: 'POST',
      body: '{"model":"gpt-4o","messages":[]}',
      headers: { 'content-type': 'application/json' },
    });

    const serialized = JSON.stringify(logEvents);

    expect(serialized).not.toContain('_secretKey');
    expect(serialized).not.toContain('_mnemonic');
    expect(serialized).not.toContain('VERY_SECRET_KEY_MATERIAL');
    expect(serialized).not.toContain('abandon ability able');
  });

  it('logs only the documented SolvelaFetchLogEvent fields (event, attempt, requestId, status)', async () => {
    const customAdapter: SolvelaWalletAdapter = {
      label: 'custom',
      signPayment: vi.fn().mockResolvedValue('sig=='),
    };

    const logEvents: SolvelaFetchLogEvent[] = [];
    const baseFetch = make402Then200Fetch();
    const fetch = createSolvelaFetch({
      wallet: customAdapter,
      budget: new BudgetState(undefined),
      maxSignedBodyBytes: 1024 * 1024,
      logger: (e) => logEvents.push(e),
      baseFetch,
    });

    await fetch('https://api.example.com/v1/chat/completions', {
      method: 'POST',
      body: '{"model":"gpt-4o","messages":[]}',
      headers: { 'content-type': 'application/json' },
    });

    // Every event must contain only the declared fields.
    const allowedFields = new Set(['event', 'attempt', 'requestId', 'status']);
    for (const ev of logEvents) {
      const extraKeys = Object.keys(ev).filter((k) => !allowedFields.has(k));
      expect(extraKeys).toHaveLength(0);
    }
  });

  it('logger events on the 402 path do not contain the adapter label value', async () => {
    // Extra guard: the adapter's label string itself must not appear in log events,
    // since logs should only describe the fetch lifecycle, not the adapter identity.
    const uniqueLabel = 'UNIQUE_ADAPTER_LABEL_XYZ';
    const customAdapter: SolvelaWalletAdapter = {
      label: uniqueLabel,
      signPayment: vi.fn().mockResolvedValue('sig=='),
    };

    const logEvents: SolvelaFetchLogEvent[] = [];
    const baseFetch = make402Then200Fetch();
    const fetch = createSolvelaFetch({
      wallet: customAdapter,
      budget: new BudgetState(undefined),
      maxSignedBodyBytes: 1024 * 1024,
      logger: (e) => logEvents.push(e),
      baseFetch,
    });

    await fetch('https://api.example.com/v1/chat/completions', {
      method: 'POST',
      body: '{"model":"gpt-4o","messages":[]}',
      headers: { 'content-type': 'application/json' },
    });

    const serialized = JSON.stringify(logEvents);
    expect(serialized).not.toContain(uniqueLabel);
  });
});

// ---------------------------------------------------------------------------
// Group 3: createLocalWalletAdapter — key surface area
// ---------------------------------------------------------------------------

describe('Group 3: createLocalWalletAdapter exposes no key bytes', () => {
  it('returns an object with exactly the keys [label, signPayment]', () => {
    const adapter = createLocalWalletAdapter(stubKeypair);
    expect(Object.keys(adapter).sort()).toEqual(['label', 'signPayment'].sort());
  });

  it('JSON.stringify does not contain any base58 substring (44-88 chars base58 alphabet)', () => {
    const adapter = createLocalWalletAdapter(stubKeypair);
    const serialized = JSON.stringify(adapter);

    // Base58 alphabet: 1-9 A-H J-N P-Z a-k m-z (same regex as redact.ts)
    const base58Re = /[1-9A-HJ-NP-Za-km-z]{44,88}/;
    expect(base58Re.test(serialized)).toBe(false);
  });

  it('label is the documented constant string', () => {
    const adapter = createLocalWalletAdapter(stubKeypair);
    expect(adapter.label).toBe('local-test-keypair');
  });

  it('signPayment is a function', () => {
    const adapter = createLocalWalletAdapter(stubKeypair);
    expect(typeof adapter.signPayment).toBe('function');
  });
});

// ---------------------------------------------------------------------------
// Group 4: util.inspect does not leak key bytes
// ---------------------------------------------------------------------------

describe('Group 4: util.inspect at depth 5 leaks no key bytes', () => {
  it('does not contain any base58 substring', () => {
    const adapter = createLocalWalletAdapter(stubKeypair);
    const inspected = inspect(adapter, { depth: 5 });

    // Base58 alphabet regex — same as the one in redact.ts
    const base58Re = /[1-9A-HJ-NP-Za-km-z]{44,88}/;
    expect(base58Re.test(inspected)).toBe(false);
  });

  it('does not contain the stub secretKey bytes as a hex string', () => {
    const adapter = createLocalWalletAdapter(stubKeypair);
    const inspected = inspect(adapter, { depth: 5 });

    // 0x42 repeated 64 times as a hex string would be "4242...42" (128 chars)
    const hexPattern = '42'.repeat(32); // first 32 bytes as a run of "42"
    expect(inspected).not.toContain(hexPattern);
  });

  it('does not expose the Uint8Array secretKey through the closure', () => {
    // The adapter returned by createLocalWalletAdapter has no own property
    // that directly references the keypair; the keypair is captured in a closure.
    const adapter = createLocalWalletAdapter(stubKeypair);

    // No own enumerable or non-enumerable property should be the secretKey.
    const allKeys = [
      ...Object.getOwnPropertyNames(adapter),
      ...Object.getOwnPropertySymbols(adapter).map(String),
    ];

    for (const key of allKeys) {
      if (key === 'signPayment' || key === 'label') continue;
      const value = (adapter as Record<string, unknown>)[key];
      expect(value instanceof Uint8Array).toBe(false);
    }
  });
});

// ---------------------------------------------------------------------------
// Group 5: createLocalWalletAdapter — invalid input guard
// ---------------------------------------------------------------------------

describe('Group 5: createLocalWalletAdapter rejects invalid keypair shapes', () => {
  it('throws SolvelaInvalidConfigError when keypair is null', () => {
    expect(() => createLocalWalletAdapter(null as unknown as { secretKey: Uint8Array })).toThrow(
      SolvelaInvalidConfigError,
    );
  });

  it('throws SolvelaInvalidConfigError when secretKey is not a Uint8Array', () => {
    expect(() =>
      createLocalWalletAdapter({ secretKey: 'not-a-uint8array' } as unknown as {
        secretKey: Uint8Array;
      }),
    ).toThrow(SolvelaInvalidConfigError);
  });

  it('throws SolvelaInvalidConfigError when secretKey is a plain Array, not Uint8Array', () => {
    expect(() =>
      createLocalWalletAdapter({
        secretKey: Array.from({ length: 64 }, () => 0x42) as unknown as Uint8Array,
      }),
    ).toThrow(SolvelaInvalidConfigError);
  });
});
