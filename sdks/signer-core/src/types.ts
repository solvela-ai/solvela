/**
 * Shared x402 protocol types used across Solvela SDK packages.
 *
 * `PaymentRequired`, `PaymentAccept`, and `CostBreakdown` are derived
 * from the valibot schemas in `schema.ts` — the schema is the single
 * source of truth for the wire format. See issue #129.
 *
 * `PaymentExpectations` stays a plain interface declared here: it's a
 * *caller-side* knob, not part of the wire format, so it has no
 * counterpart in the validation schema.
 */

export type {
  CostBreakdown,
  PaymentAccept,
  PaymentRequired,
} from './schema.js';

/**
 * Caller-side validation knobs for `createPaymentHeader`.
 *
 * The 402 response is attacker-controlled — a phishing gateway, MITM, or
 * compromised DNS can return arbitrary `pay_to`, `amount`, and
 * `escrow_program_id` values. Without these knobs, the signer trusts those
 * fields and signs a transaction draining the agent wallet.
 *
 * Callers SHOULD always pass `recipient`. `maxAmount` and `escrowProgramId`
 * are recommended for any caller that knows them statically.
 */
export interface PaymentExpectations {
  /** The chosen accept's `pay_to` MUST equal this. */
  recipient?: string;
  /**
   * Maximum amount the caller authorizes, in atomic units (USDC has 6
   * decimals — `1000000` = 1 USDC). Accepts `bigint` or a non-negative
   * integer string.
   */
  maxAmount?: bigint | string;
  /** When the chosen scheme is `escrow`, `accept.escrow_program_id` MUST equal this. */
  escrowProgramId?: string;
}
