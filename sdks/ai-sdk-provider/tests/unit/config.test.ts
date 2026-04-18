/**
 * Unit-1: config.ts — validateSettings()
 *
 * Coverage: zod validation, env precedence, TLS rejection matrix,
 * baseURL /v1 normalization, missing/invalid wallet rejection,
 * PAYMENT-SIGNATURE header stripping, sessionBudget + maxBodyBytes env vars.
 *
 * IMPORTANT: each test does vi.resetModules() + dynamic import so the
 * module-scoped `emitted` Set in warn-once.ts is reset between tests.
 */

import { afterAll, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';

// ---------------------------------------------------------------------------
// Types — imported once for type-checking only; instances are dynamic-imported
// ---------------------------------------------------------------------------

type ValidateSettings = typeof import('../../src/config.js').validateSettings;
type SolvelaInvalidConfigErrorClass =
  typeof import('../../src/errors.js').SolvelaInvalidConfigError;

// ---------------------------------------------------------------------------
// Minimal valid wallet fixture
// ---------------------------------------------------------------------------

const VALID_WALLET = {
  label: 'test-wallet',
  signPayment: async () => 'sig',
};

// ---------------------------------------------------------------------------
// Env snapshot — preserve outer process env across the entire file
// ---------------------------------------------------------------------------

const ENV_KEYS = [
  'NODE_ENV',
  'VERCEL_ENV',
  'SOLVELA_API_URL',
  'SOLVELA_SESSION_BUDGET',
  'SOLVELA_MAX_SIGNED_BODY_BYTES',
  'SOLVELA_ALLOW_INSECURE_BASE_URL',
  'SOLVELA_AI_SDK_PROVIDER_TEST_MODE',
] as const;

let savedEnv: Partial<Record<string, string>> = {};

beforeAll(() => {
  for (const k of ENV_KEYS) {
    savedEnv[k] = process.env[k];
  }
});

afterAll(() => {
  for (const k of ENV_KEYS) {
    const v = savedEnv[k];
    if (v === undefined) {
      delete process.env[k];
    } else {
      process.env[k] = v;
    }
  }
  delete (globalThis as Record<string, unknown>).EdgeRuntime;
});

// ---------------------------------------------------------------------------
// beforeEach: reset env to a clean "development" baseline and re-import SUT
// ---------------------------------------------------------------------------

let validateSettings: ValidateSettings;
let SolvelaInvalidConfigError: SolvelaInvalidConfigErrorClass;

beforeEach(async () => {
  // Reset all env vars to a known baseline
  process.env.NODE_ENV = 'development';
  delete process.env.VERCEL_ENV;
  delete process.env.SOLVELA_API_URL;
  delete process.env.SOLVELA_SESSION_BUDGET;
  delete process.env.SOLVELA_MAX_SIGNED_BODY_BYTES;
  delete process.env.SOLVELA_ALLOW_INSECURE_BASE_URL;
  delete process.env.SOLVELA_AI_SDK_PROVIDER_TEST_MODE;
  delete (globalThis as Record<string, unknown>).EdgeRuntime;

  // Reset module registry so warn-once Set is fresh for every test
  vi.resetModules();
  vi.restoreAllMocks();

  ({ validateSettings } = await import('../../src/config.js'));
  ({ SolvelaInvalidConfigError } = await import('../../src/errors.js'));
});

// ===========================================================================
// 1. baseURL normalization and precedence
// ===========================================================================

describe('baseURL normalization and precedence', () => {
  it('uses default https://api.solvela.ai/v1 when no baseURL or env var is set', () => {
    const result = validateSettings({ wallet: VALID_WALLET });
    expect(result.baseURL).toBe('https://api.solvela.ai/v1');
  });

  it('prefers explicit baseURL over SOLVELA_API_URL env var', () => {
    process.env.SOLVELA_API_URL = 'https://env.example.com';
    const result = validateSettings({ wallet: VALID_WALLET, baseURL: 'https://explicit.example.com' });
    expect(result.baseURL).toBe('https://explicit.example.com/v1');
  });

  it('uses SOLVELA_API_URL env var when no explicit baseURL is set', () => {
    process.env.SOLVELA_API_URL = 'https://env.example.com';
    const result = validateSettings({ wallet: VALID_WALLET });
    expect(result.baseURL).toBe('https://env.example.com/v1');
  });

  it('appends /v1 when baseURL has no path suffix', () => {
    const result = validateSettings({ wallet: VALID_WALLET, baseURL: 'https://api.example.com' });
    expect(result.baseURL).toBe('https://api.example.com/v1');
  });

  it('strips trailing slash then appends /v1', () => {
    const result = validateSettings({ wallet: VALID_WALLET, baseURL: 'https://api.example.com/' });
    expect(result.baseURL).toBe('https://api.example.com/v1');
  });

  it('does not double-append /v1 when already present', () => {
    const result = validateSettings({ wallet: VALID_WALLET, baseURL: 'https://api.example.com/v1' });
    expect(result.baseURL).toBe('https://api.example.com/v1');
  });

  it('strips trailing slash from /v1/ without double-appending', () => {
    const result = validateSettings({ wallet: VALID_WALLET, baseURL: 'https://api.example.com/v1/' });
    expect(result.baseURL).toBe('https://api.example.com/v1');
  });
});

// ===========================================================================
// 2. TLS rejection matrix
// ===========================================================================

describe('TLS rejection matrix', () => {
  // (a) dev, no allowInsecureBaseURL flag → throws
  it('throws when non-HTTPS baseURL used in dev without allowInsecureBaseURL', () => {
    expect(() =>
      validateSettings({ wallet: VALID_WALLET, baseURL: 'http://api.example.com' }),
    ).toThrow(SolvelaInvalidConfigError);
  });

  it('does not emit console.error when rejecting non-HTTPS in plain dev mode', () => {
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    try {
      validateSettings({ wallet: VALID_WALLET, baseURL: 'http://api.example.com' });
    } catch {
      // expected
    }
    expect(errorSpy).not.toHaveBeenCalled();
  });

  // (b) dev with allowInsecureBaseURL: true → accepts
  it('accepts non-HTTPS baseURL in dev when allowInsecureBaseURL is true', () => {
    const result = validateSettings({
      wallet: VALID_WALLET,
      baseURL: 'http://localhost:8080',
      allowInsecureBaseURL: true,
    });
    expect(result.baseURL).toBe('http://localhost:8080/v1');
  });

  // (c) prod NODE_ENV + allowInsecureBaseURL: true → console.error + throws
  it('emits console.error and throws in production when allowInsecureBaseURL is true', () => {
    process.env.NODE_ENV = 'production';
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => undefined);

    expect(() =>
      validateSettings({
        wallet: VALID_WALLET,
        baseURL: 'http://api.example.com',
        allowInsecureBaseURL: true,
      }),
    ).toThrow(SolvelaInvalidConfigError);

    expect(errorSpy).toHaveBeenCalledOnce();
    expect(errorSpy.mock.calls[0]![0]).toContain('production or Vercel Edge');
  });

  // (c variant) prod VERCEL_ENV + allowInsecureBaseURL: true → console.error + throws
  it('emits console.error and throws when VERCEL_ENV=production and allowInsecureBaseURL is true', () => {
    process.env.VERCEL_ENV = 'production';
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => undefined);

    expect(() =>
      validateSettings({
        wallet: VALID_WALLET,
        baseURL: 'http://api.example.com',
        allowInsecureBaseURL: true,
      }),
    ).toThrow(SolvelaInvalidConfigError);

    expect(errorSpy).toHaveBeenCalledOnce();
  });

  // (d) Edge runtime + allowInsecureBaseURL: true → console.error + throws
  it('emits console.error and throws on Edge runtime when allowInsecureBaseURL is true', () => {
    (globalThis as Record<string, unknown>).EdgeRuntime = 'edge-runtime';
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => undefined);

    expect(() =>
      validateSettings({
        wallet: VALID_WALLET,
        baseURL: 'http://api.example.com',
        allowInsecureBaseURL: true,
      }),
    ).toThrow(SolvelaInvalidConfigError);

    expect(errorSpy).toHaveBeenCalledOnce();
    expect(errorSpy.mock.calls[0]![0]).toContain('production or Vercel Edge');
  });

  it('accepts non-HTTPS localhost in dev with allowInsecureBaseURL when EdgeRuntime is absent', () => {
    const result = validateSettings({
      wallet: VALID_WALLET,
      baseURL: 'http://localhost:3000',
      allowInsecureBaseURL: true,
    });
    expect(result.baseURL).toBe('http://localhost:3000/v1');
  });

  // (e) test-mode + NODE_ENV=test + localhost → accepts
  it('accepts non-HTTPS baseURL in test mode with localhost hostname', () => {
    process.env.NODE_ENV = 'test';
    process.env.SOLVELA_AI_SDK_PROVIDER_TEST_MODE = 'true';
    const result = validateSettings({
      wallet: VALID_WALLET,
      baseURL: 'http://localhost:8402',
    });
    expect(result.baseURL).toBe('http://localhost:8402/v1');
  });

  it('accepts 127.0.0.1 as localhost equivalent in test mode', () => {
    process.env.NODE_ENV = 'test';
    process.env.SOLVELA_AI_SDK_PROVIDER_TEST_MODE = 'true';
    const result = validateSettings({
      wallet: VALID_WALLET,
      baseURL: 'http://127.0.0.1:8402',
    });
    expect(result.baseURL).toBe('http://127.0.0.1:8402/v1');
  });

  // (f) test-mode + hostname=example.com → throws (no console.error)
  it('throws when test-mode is set but hostname is not localhost', () => {
    process.env.NODE_ENV = 'test';
    process.env.SOLVELA_AI_SDK_PROVIDER_TEST_MODE = 'true';
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => undefined);

    expect(() =>
      validateSettings({ wallet: VALID_WALLET, baseURL: 'http://example.com' }),
    ).toThrow(SolvelaInvalidConfigError);

    expect(errorSpy).not.toHaveBeenCalled();
  });

  // (g) prod + SOLVELA_ALLOW_INSECURE_BASE_URL=true env → console.error + throws
  it('emits console.error and throws in production even when SOLVELA_ALLOW_INSECURE_BASE_URL=true', () => {
    process.env.NODE_ENV = 'production';
    process.env.SOLVELA_ALLOW_INSECURE_BASE_URL = 'true';
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => undefined);

    expect(() =>
      validateSettings({ wallet: VALID_WALLET, baseURL: 'http://api.example.com' }),
    ).toThrow(SolvelaInvalidConfigError);

    expect(errorSpy).toHaveBeenCalledOnce();
    expect(errorSpy.mock.calls[0]![0]).toContain('REFUSED');
  });

  // Positive: SOLVELA_ALLOW_INSECURE_BASE_URL=true in dev → accepts
  it('accepts non-HTTPS baseURL in dev when SOLVELA_ALLOW_INSECURE_BASE_URL=true', () => {
    process.env.SOLVELA_ALLOW_INSECURE_BASE_URL = 'true';
    const result = validateSettings({
      wallet: VALID_WALLET,
      baseURL: 'http://localhost:8402',
    });
    expect(result.baseURL).toBe('http://localhost:8402/v1');
  });
});

// ===========================================================================
// 3. Missing wallet and invalid wallet shapes
// ===========================================================================

describe('wallet validation', () => {
  it('throws SolvelaInvalidConfigError when wallet is absent', () => {
    expect(() => validateSettings({ baseURL: 'https://api.solvela.ai/v1' })).toThrow(
      SolvelaInvalidConfigError,
    );
  });

  it('throws SolvelaInvalidConfigError when wallet is null', () => {
    expect(() =>
      validateSettings({ wallet: null, baseURL: 'https://api.solvela.ai/v1' }),
    ).toThrow(SolvelaInvalidConfigError);
  });

  it('throws SolvelaInvalidConfigError when wallet is a string', () => {
    expect(() =>
      validateSettings({ wallet: 'not-an-object', baseURL: 'https://api.solvela.ai/v1' }),
    ).toThrow(SolvelaInvalidConfigError);
  });

  it('throws SolvelaInvalidConfigError when wallet has signPayment but no label', () => {
    expect(() =>
      validateSettings({
        wallet: { signPayment: async () => 'sig' },
        baseURL: 'https://api.solvela.ai/v1',
      }),
    ).toThrow(SolvelaInvalidConfigError);
  });

  it('throws SolvelaInvalidConfigError when wallet has label but no signPayment', () => {
    expect(() =>
      validateSettings({
        wallet: { label: 'test' },
        baseURL: 'https://api.solvela.ai/v1',
      }),
    ).toThrow(SolvelaInvalidConfigError);
  });

  it('throws SolvelaInvalidConfigError when wallet is an empty object', () => {
    expect(() =>
      validateSettings({ wallet: {}, baseURL: 'https://api.solvela.ai/v1' }),
    ).toThrow(SolvelaInvalidConfigError);
  });

  it('accepts a valid wallet adapter with label and signPayment', () => {
    const result = validateSettings({ wallet: VALID_WALLET });
    expect(result.wallet).toBe(VALID_WALLET);
  });

  it('SolvelaInvalidConfigError.isInstance identifies the thrown error correctly', () => {
    let caught: unknown;
    try {
      validateSettings({ baseURL: 'https://api.solvela.ai/v1' });
    } catch (err) {
      caught = err;
    }
    expect(SolvelaInvalidConfigError.isInstance(caught)).toBe(true);
  });
});

// ===========================================================================
// 4. PAYMENT-SIGNATURE header filtering
// ===========================================================================

describe('PAYMENT-SIGNATURE header filtering', () => {
  it('removes PAYMENT-SIGNATURE in exact uppercase and emits warn once', () => {
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    const result = validateSettings({
      wallet: VALID_WALLET,
      headers: { 'PAYMENT-SIGNATURE': 'abc', 'X-Custom': 'keep' },
    });
    expect(result.headers).not.toHaveProperty('PAYMENT-SIGNATURE');
    expect(result.headers).toHaveProperty('X-Custom', 'keep');
    expect(warnSpy).toHaveBeenCalledOnce();
    expect(warnSpy.mock.calls[0]![0]).toContain('PAYMENT-SIGNATURE');
  });

  it('removes payment-signature in lowercase', () => {
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    const result = validateSettings({
      wallet: VALID_WALLET,
      headers: { 'payment-signature': 'abc' },
    });
    expect(result.headers).not.toHaveProperty('payment-signature');
    expect(warnSpy).toHaveBeenCalledOnce();
  });

  it('removes Payment-Signature in mixed case', () => {
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    const result = validateSettings({
      wallet: VALID_WALLET,
      headers: { 'Payment-Signature': 'abc' },
    });
    expect(result.headers).not.toHaveProperty('Payment-Signature');
    expect(warnSpy).toHaveBeenCalledOnce();
  });

  it('does not emit warn when PAYMENT-SIGNATURE header is absent', () => {
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    validateSettings({ wallet: VALID_WALLET, headers: { 'X-Custom': 'keep' } });
    expect(warnSpy).not.toHaveBeenCalled();
  });

  it('returns empty headers object when no headers are provided', () => {
    const result = validateSettings({ wallet: VALID_WALLET });
    expect(result.headers).toEqual({});
  });

  it('warn fires only once even when called twice (warnOnce behavior)', () => {
    // Both calls share the same module import within this test
    const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    validateSettings({ wallet: VALID_WALLET, headers: { 'PAYMENT-SIGNATURE': 'x' } });
    validateSettings({ wallet: VALID_WALLET, headers: { 'PAYMENT-SIGNATURE': 'y' } });
    expect(warnSpy).toHaveBeenCalledOnce();
  });
});

// ===========================================================================
// 5. sessionBudget resolution
// ===========================================================================

describe('sessionBudget resolution', () => {
  it('returns undefined when sessionBudget is not set and env var is absent', () => {
    const result = validateSettings({ wallet: VALID_WALLET });
    expect(result.sessionBudget).toBeUndefined();
  });

  it('parses SOLVELA_SESSION_BUDGET env var as bigint', () => {
    process.env.SOLVELA_SESSION_BUDGET = '1000000';
    const result = validateSettings({ wallet: VALID_WALLET });
    expect(result.sessionBudget).toBe(1_000_000n);
  });

  it('explicit sessionBudget setting takes precedence over env var', () => {
    process.env.SOLVELA_SESSION_BUDGET = '1000000';
    const result = validateSettings({ wallet: VALID_WALLET, sessionBudget: 500n });
    expect(result.sessionBudget).toBe(500n);
  });

  it('throws SolvelaInvalidConfigError when SOLVELA_SESSION_BUDGET is not a valid integer', () => {
    process.env.SOLVELA_SESSION_BUDGET = 'not-a-number';
    expect(() => validateSettings({ wallet: VALID_WALLET })).toThrow(SolvelaInvalidConfigError);
  });
});

// ===========================================================================
// 6. maxBodyBytes resolution
// ===========================================================================

describe('maxBodyBytes resolution', () => {
  it('defaults to 1_000_000 when no setting or env var is provided', () => {
    const result = validateSettings({ wallet: VALID_WALLET });
    expect(result.maxBodyBytes).toBe(1_000_000);
  });

  it('uses SOLVELA_MAX_SIGNED_BODY_BYTES env var when no explicit setting', () => {
    process.env.SOLVELA_MAX_SIGNED_BODY_BYTES = '512000';
    const result = validateSettings({ wallet: VALID_WALLET });
    expect(result.maxBodyBytes).toBe(512_000);
  });

  it('explicit maxBodyBytes setting takes precedence over env var', () => {
    process.env.SOLVELA_MAX_SIGNED_BODY_BYTES = '512000';
    const result = validateSettings({ wallet: VALID_WALLET, maxBodyBytes: 250_000 });
    expect(result.maxBodyBytes).toBe(250_000);
  });

  it('throws SolvelaInvalidConfigError when SOLVELA_MAX_SIGNED_BODY_BYTES is not a positive integer', () => {
    process.env.SOLVELA_MAX_SIGNED_BODY_BYTES = 'bad-value';
    expect(() => validateSettings({ wallet: VALID_WALLET })).toThrow(SolvelaInvalidConfigError);
  });

  it('throws SolvelaInvalidConfigError when SOLVELA_MAX_SIGNED_BODY_BYTES is zero', () => {
    process.env.SOLVELA_MAX_SIGNED_BODY_BYTES = '0';
    expect(() => validateSettings({ wallet: VALID_WALLET })).toThrow(SolvelaInvalidConfigError);
  });

  it('throws SolvelaInvalidConfigError when SOLVELA_MAX_SIGNED_BODY_BYTES is negative', () => {
    process.env.SOLVELA_MAX_SIGNED_BODY_BYTES = '-100';
    expect(() => validateSettings({ wallet: VALID_WALLET })).toThrow(SolvelaInvalidConfigError);
  });
});

// ===========================================================================
// 7. Normalized settings pass-through for non-validated fields
// ===========================================================================

describe('normalized settings pass-through', () => {
  it('passes apiKey through to normalized settings', () => {
    const result = validateSettings({ wallet: VALID_WALLET, apiKey: 'test-key' });
    expect(result.apiKey).toBe('test-key');
  });

  it('returns undefined apiKey when not provided', () => {
    const result = validateSettings({ wallet: VALID_WALLET });
    expect(result.apiKey).toBeUndefined();
  });

  it('defaults supportsStructuredOutputs to false', () => {
    const result = validateSettings({ wallet: VALID_WALLET });
    expect(result.supportsStructuredOutputs).toBe(false);
  });

  it('passes supportsStructuredOutputs: true through', () => {
    const result = validateSettings({ wallet: VALID_WALLET, supportsStructuredOutputs: true });
    expect(result.supportsStructuredOutputs).toBe(true);
  });

  it('passes custom fetch function through', () => {
    const customFetch = async () => new Response();
    const result = validateSettings({ wallet: VALID_WALLET, fetch: customFetch as never });
    expect(result.fetch).toBe(customFetch);
  });

  it('returns allowInsecureBaseURL as false when not set', () => {
    const result = validateSettings({ wallet: VALID_WALLET });
    expect(result.allowInsecureBaseURL).toBe(false);
  });
});
