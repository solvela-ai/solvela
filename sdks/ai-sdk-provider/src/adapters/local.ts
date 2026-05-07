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
 * file into the bundle. The signing-only crypto path stays out of the
 * default tree (see tsup `entry` config and tree-shake grep asserted in CI).
 *
 * Signing is delegated to `@solvela/signer-core`'s `createPaymentHeader`,
 * which produces wire-format compatible x402 headers for the production
 * gateway. signer-core hard-deps `@solana/web3.js`, `@solana/spl-token`,
 * and `bs58`, so they are transitively present in any consumer install.
 *
 * @module adapters/local
 */

import { createPaymentHeader } from '@solvela/signer-core';
import bs58 from 'bs58';

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
 * Declared as a structural interface rather than imported from
 * `@solana/web3.js` so consumers passing non-web3.js Keypair-shaped objects
 * (KMS-backed, hardware-backed) are accepted by structural typing.
 */
export interface LocalKeypairLike {
  /** 64-byte Solana secret key. */
  readonly secretKey: Uint8Array;
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const ADAPTER_LABEL = 'local-test-keypair';

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

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/**
 * Create a Solvela wallet adapter backed by a raw Solana `Keypair`.
 *
 * ⚠️ DEVELOPMENT AND TESTING ONLY. See the module-level warning above.
 *
 * The adapter delegates transaction building and Ed25519 signing to
 * `@solvela/signer-core`'s `createPaymentHeader`. No SPL / crypto logic is
 * reimplemented here — the adapter is a thin bridge from the AI SDK
 * provider's `SolvelaWalletAdapter` interface to the shared signer.
 *
 * Thrown errors:
 * - `SolvelaInvalidConfigError` if `keypair.secretKey` is not a Uint8Array.
 * - `AbortError` (preserved identity) if the supplied `signal` is aborted
 *   before or after the underlying signing call.
 * - Any error raised by `createPaymentHeader` (typically `SigningError` from
 *   signer-core) propagates unchanged; the fetch-wrapper wraps it in
 *   `SolvelaSigningError` whose constructor scrubs base58/hex from the
 *   cause message.
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
      // Pre-call abort check. Defense-in-depth — the underlying signer does
      // not honour AbortSignal itself.
      if (args.signal?.aborted) {
        throw makeAbortError();
      }

      // Encode the 64-byte secret key as base58 for signer-core's API.
      // NOTE: privateKeyB58 cannot be securely zeroed in JavaScript — strings
      // are immutable in the V8 heap until GC. This is dev/test-only code
      // (see module banner); production signers must use a wallet boundary
      // that does not materialize the secret in JS string memory.
      const privateKeyB58 = bs58.encode(keypair.secretKey);

      // Let createPaymentHeader errors propagate unchanged. The fetch-wrapper
      // catches them and constructs SolvelaSigningError, whose constructor
      // runs redactBase58(redactHex(...)) on the cause message.
      const header = await createPaymentHeader(
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
