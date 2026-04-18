/**
 * Unit-7: provider.ts factory shape + URL routing (plan §6 Phase 7)
 *
 * Assertion groups:
 *  A. Factory shape — callable form, .chat(), .textEmbeddingModel(), .imageModel()
 *  B. specificationVersion — returned LanguageModel is V3 (v3 channel)
 *  C. URL routing — baseFetch receives exact /v1/chat/completions URL
 *  D. Construction-time validation — missing wallet throws SolvelaInvalidConfigError
 *  E. Default singleton — solvela() throws SolvelaInvalidConfigError directing to createSolvelaProvider
 */

import { UnsupportedFunctionalityError } from '@ai-sdk/provider';
import type { LanguageModelV3, LanguageModelV3CallOptions } from '@ai-sdk/provider';
import { describe, expect, it, vi } from 'vitest';

import { SolvelaInvalidConfigError } from '../../src/errors.js';
import { createSolvelaProvider } from '../../src/provider.js';
import { solvela } from '../../src/index.js';
import type { SolvelaWalletAdapter } from '../../src/wallet-adapter.js';

// ---------------------------------------------------------------------------
// Shared test helpers
// ---------------------------------------------------------------------------

/** Minimal wallet adapter that satisfies the SolvelaWalletAdapter interface. */
function makeWallet(): SolvelaWalletAdapter {
  return {
    label: 'test-wallet',
    signPayment: vi.fn().mockResolvedValue('base64-sig-placeholder'),
  };
}

/**
 * Build a fetch spy that records the first argument (URL) and returns a
 * canned 200 JSON response. The response body is a minimal OpenAI-compatible
 * chat completion envelope so the openai-compatible layer does not crash on
 * the parse step.
 */
function makeFetchSpy() {
  const capturedUrls: string[] = [];

  const cannedBody = JSON.stringify({
    id: 'chatcmpl-test',
    object: 'chat.completion',
    created: 0,
    model: 'gpt-4o',
    choices: [
      {
        index: 0,
        message: { role: 'assistant', content: 'ok' },
        finish_reason: 'stop',
      },
    ],
    usage: { prompt_tokens: 1, completion_tokens: 1, total_tokens: 2 },
  });

  const spy = vi.fn(async (input: RequestInfo | URL, _init?: RequestInit): Promise<Response> => {
    // Capture the URL string regardless of input shape.
    const urlStr =
      typeof input === 'string'
        ? input
        : input instanceof URL
          ? input.toString()
          : (input as Request).url;
    capturedUrls.push(urlStr);

    return new Response(cannedBody, {
      status: 200,
      headers: { 'content-type': 'application/json' },
    });
  });

  return { spy, capturedUrls };
}

/**
 * Build the minimal LanguageModelV3CallOptions required to invoke doGenerate.
 * Only `prompt` is required; all other fields are optional.
 */
function minimalCallOptions(): LanguageModelV3CallOptions {
  return {
    prompt: [{ role: 'user', content: [{ type: 'text', text: 'ping' }] }],
  };
}

// ---------------------------------------------------------------------------
// A. Factory shape
// ---------------------------------------------------------------------------

describe('A. Factory shape', () => {
  it('returns a callable function when constructed with a valid wallet', () => {
    const provider = createSolvelaProvider({ wallet: makeWallet() });
    expect(typeof provider).toBe('function');
  });

  it('callable form returns an object (LanguageModel) for a given model id', () => {
    const provider = createSolvelaProvider({ wallet: makeWallet() });
    const model = provider('gpt-4o');
    expect(model).toBeDefined();
    expect(typeof model).toBe('object');
  });

  it('exposes a .chat() method that returns a LanguageModel', () => {
    const provider = createSolvelaProvider({ wallet: makeWallet() });
    expect(typeof provider.chat).toBe('function');
    const model = provider.chat('gpt-4o');
    expect(model).toBeDefined();
    expect(typeof model).toBe('object');
  });

  it('exposes a .textEmbeddingModel() method', () => {
    const provider = createSolvelaProvider({ wallet: makeWallet() });
    expect(typeof provider.textEmbeddingModel).toBe('function');
  });

  it('exposes an .imageModel() method', () => {
    const provider = createSolvelaProvider({ wallet: makeWallet() });
    expect(typeof provider.imageModel).toBe('function');
  });

  it('callable form and .chat() return the same model id', () => {
    const provider = createSolvelaProvider({ wallet: makeWallet() });
    const modelA = provider('gpt-4o') as LanguageModelV3;
    const modelB = provider.chat('gpt-4o') as LanguageModelV3;
    expect(modelA.modelId).toBe(modelB.modelId);
  });
});

// ---------------------------------------------------------------------------
// B. specificationVersion === 'v3'
// ---------------------------------------------------------------------------

describe('B. specificationVersion', () => {
  it('callable form returns a LanguageModel with specificationVersion === "v3"', () => {
    const provider = createSolvelaProvider({ wallet: makeWallet() });
    const model = provider('gpt-4o') as LanguageModelV3;
    expect(model.specificationVersion).toBe('v3');
  });

  it('.chat() returns a LanguageModel with specificationVersion === "v3"', () => {
    const provider = createSolvelaProvider({ wallet: makeWallet() });
    const model = provider.chat('claude-sonnet-4-5') as LanguageModelV3;
    expect(model.specificationVersion).toBe('v3');
  });

  it('specificationVersion is "v3" for any arbitrary model id string', () => {
    const provider = createSolvelaProvider({ wallet: makeWallet() });
    const model = provider('some-future-model-id') as LanguageModelV3;
    expect(model.specificationVersion).toBe('v3');
  });
});

// ---------------------------------------------------------------------------
// C. URL routing — T2-B assertion
// ---------------------------------------------------------------------------

describe('C. URL routing via baseFetch spy', () => {
  it('default config routes to https://api.solvela.ai/v1/chat/completions', async () => {
    const { spy, capturedUrls } = makeFetchSpy();
    const provider = createSolvelaProvider({
      wallet: makeWallet(),
      fetch: spy as typeof globalThis.fetch,
    });

    const model = provider('gpt-4o') as LanguageModelV3;
    // doGenerate triggers the underlying fetch. Canned 200 response is a
    // valid OpenAI chat completion envelope so the call should succeed.
    await model.doGenerate(minimalCallOptions());

    // Exactly 1 fetch on the non-402 path — locks the "no preflight" contract.
    expect(spy).toHaveBeenCalledTimes(1);
    expect(capturedUrls[0]).toBe('https://api.solvela.ai/v1/chat/completions');
  });

  it('custom baseURL without /v1 suffix is normalised to baseURL/v1/chat/completions', async () => {
    const { spy, capturedUrls } = makeFetchSpy();
    const provider = createSolvelaProvider({
      wallet: makeWallet(),
      baseURL: 'https://custom.example.com',
      fetch: spy as typeof globalThis.fetch,
    });

    const model = provider('gpt-4o') as LanguageModelV3;
    await model.doGenerate(minimalCallOptions());

    expect(spy).toHaveBeenCalledTimes(1);
    expect(capturedUrls[0]).toBe('https://custom.example.com/v1/chat/completions');
  });

  it('trailing slash on custom baseURL is stripped before /v1 is appended', async () => {
    const { spy, capturedUrls } = makeFetchSpy();
    const provider = createSolvelaProvider({
      wallet: makeWallet(),
      baseURL: 'https://custom.example.com/v1/',
      fetch: spy as typeof globalThis.fetch,
    });

    const model = provider('gpt-4o') as LanguageModelV3;
    await model.doGenerate(minimalCallOptions());

    expect(spy).toHaveBeenCalledTimes(1);
    // /v1/ → strip slash → /v1, no second /v1 appended → /v1/chat/completions
    expect(capturedUrls[0]).toBe('https://custom.example.com/v1/chat/completions');
  });

  it('fetch spy is called exactly once on the non-402 path', async () => {
    const { spy } = makeFetchSpy();
    const provider = createSolvelaProvider({
      wallet: makeWallet(),
      fetch: spy as typeof globalThis.fetch,
    });
    const model = provider('gpt-4o') as LanguageModelV3;
    await model.doGenerate(minimalCallOptions());
    expect(spy).toHaveBeenCalledTimes(1);
  });
});

// ---------------------------------------------------------------------------
// D. Construction-time validation — missing wallet
// ---------------------------------------------------------------------------

describe('D. Construction-time validation', () => {
  it('throws SolvelaInvalidConfigError synchronously when wallet is missing', () => {
    expect(() =>
      createSolvelaProvider({} as Parameters<typeof createSolvelaProvider>[0]),
    ).toThrow(SolvelaInvalidConfigError);
  });

  it('throws SolvelaInvalidConfigError synchronously when wallet is null', () => {
    expect(() =>
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      createSolvelaProvider({ wallet: null as any }),
    ).toThrow(SolvelaInvalidConfigError);
  });

  it('error message from missing wallet describes the configuration problem', () => {
    let caught: unknown;
    try {
      createSolvelaProvider({} as Parameters<typeof createSolvelaProvider>[0]);
    } catch (err) {
      caught = err;
    }
    expect(caught).toBeInstanceOf(SolvelaInvalidConfigError);
    expect((caught as SolvelaInvalidConfigError).message).toMatch(/wallet/i);
  });

  it('SolvelaInvalidConfigError.isInstance() returns true for the thrown error', () => {
    let caught: unknown;
    try {
      createSolvelaProvider({} as Parameters<typeof createSolvelaProvider>[0]);
    } catch (err) {
      caught = err;
    }
    expect(SolvelaInvalidConfigError.isInstance(caught)).toBe(true);
  });
});

// ---------------------------------------------------------------------------
// C-ext. Unsupported model families
// ---------------------------------------------------------------------------

describe('C-ext. Unsupported model families', () => {
  it('textEmbeddingModel() throws UnsupportedFunctionalityError', () => {
    const provider = createSolvelaProvider({ wallet: makeWallet() });
    expect(() => provider.textEmbeddingModel('text-embedding-3-small')).toThrow(
      UnsupportedFunctionalityError,
    );
  });

  it('textEmbeddingModel() error message mentions text embedding or v1 scope', () => {
    const provider = createSolvelaProvider({ wallet: makeWallet() });
    let caught: unknown;
    try {
      provider.textEmbeddingModel('text-embedding-3-small');
    } catch (err) {
      caught = err;
    }
    expect((caught as Error).message).toMatch(/textEmbeddingModel/i);
  });

  it('imageModel() throws UnsupportedFunctionalityError', () => {
    const provider = createSolvelaProvider({ wallet: makeWallet() });
    expect(() => provider.imageModel('dall-e-3')).toThrow(
      UnsupportedFunctionalityError,
    );
  });

  it('imageModel() error message mentions imageModel or v1 scope', () => {
    const provider = createSolvelaProvider({ wallet: makeWallet() });
    let caught: unknown;
    try {
      provider.imageModel('dall-e-3');
    } catch (err) {
      caught = err;
    }
    expect((caught as Error).message).toMatch(/imageModel/i);
  });
});

// ---------------------------------------------------------------------------
// E. Default singleton proxy
// ---------------------------------------------------------------------------

describe('E. Default solvela singleton', () => {
  it('calling solvela() throws SolvelaInvalidConfigError', () => {
    expect(() => solvela('gpt-4o')).toThrow(SolvelaInvalidConfigError);
  });

  it('solvela() error message directs user to createSolvelaProvider', () => {
    let caught: unknown;
    try {
      solvela('gpt-4o');
    } catch (err) {
      caught = err;
    }
    expect((caught as SolvelaInvalidConfigError).message).toMatch(
      /createSolvelaProvider/,
    );
  });

  it('solvela.chat() throws SolvelaInvalidConfigError', () => {
    expect(() => solvela.chat('gpt-4o')).toThrow(SolvelaInvalidConfigError);
  });

  it('solvela.textEmbeddingModel() throws SolvelaInvalidConfigError', () => {
    expect(() => solvela.textEmbeddingModel('text-embedding-3-small')).toThrow(
      SolvelaInvalidConfigError,
    );
  });

  it('solvela.imageModel() throws SolvelaInvalidConfigError', () => {
    expect(() => solvela.imageModel('dall-e-3')).toThrow(SolvelaInvalidConfigError);
  });

  it('SolvelaInvalidConfigError.isInstance() returns true for singleton error', () => {
    let caught: unknown;
    try {
      solvela('gpt-4o');
    } catch (err) {
      caught = err;
    }
    expect(SolvelaInvalidConfigError.isInstance(caught)).toBe(true);
  });
});
