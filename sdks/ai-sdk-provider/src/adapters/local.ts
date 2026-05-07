/**
 * @solvela/ai-sdk-provider/adapters/local
 *
 * ⚠️  DEVELOPMENT AND TESTING ONLY — not for production key material.
 *
 * Production users: implement your own adapter backed by a hardware wallet,
 * MPC signer, or wallet-standard adapter. This reference adapter reads raw
 * private-key bytes from the process and is intended for local dev + tests
 * only.
 *
 * This module is shipped as a SEPARATE subpath entry point:
 *   `@solvela/ai-sdk-provider/adapters/local`
 *
 * Importing the main `@solvela/ai-sdk-provider` package does NOT pull this
 * file into the bundle. No key-material code or crypto peer dependencies
 * leak into the default tree (see tsup `entry` config and tree-shake grep
 * asserted in CI).
 *
 * Runtime peer dependencies (loaded via dynamic `import()` on first call so
 * the provider's main entry stays free of Solana crypto code):
 *   - @solvela/sdk
 *   - @solana/web3.js
 *   - @solana/spl-token
 *   - bs58
 *
 * If any peer is missing at runtime, `signPayment` throws
 * `SolvelaInvalidConfigError` with an install instruction. The peers are
 * declared `optional: true` in the provider's `package.json`.
 *
 * @module adapters/local
 */

import { SolvelaInvalidConfigError } from '../errors.js';
import type {
  SolvelaPaymentRequired,
  SolvelaWalletAdapter,
} from '../wallet-adapter.js';

// ---------------------------------------------------------------------------
// Local types
// ---------------------------------------------------------------------------

/**
 * Structural shape of a Solana `Keypair`. Matches `Keypair` from
 * `@solana/web3.js` v1 (which exposes `secretKey: Uint8Array` of 64 bytes).
 *
 * Declared locally as an `interface` rather than imported from
 * `@solana/web3.js` so this file typechecks even when the optional peer is
 * not installed. Consumers pass the real `Keypair`; structural typing
 * accepts it.
 */
export interface LocalKeypairLike {
  /** 64-byte Solana secret key. */
  readonly secretKey: Uint8Array;
}

/**
 * Minimal shape of the `@solvela/sdk` module used at runtime. Declared
 * locally to avoid a hard dependency on the `@solvela/sdk` type package,
 * which is an optional peer and may not be resolvable at compile time in
 * downstream consumer environments.
 */
interface SolvelaSdkModule {
  createPaymentHeader(
    paymentInfo: SolvelaPaymentRequired,
    resourceUrl: string,
    privateKey?: string,
    requestBody?: string,
  ): Promise<string>;
}

/**
 * Minimal shape of the `bs58` module used at runtime. Declared locally for
 * the same reason as `SolvelaSdkModule`.
 */
interface Bs58Module {
  encode(bytes: Uint8Array): string;
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const ADAPTER_LABEL = 'local-test-keypair';

const PEER_INSTALL_MESSAGE =
  '@solvela/ai-sdk-provider/adapters/local requires peers: @solvela/sdk, ' +
  '@solana/web3.js, @solana/spl-token, bs58. Install them: npm install ' +
  '@solvela/sdk @solana/web3.js @solana/spl-token bs58';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/**
 * Create a fresh `AbortError`. Mirrors the fetch-wrapper helper so the error
 * identity is recognised by the AI SDK's abort handling.
 */
function makeAbortError(): Error {
  if (typeof DOMException === 'function') {
    return new DOMException('The operation was aborted', 'AbortError');
  }
  const err = new Error('The operation was aborted');
  err.name = 'AbortError';
  return err;
}

/**
 * Load the `@solvela/sdk` module at first call. Any resolution failure is
 * re-thrown as `SolvelaInvalidConfigError` with install instructions.
 *
 * Using a cast to `unknown` avoids coupling the provider's typecheck to the
 * optional peer's declaration files (which may not be installed in every
 * environment that typechecks the provider).
 */
async function loadSolvelaSdk(): Promise<SolvelaSdkModule> {
  try {
    // @ts-expect-error — optional peer dependency; not resolvable when the
    // peer isn't installed (the whole point of this dynamic-import branch).
    // If the peer ever becomes resolvable, this directive will fail and
    // signal that the suppression can be removed. See PEER_INSTALL_MESSAGE.
    const mod = (await import('@solvela/sdk')) as unknown as Record<
      string,
      unknown
    >;
    const candidate =
      (mod['createPaymentHeader'] as SolvelaSdkModule['createPaymentHeader'] | undefined) ??
      ((mod['default'] as Record<string, unknown> | undefined)?.[
        'createPaymentHeader'
      ] as SolvelaSdkModule['createPaymentHeader'] | undefined);
    if (typeof candidate !== 'function') {
      throw new Error('createPaymentHeader export not found on @solvela/sdk');
    }
    return { createPaymentHeader: candidate };
  } catch (err) {
    throw new SolvelaInvalidConfigError({
      message: PEER_INSTALL_MESSAGE,
      cause: err,
    });
  }
}

/**
 * Load `bs58` at first call so the Keypair's secret-key bytes can be encoded
 * into the base58 form expected by `@solvela/sdk`'s `createPaymentHeader`.
 *
 * Re-thrown as `SolvelaInvalidConfigError` with install instructions on
 * resolution failure.
 */
async function loadBs58(): Promise<Bs58Module> {
  try {
    // @ts-expect-error — optional peer dependency; not resolvable when the
    // peer isn't installed. See PEER_INSTALL_MESSAGE.
    const mod = (await import('bs58')) as unknown as Record<
      string,
      unknown
    >;
    const encode =
      (mod['encode'] as Bs58Module['encode'] | undefined) ??
      ((mod['default'] as Record<string, unknown> | undefined)?.[
        'encode'
      ] as Bs58Module['encode'] | undefined);
    if (typeof encode !== 'function') {
      throw new Error('encode export not found on bs58');
    }
    return { encode };
  } catch (err) {
    throw new SolvelaInvalidConfigError({
      message: PEER_INSTALL_MESSAGE,
      cause: err,
    });
  }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/**
 * Create a Solvela wallet adapter backed by a raw Solana `Keypair`.
 *
 * ⚠️ DEVELOPMENT AND TESTING ONLY. See the module-level warning above.
 *
 * The adapter delegates transaction building and Ed25519 signing to
 * `createPaymentHeader` in `@solvela/sdk`. No SPL / crypto logic is
 * reimplemented here — the adapter is a thin bridge from the AI SDK
 * provider's `SolvelaWalletAdapter` interface to the existing SDK function.
 *
 * Thrown errors:
 * - `SolvelaInvalidConfigError` if a peer dependency is missing at runtime.
 * - `AbortError` (preserved identity) if the supplied `signal` is aborted
 *   before or after the underlying signing call.
 * - Any error raised by `@solvela/sdk`'s `createPaymentHeader` propagates
 *   unchanged; the fetch-wrapper wraps it in `SolvelaSigningError` whose
 *   constructor already scrubs base58/hex from the cause message.
 *
 * The `Keypair` reference is captured in a closure — the private key never
 * surfaces in adapter-side error surfaces. The `bs58`-encoded secret key
 * cannot be securely zeroed in JavaScript: it is an immutable string in V8's
 * heap until the runtime schedules a major GC, which under load may be
 * many requests later. Heap dumps captured for any other reason will
 * contain the encoded private key. **This adapter is for development and
 * testing only — do not run it in any process that handles production
 * funds.** A future SDK boundary that accepts `Uint8Array` directly would
 * permit `secretKey.fill(0)` post-call; tracked as a follow-up.
 *
 * @example
 * ```typescript
 * import { Keypair } from '@solana/web3.js';
 * import { createSolvelaProvider } from '@solvela/ai-sdk-provider';
 * import { createLocalWalletAdapter } from '@solvela/ai-sdk-provider/adapters/local';
 *
 * const keypair = Keypair.generate();
 * const solvela = createSolvelaProvider({
 *   wallet: createLocalWalletAdapter(keypair),
 * });
 * ```
 */
export function createLocalWalletAdapter(
  keypair: LocalKeypairLike,
): SolvelaWalletAdapter {
  if (keypair == null || !(keypair.secretKey instanceof Uint8Array)) {
    throw new SolvelaInvalidConfigError({
      message:
        '[solvela] createLocalWalletAdapter requires a Keypair with a ' +
        'secretKey: Uint8Array. Got an object that does not match the shape.',
    });
  }

  return {
    label: ADAPTER_LABEL,
    async signPayment(args: {
      paymentRequired: SolvelaPaymentRequired;
      resourceUrl: string;
      requestBody: string;
      signal?: AbortSignal;
    }): Promise<string> {
      // Pre-call abort check. Defense-in-depth — the underlying SDK does not
      // honour AbortSignal itself.
      if (args.signal?.aborted) {
        throw makeAbortError();
      }

      const [sdk, bs58] = await Promise.all([loadSolvelaSdk(), loadBs58()]);

      // Encode the 64-byte secret key as base58 for the existing SDK API.
      // NOTE: privateKeyB58 cannot be securely zeroed in JavaScript — strings
      // are immutable in the V8 heap until GC. This is dev/test-only code
      // (see module banner); production signers must use a wallet boundary
      // that does not materialize the secret in JS string memory.
      const privateKeyB58 = bs58.encode(keypair.secretKey);

      // Let createPaymentHeader errors propagate unchanged. The fetch-wrapper
      // catches them and constructs SolvelaSigningError, whose constructor
      // runs redactBase58(redactHex(...)) on the cause message.
      const header = await sdk.createPaymentHeader(
        args.paymentRequired,
        args.resourceUrl,
        privateKeyB58,
        args.requestBody,
      );

      // Post-call abort check. If the signal fired during signing, do not
      // return the signed header — surface an AbortError so the fetch-wrapper
      // releases the budget reservation without submitting a signed retry.
      if (args.signal?.aborted) {
        throw makeAbortError();
      }

      return header;
    },
  };
}
