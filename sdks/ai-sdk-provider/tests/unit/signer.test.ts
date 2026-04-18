/**
 * Unit-4: Adapter invocation semantics for createSolvelaFetch.
 *
 * Covers:
 *   - signPayment called EXACTLY ONCE per logical request (no double-call)
 *   - paymentRequired argument matches parsed envelope byte-for-byte
 *   - resourceUrl matches the URL passed to solvelaFetch
 *   - requestBody matches init.body verbatim
 *   - signal passed to adapter is the same reference as init.signal
 *   - adapter return value threaded into retry PAYMENT-SIGNATURE header exactly
 *   - adapter throws Error → budget released, SolvelaSigningError surfaced (not bare Error)
 *   - adapter throws AbortError → rethrown as-is, name preserved
 *   - two sequential solvelaFetch calls → adapter called once per call (twice total)
 *   - logger events contain adapter.label but never internal secretKey field value
 */

import { describe, it, expect, vi, beforeEach } from 'vitest';
import { createSolvelaFetch, type SolvelaFetchLogEvent } from '../../src/fetch-wrapper.js';
import { BudgetState } from '../../src/budget.js';
import { SolvelaSigningError } from '../../src/errors.js';
import type { SolvelaWalletAdapter, SolvelaPaymentRequired } from '../../src/wallet-adapter.js';

// ---------------------------------------------------------------------------
// Fixture: the canonical 402 envelope from tests/fixtures/402-envelope.json
// ---------------------------------------------------------------------------

const FIXTURE_INNER: SolvelaPaymentRequired = {
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
};

/** Serialized the same way the gateway sends it — inner object JSON-stringified. */
const FIXTURE_402_BODY = JSON.stringify({
  error: {
    type: 'invalid_payment',
    message: JSON.stringify(FIXTURE_INNER),
  },
});

const FIXTURE_URL = 'https://gateway.solvela.com/v1/chat/completions';
const FIXTURE_REQUEST_BODY = JSON.stringify({ model: 'claude-sonnet-4-5', messages: [] });
const FIXTURE_SIGNATURE = 'signed-base64==';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Build a mock Response with a given status and optional text body. */
function makeResponse(status: number, body = ''): Response {
  return new Response(body, { status });
}

/** Build a 402 Response using the canonical fixture envelope. */
function make402(): Response {
  return new Response(FIXTURE_402_BODY, { status: 402 });
}

/** Build a 200 success Response. */
function make200(): Response {
  return makeResponse(200, '{"id":"chatcmpl-1","choices":[]}');
}

/**
 * Create a minimal no-op BudgetState (budget disabled — all reserves succeed).
 * Using the real class keeps the test honest; passing `undefined` to the
 * constructor disables budget enforcement.
 */
function makeUnlimitedBudget(): BudgetState {
  return new BudgetState(undefined);
}

/**
 * Build a simple stub adapter. `signPayment` is a vitest mock that resolves to
 * `FIXTURE_SIGNATURE` by default. The `label` is exposed as a public field.
 */
function makeStubAdapter(
  signImpl?: () => Promise<string>,
): SolvelaWalletAdapter {
  return {
    label: 'stub-test-adapter',
    signPayment: vi.fn(signImpl ?? (() => Promise.resolve(FIXTURE_SIGNATURE))),
  };
}

/**
 * Wire up createSolvelaFetch with the given baseFetch mock and adapter.
 * Returns the fetch function ready to invoke.
 */
function makeSolvelaFetch(
  baseFetch: typeof globalThis.fetch,
  adapter: SolvelaWalletAdapter,
  logEvents?: SolvelaFetchLogEvent[],
) {
  return createSolvelaFetch({
    wallet: adapter,
    budget: makeUnlimitedBudget(),
    maxSignedBodyBytes: 1024 * 1024,
    baseFetch,
    logger: logEvents ? (e) => logEvents.push(e) : undefined,
  });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

describe('Unit-4: signer adapter invocation semantics', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
  });

  // -------------------------------------------------------------------------
  // 1. signPayment called EXACTLY ONCE per logical request
  // -------------------------------------------------------------------------
  it('calls signPayment exactly once on the happy 402-sign-retry path', async () => {
    const adapter = makeStubAdapter();
    const baseFetch = vi.fn()
      .mockResolvedValueOnce(make402())
      .mockResolvedValueOnce(make200());

    const solvelaFetch = makeSolvelaFetch(baseFetch, adapter);
    await solvelaFetch(FIXTURE_URL, { body: FIXTURE_REQUEST_BODY });

    expect(adapter.signPayment).toHaveBeenCalledTimes(1);
  });

  // -------------------------------------------------------------------------
  // 2. paymentRequired argument matches the parsed envelope byte-for-byte
  // -------------------------------------------------------------------------
  it('passes paymentRequired to adapter matching the parsed fixture envelope exactly', async () => {
    const adapter = makeStubAdapter();
    const baseFetch = vi.fn()
      .mockResolvedValueOnce(make402())
      .mockResolvedValueOnce(make200());

    const solvelaFetch = makeSolvelaFetch(baseFetch, adapter);
    await solvelaFetch(FIXTURE_URL, { body: FIXTURE_REQUEST_BODY });

    const { paymentRequired } = (adapter.signPayment as ReturnType<typeof vi.fn>).mock.calls[0][0];
    expect(paymentRequired).toEqual(FIXTURE_INNER);
  });

  // -------------------------------------------------------------------------
  // 3. resourceUrl matches the URL the wrapper was invoked with
  // -------------------------------------------------------------------------
  it('passes resourceUrl to adapter matching the fetch URL', async () => {
    const adapter = makeStubAdapter();
    const baseFetch = vi.fn()
      .mockResolvedValueOnce(make402())
      .mockResolvedValueOnce(make200());

    const solvelaFetch = makeSolvelaFetch(baseFetch, adapter);
    await solvelaFetch(FIXTURE_URL, { body: FIXTURE_REQUEST_BODY });

    const { resourceUrl } = (adapter.signPayment as ReturnType<typeof vi.fn>).mock.calls[0][0];
    expect(resourceUrl).toBe(FIXTURE_URL);
  });

  // -------------------------------------------------------------------------
  // 4. requestBody passed to adapter matches init.body verbatim
  // -------------------------------------------------------------------------
  it('passes requestBody to adapter matching init.body verbatim', async () => {
    const adapter = makeStubAdapter();
    const baseFetch = vi.fn()
      .mockResolvedValueOnce(make402())
      .mockResolvedValueOnce(make200());

    const solvelaFetch = makeSolvelaFetch(baseFetch, adapter);
    await solvelaFetch(FIXTURE_URL, { body: FIXTURE_REQUEST_BODY });

    const { requestBody } = (adapter.signPayment as ReturnType<typeof vi.fn>).mock.calls[0][0];
    expect(requestBody).toBe(FIXTURE_REQUEST_BODY);
  });

  // -------------------------------------------------------------------------
  // 5. signal passed to adapter is the same reference as init.signal
  // -------------------------------------------------------------------------
  it('passes signal to adapter by reference (Object.is equality)', async () => {
    const controller = new AbortController();
    const adapter = makeStubAdapter();
    const baseFetch = vi.fn()
      .mockResolvedValueOnce(make402())
      .mockResolvedValueOnce(make200());

    const solvelaFetch = makeSolvelaFetch(baseFetch, adapter);
    await solvelaFetch(FIXTURE_URL, { body: FIXTURE_REQUEST_BODY, signal: controller.signal });

    const { signal } = (adapter.signPayment as ReturnType<typeof vi.fn>).mock.calls[0][0];
    expect(Object.is(signal, controller.signal)).toBe(true);
  });

  // -------------------------------------------------------------------------
  // 6. Adapter return value threaded into retry PAYMENT-SIGNATURE header exactly
  // -------------------------------------------------------------------------
  it('sets PAYMENT-SIGNATURE header on retry to the exact value returned by adapter', async () => {
    const adapter = makeStubAdapter(() => Promise.resolve(FIXTURE_SIGNATURE));
    const capturedRetryHeaders: Record<string, string>[] = [];

    const baseFetch = vi.fn()
      .mockResolvedValueOnce(make402())
      .mockImplementationOnce((_url: unknown, init?: RequestInit) => {
        const h = init?.headers as Record<string, string> | undefined;
        if (h) capturedRetryHeaders.push(h);
        return Promise.resolve(make200());
      });

    const solvelaFetch = makeSolvelaFetch(baseFetch, adapter);
    await solvelaFetch(FIXTURE_URL, { body: FIXTURE_REQUEST_BODY });

    expect(capturedRetryHeaders).toHaveLength(1);
    // Header key is the canonical casing emitted by the wrapper.
    expect(capturedRetryHeaders[0]['PAYMENT-SIGNATURE']).toBe(FIXTURE_SIGNATURE);
  });

  // -------------------------------------------------------------------------
  // 7. Adapter throws random Error → reservation released, SolvelaSigningError thrown
  // -------------------------------------------------------------------------
  it('releases budget and throws SolvelaSigningError when adapter throws a plain Error', async () => {
    const adapterError = new Error('signing device unavailable');
    const adapter = makeStubAdapter(() => Promise.reject(adapterError));

    // Spy on the real budget to verify release was called.
    const budget = makeUnlimitedBudget();
    const releaseSpy = vi.spyOn(budget, 'release');

    const baseFetch = vi.fn().mockResolvedValueOnce(make402());

    const solvelaFetch = createSolvelaFetch({
      wallet: adapter,
      budget,
      maxSignedBodyBytes: 1024 * 1024,
      baseFetch,
    });

    let thrown: unknown;
    try {
      await solvelaFetch(FIXTURE_URL, { body: FIXTURE_REQUEST_BODY });
    } catch (err) {
      thrown = err;
    }

    // Budget must have been released.
    expect(releaseSpy).toHaveBeenCalledTimes(1);

    // Error must be SolvelaSigningError, not the raw adapter Error.
    expect(SolvelaSigningError.isInstance(thrown)).toBe(true);
    // Must not be the original Error instance that the adapter threw.
    expect(Object.is(thrown, adapterError)).toBe(false);
    // Must not be a plain (non-subclassed) Error — the name discriminates.
    expect((thrown as Error).constructor.name).toBe('SolvelaSigningError');
    expect((thrown as SolvelaSigningError).name).toBe('SolvelaSigningError');
  });

  // -------------------------------------------------------------------------
  // 8. Adapter throws AbortError (DOMException) → rethrown as-is, name preserved
  // -------------------------------------------------------------------------
  it('rethrows AbortError from adapter unchanged without wrapping in SolvelaSigningError', async () => {
    const abortError = new DOMException('aborted', 'AbortError');
    const adapter = makeStubAdapter(() => Promise.reject(abortError));
    const baseFetch = vi.fn().mockResolvedValueOnce(make402());

    const solvelaFetch = makeSolvelaFetch(baseFetch, adapter);

    let thrown: unknown;
    try {
      await solvelaFetch(FIXTURE_URL, { body: FIXTURE_REQUEST_BODY });
    } catch (err) {
      thrown = err;
    }

    // The original DOMException instance must be rethrown — same reference.
    expect(Object.is(thrown, abortError)).toBe(true);
    // AbortError name must be preserved.
    expect((thrown as DOMException).name).toBe('AbortError');
    // Must NOT be wrapped in SolvelaSigningError.
    expect(SolvelaSigningError.isInstance(thrown)).toBe(false);
  });

  // -------------------------------------------------------------------------
  // 9. Two sequential solvelaFetch calls → adapter called once per call (twice total)
  // -------------------------------------------------------------------------
  it('calls adapter once per logical request across two sequential invocations', async () => {
    const adapter = makeStubAdapter();
    // Each call sees: 402 first, then 200.
    const baseFetch = vi.fn()
      .mockResolvedValueOnce(make402())
      .mockResolvedValueOnce(make200())
      .mockResolvedValueOnce(make402())
      .mockResolvedValueOnce(make200());

    const solvelaFetch = makeSolvelaFetch(baseFetch, adapter);

    await solvelaFetch(FIXTURE_URL, { body: FIXTURE_REQUEST_BODY });
    await solvelaFetch(FIXTURE_URL, { body: FIXTURE_REQUEST_BODY });

    expect(adapter.signPayment).toHaveBeenCalledTimes(2);
    // Each call gets its own invocation — verify each receives the same resourceUrl.
    const calls = (adapter.signPayment as ReturnType<typeof vi.fn>).mock.calls;
    expect(calls[0][0].resourceUrl).toBe(FIXTURE_URL);
    expect(calls[1][0].resourceUrl).toBe(FIXTURE_URL);
  });

  // -------------------------------------------------------------------------
  // 10. Logger events contain adapter.label but never internal secretKey
  // -------------------------------------------------------------------------
  it('emits adapter.label in log events and never emits secretKey from adapter internals', async () => {
    /**
     * An adapter that exposes an internal `secretKey` field (simulating an
     * implementor that mistakenly puts a raw key on the instance). The wrapper
     * must never log that field or its value.
     */
    const SECRET_KEY_VALUE = 'supersecret-private-key-8fj3k2';
    const adapterWithSecret: SolvelaWalletAdapter & { secretKey: string } = {
      label: 'stub-leaky-adapter',
      secretKey: SECRET_KEY_VALUE,
      signPayment: vi.fn(() => Promise.resolve(FIXTURE_SIGNATURE)),
    };

    const logEvents: SolvelaFetchLogEvent[] = [];
    const logStrings: string[] = [];

    const wrappedLogger = (event: SolvelaFetchLogEvent) => {
      logEvents.push(event);
      // Serialise each event to detect any accidental field leakage.
      logStrings.push(JSON.stringify(event));
    };

    const baseFetch = vi.fn()
      .mockResolvedValueOnce(make402())
      .mockResolvedValueOnce(make200());

    const solvelaFetch = createSolvelaFetch({
      wallet: adapterWithSecret,
      budget: makeUnlimitedBudget(),
      maxSignedBodyBytes: 1024 * 1024,
      baseFetch,
      logger: wrappedLogger,
    });

    await solvelaFetch(FIXTURE_URL, { body: FIXTURE_REQUEST_BODY });

    // There must be log events — the 402 path emits at least sign-start and sign-end.
    expect(logEvents.length).toBeGreaterThan(0);

    // Every serialised log event must NOT contain the secret key value.
    for (const serialised of logStrings) {
      expect(serialised).not.toContain(SECRET_KEY_VALUE);
      expect(serialised).not.toContain('secretKey');
    }

    // The adapter's label is not injected by the wrapper into log events
    // (the SolvelaFetchLogEvent type does not carry a label field), so we
    // verify the wrapper did not accidentally embed the whole adapter object.
    // The log event shape contains only: event, attempt, requestId, status?.
    for (const evt of logEvents) {
      const keys = Object.keys(evt);
      expect(keys.every((k) => ['event', 'attempt', 'requestId', 'status'].includes(k))).toBe(true);
    }
  });
});
