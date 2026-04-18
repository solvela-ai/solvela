/**
 * @solvela/ai-sdk-provider — public entry point.
 *
 * Re-exports the factory, types, error classes, and model registry.
 * Zero logic lives here — this file is pure re-exports.
 */

// ---------------------------------------------------------------------------
// Core factory + types
// ---------------------------------------------------------------------------

export { createSolvelaProvider } from './provider.js';
export type { SolvelaProvider } from './provider.js';
export type { SolvelaProviderSettings } from './config.js';
export type { SolvelaWalletAdapter } from './wallet-adapter.js';

// ---------------------------------------------------------------------------
// Error classes
// ---------------------------------------------------------------------------

export {
  SolvelaPaymentError,
  SolvelaBudgetExceededError,
  SolvelaSigningError,
  SolvelaInvalidConfigError,
  SolvelaUpstreamError,
} from './errors.js';

// ---------------------------------------------------------------------------
// Model registry (auto-generated)
// ---------------------------------------------------------------------------

export type { SolvelaModelId } from './generated/models.js';
export { MODELS } from './generated/models.js';

// ---------------------------------------------------------------------------
// Default singleton — like @ai-sdk/openai's `openai`
//
// IMPORTANT: this singleton has no wallet configured. Every method call throws
// SolvelaInvalidConfigError with a helpful message directing the user to call
// createSolvelaProvider({ wallet }) instead. Module load is side-effect-free.
// ---------------------------------------------------------------------------

import { SolvelaInvalidConfigError } from './errors.js';
import type { SolvelaProvider } from './provider.js';

const NO_WALLET_MESSAGE =
  '[solvela] The default `solvela` singleton has no wallet configured. ' +
  'Call createSolvelaProvider({ wallet: yourAdapter }) to create a provider ' +
  'with a wallet. See @solvela/ai-sdk-provider/adapters/local for a dev/test adapter.';

function throwNoWallet(): never {
  throw new SolvelaInvalidConfigError({ message: NO_WALLET_MESSAGE });
}

/**
 * Default Solvela provider singleton (analogous to @ai-sdk/openai's `openai`).
 *
 * Every property access or call throws SolvelaInvalidConfigError because no
 * wallet adapter is configured. This allows consumers to import the package
 * for types without triggering a module-load error.
 *
 * To use, call createSolvelaProvider({ wallet: yourAdapter }) instead.
 */
export const solvela: SolvelaProvider = new Proxy(
  // Base target: a function so the Proxy is callable.
  function solvelaNoWallet() {
    throwNoWallet();
  } as unknown as SolvelaProvider,
  {
    apply(): never {
      throwNoWallet();
    },
    get(_target, prop: string | symbol): never {
      // Allow typeof checks and Symbol.toPrimitive without throwing.
      if (prop === Symbol.toPrimitive || prop === 'then') {
        // Returning undefined for 'then' prevents accidental Promise treatment.
        return undefined as never;
      }
      throwNoWallet();
    },
  },
);
