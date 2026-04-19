/**
 * Solvela x402 signer adapter for the OpenClaw Provider Plugin.
 *
 * Wraps createPaymentHeader from @solvela/sdk with:
 *   - Budget mutex for safe concurrent session-spend tracking
 *   - Stub-header guard (HF3 pattern from Phase 1)
 *   - SigningError wrapping — private key bytes never appear in thrown messages
 *   - Escrow-deposit counter with per-10-deposit WARN (HF-P3-H6)
 *
 * Security invariants:
 *   - SOLANA_WALLET_KEY is read from env per-call, never stored on the module
 *   - err.cause is never stringified into user-visible error messages
 *   - Stub headers (STUB_BASE64_TX, STUB_ESCROW_DEPOSIT_TX) are rejected before
 *     injecting into the outbound request
 *   - SOLANA_WALLET_KEY absence is detected before any budget reservation (HF-P3-L2)
 */

import { Mutex } from 'async-mutex';
import { createPaymentHeader, SigningError } from '@solvela/sdk/x402';
import type { PaymentRequired } from '@solvela/sdk/types';

export type { SigningError };

export interface SignerOptions {
  /** Optional session spend cap in USDC. Rejects calls that would exceed it. */
  sessionBudget?: number;
  /** Payment signing mode (default: 'direct' — see HF-P3-H6). 'off' is handled by the caller (index.ts) before reaching SolvelaSigner. */
  signingMode?: 'auto' | 'escrow' | 'direct';
  /**
   * Optional DI override for createPaymentHeader — for testing stub-guard behavior.
   * In production this is always undefined; the real createPaymentHeader from
   * @solvela/sdk is used. Tests that need to inject a known stub header use this.
   * (HF-P3-H3)
   */
  _createPaymentHeaderFn?: (
    paymentInfo: PaymentRequired,
    resourceUrl: string,
    privateKey: string | undefined,
    requestBody: string,
  ) => Promise<string>;
}

/**
 * x402 signer for OpenClaw's wrapStreamFn hook.
 *
 * Create one instance per plugin registration; reuse across calls.
 * The budget mutex serialises concurrent access to sessionSpent.
 *
 * Default signing mode is 'direct'. Escrow mode works but relies on gateway
 * auto-claim after max_timeout_seconds; direct mode is recommended until
 * the F4 escrow-claim hook lands.
 */
export class SolvelaSigner {
  private readonly sessionBudget?: number;
  private readonly signingMode: 'auto' | 'escrow' | 'direct';
  private sessionSpent = 0;
  /** Count of escrow/auto deposits — emits a WARN every 10 (HF-P3-H6). */
  private escrowDepositCount = 0;
  /** Mutex ensures budget check + increment is atomic across parallel calls. */
  private readonly budgetMutex = new Mutex();
  /** DI override for createPaymentHeader — tests only (HF-P3-H3). */
  private readonly _createPaymentHeaderFn: SignerOptions['_createPaymentHeaderFn'];

  constructor(opts: SignerOptions = {}) {
    this.sessionBudget = opts.sessionBudget;
    this.signingMode = opts.signingMode ?? 'direct';
    this._createPaymentHeaderFn = opts._createPaymentHeaderFn;
  }

  /**
   * Build a payment-signature header for a 402 PaymentRequired response.
   *
   * Order of operations (fail-fast, no side effects before all guards pass):
   *   1. Validate SOLANA_WALLET_KEY is set (L2 fail-closed)
   *   2. Validate cost is finite (M9)
   *   3. filterAccepts — throws if no matching scheme (H7, before reservation)
   *   4. Budget reservation inside mutex
   *   5. createPaymentHeader — on failure → refundBudget then rethrow
   *   6. Stub-header guard — on stub → refundBudget then throw
   *   7. Escrow deposit counter (H6)
   *
   * @param paymentInfo - Parsed 402 payload from the gateway
   * @param resourceUrl - The URL that returned 402
   * @param requestBody - JSON-serialized request body (used for escrow service_id)
   * @returns base64-encoded PAYMENT-SIGNATURE header value
   * @throws Error with sanitized message on budget exceeded or signing failure
   */
  async buildHeader(
    paymentInfo: PaymentRequired,
    resourceUrl: string,
    requestBody: string,
  ): Promise<string> {
    // Step 1 — fail-closed if key vanished after plugin load (HF-P3-L2)
    const privateKey = process.env['SOLANA_WALLET_KEY'];
    if (!privateKey) {
      throw new Error(
        'SOLANA_WALLET_KEY is not set at signing time. ' +
          'The key may have been unset after plugin load. Refusing to proceed.',
      );
    }

    // Step 2 — validate cost is a finite, non-negative number (HF-P3-M9)
    const cost = parseFloat(paymentInfo.cost_breakdown?.total ?? 'NaN');
    if (!Number.isFinite(cost) || cost < 0) {
      throw new Error(`Gateway 402 has invalid cost: ${paymentInfo.cost_breakdown?.total}`);
    }

    // Step 3 — filterAccepts before budget reservation (HF-P3-H7)
    // Throws if no matching scheme — zero budget impact on failure.
    const filteredPaymentInfo = this.filterAccepts(paymentInfo);

    // Step 4 — atomic budget reservation (mirrors Phase 1 client.ts T1-H pattern)
    await this.budgetMutex.runExclusive(async () => {
      if (this.sessionBudget !== undefined && this.sessionSpent + cost > this.sessionBudget) {
        throw new Error(
          `Solvela session budget $${this.sessionBudget.toFixed(6)} USDC exceeded ` +
            `(spent: $${this.sessionSpent.toFixed(6)}, request cost: $${cost.toFixed(6)})`,
        );
      }
      this.sessionSpent += cost;
    });

    // Step 5 — sign; refund budget on any failure.
    // Use the DI override if provided (tests only — HF-P3-H3); otherwise the real SDK.
    const signFn = this._createPaymentHeaderFn ?? createPaymentHeader;
    let paymentHeader: string;
    try {
      paymentHeader = await signFn(
        filteredPaymentInfo,
        resourceUrl,
        privateKey,
        requestBody,
      );
    } catch (err) {
      // Refund the reserved budget — signer never succeeded
      await this.refundBudget(cost);
      // HF1 pattern: never propagate err.cause — may contain raw key bytes
      if (err instanceof SigningError) {
        throw new Error(`Payment signing failed: ${err.message}`);
      }
      throw new Error(
        `Unexpected error during payment signing: ${err instanceof Error ? err.message : String(err)}`,
      );
    }

    // Step 6 — Stub-header guard (HF3 pattern): reject stub tx before injecting.
    // Guard is always active when privateKey is set (key present → real signing expected).
    let isStub = false;
    try {
      const decoded = JSON.parse(
        typeof atob === 'function'
          ? atob(paymentHeader)
          : Buffer.from(paymentHeader, 'base64').toString('utf-8'),
      );
      const tx = decoded?.payload?.transaction;
      const depositTx = decoded?.payload?.deposit_tx;
      if (
        (typeof tx === 'string' && tx.startsWith('STUB_')) ||
        (typeof depositTx === 'string' && depositTx.startsWith('STUB_'))
      ) {
        isStub = true;
      }
    } catch (err) {
      // Narrow to SyntaxError — malformed base64/JSON is not a stub (HF-P3-L3)
      if (!(err instanceof SyntaxError)) {
        throw err;
      }
      // SyntaxError: decode failure is not a stub — let the gateway handle it
    }

    if (isStub) {
      // Refund before throwing
      await this.refundBudget(cost);
      throw new Error(
        'Payment signing returned a stub transaction. Reinstall @solvela/openclaw-provider ' +
          'and ensure @solana/web3.js peer deps are resolvable at runtime.',
      );
    }

    // Step 7 — Escrow deposit counter WARN (HF-P3-H6)
    // Emit a warning every 10 escrow/auto deposits to remind users about auto-claim reliance.
    if (this.signingMode === 'escrow' || this.signingMode === 'auto') {
      this.escrowDepositCount++;
      if (this.escrowDepositCount % 10 === 0) {
        process.stderr.write(
          `[solvela-openclaw] WARN: ${this.escrowDepositCount} escrow deposits made; ` +
            'auto-claim relies on gateway timeout. Direct mode recommended until F4 escrow-claim hook lands.\n',
        );
      }
    }

    return paymentHeader;
  }

  /**
   * Refund cost from session budget (used when inner() throws after signing).
   *
   * Clamps to zero and emits a WARN if the refund amount exceeds what was
   * spent — that indicates a race-condition bug in budget accounting (HF-P3-H2).
   */
  async refundBudget(cost: number): Promise<void> {
    await this.budgetMutex.runExclusive(async () => {
      const before = this.sessionSpent;
      if (before > 0 && before < cost) {
        process.stderr.write(
          `[solvela-openclaw] WARN: budget refund clamped (before=${before}, refund=${cost}). ` +
            'Possible race condition in budget accounting — please report.\n',
        );
      }
      this.sessionSpent = Math.max(0, this.sessionSpent - cost);
    });
  }

  /** Current session spend in USDC (for observability). */
  getSessionSpent(): number {
    return this.sessionSpent;
  }

  /** Filter payment accepts by signing mode before passing to the SDK. */
  private filterAccepts(paymentInfo: PaymentRequired): PaymentRequired {
    if (this.signingMode === 'auto') return paymentInfo;

    const filtered = paymentInfo.accepts.filter((a) => {
      if (this.signingMode === 'escrow') return a.scheme === 'escrow';
      if (this.signingMode === 'direct') return a.scheme !== 'escrow';
      return true;
    });

    if (filtered.length === 0) {
      throw new Error(
        `No payment accepts match signing mode '${this.signingMode}'. ` +
          'Gateway offered: ' +
          paymentInfo.accepts.map((a) => a.scheme).join(', '),
      );
    }

    return { ...paymentInfo, accepts: filtered };
  }
}
