/**
 * createSolvelaProvider — main factory for the Solvela AI SDK provider.
 *
 * Wraps createOpenAICompatible from @ai-sdk/openai-compatible with a
 * validated config and the Phase 3 fetch wrapper that implements the real
 * 402-sign-retry loop and per-provider budget state.
 */

import { createOpenAICompatible } from '@ai-sdk/openai-compatible';
import { UnsupportedFunctionalityError } from '@ai-sdk/provider';
import type { LanguageModelV3 } from '@ai-sdk/provider';

import { BudgetState } from './budget.js';
import type { SolvelaProviderSettings } from './config.js';
import { validateSettings } from './config.js';
import { createSolvelaFetch } from './fetch-wrapper.js';
import type { SolvelaModelId } from './generated/models.js';

// ---------------------------------------------------------------------------
// SolvelaProvider public type
// ---------------------------------------------------------------------------

/**
 * A Solvela provider instance. Callable directly or via `.chat()`.
 * `.textEmbeddingModel()` and `.imageModel()` throw UnsupportedFunctionalityError
 * — embeddings and image generation are out of scope for v1.
 */
export interface SolvelaProvider {
  /** Create a language model for the given model ID. */
  (modelId: SolvelaModelId | (string & {})): LanguageModelV3;
  /** Alias for the callable form — explicit method for clarity. */
  chat(modelId: SolvelaModelId | (string & {})): LanguageModelV3;
  /**
   * @throws {UnsupportedFunctionalityError} always — not supported in v1.
   */
  textEmbeddingModel(_id: string): never;
  /**
   * @throws {UnsupportedFunctionalityError} always — not supported in v1.
   */
  imageModel(_id: string): never;
}

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

/**
 * Create a Solvela AI SDK provider.
 *
 * Configuration is validated synchronously at construction time.
 * Throws SolvelaInvalidConfigError immediately if settings are invalid
 * (e.g. missing wallet, bad baseURL).
 *
 * @param settings - Provider configuration. `wallet` is required.
 * @returns A SolvelaProvider callable factory.
 *
 * @example
 * ```typescript
 * import { createSolvelaProvider } from '@solvela/ai-sdk-provider';
 * import { createLocalWalletAdapter } from '@solvela/ai-sdk-provider/adapters/local';
 *
 * const solvela = createSolvelaProvider({
 *   wallet: createLocalWalletAdapter(keypair),
 * });
 *
 * const model = solvela('anthropic-claude-sonnet-4-5');
 * ```
 */
export function createSolvelaProvider(
  settings: SolvelaProviderSettings,
): SolvelaProvider {
  // Throws SolvelaInvalidConfigError synchronously on bad input.
  const normalized = validateSettings(settings);

  // Per-provider budget state. `undefined` sessionBudget disables the cap.
  const budget = new BudgetState(normalized.sessionBudget);

  // The real 402-sign-retry fetch. If the caller provided a test-seam
  // `settings.fetch`, thread it through as the base fetch so test doubles
  // compose with the payment logic (rather than bypassing it).
  const solvelaFetch = createSolvelaFetch({
    wallet: normalized.wallet,
    budget,
    maxSignedBodyBytes: normalized.maxBodyBytes,
    baseFetch: normalized.fetch as typeof globalThis.fetch | undefined,
  });

  // Build the inner openai-compatible provider with the validated config.
  const inner = createOpenAICompatible({
    name: 'solvela',
    baseURL: normalized.baseURL,
    apiKey: normalized.apiKey,
    headers: Object.keys(normalized.headers).length > 0
      ? normalized.headers
      : undefined,
    supportsStructuredOutputs: normalized.supportsStructuredOutputs,
    fetch: solvelaFetch,
  });

  // Build the callable provider function.
  const provider = function solvelaProvider(
    modelId: SolvelaModelId | (string & {}),
  ): LanguageModelV3 {
    return inner.chatModel(modelId);
  } as SolvelaProvider;

  // Explicit .chat() method — alias for the callable form.
  provider.chat = function chat(
    modelId: SolvelaModelId | (string & {}),
  ): LanguageModelV3 {
    return inner.chatModel(modelId);
  };

  // Throw eagerly for unsupported model families (v1 scope: chat text + tools only).
  provider.textEmbeddingModel = function textEmbeddingModel(
    _id: string,
  ): never {
    throw new UnsupportedFunctionalityError({
      functionality:
        'textEmbeddingModel — Solvela AI SDK provider v1 supports chat/text language models only.',
    });
  };

  provider.imageModel = function imageModel(_id: string): never {
    throw new UnsupportedFunctionalityError({
      functionality:
        'imageModel — Solvela AI SDK provider v1 supports chat/text language models only.',
    });
  };

  return provider;
}
