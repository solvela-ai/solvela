/**
 * Shared x402 protocol types used across Solvela SDK packages.
 *
 * These are structurally compatible with @solvela/sdk's PaymentRequired,
 * PaymentAccept, and CostBreakdown types — the same wire shape.
 */

export interface CostBreakdown {
  provider_cost: string;
  platform_fee: string;
  total: string;
  currency: string;
  fee_percent: number;
}

export interface PaymentAccept {
  scheme: string;
  network: string;
  amount: string;
  asset: string;
  pay_to: string;
  max_timeout_seconds: number;
  escrow_program_id?: string;
}

export interface PaymentRequired {
  x402_version: number;
  accepts: PaymentAccept[];
  cost_breakdown: CostBreakdown;
  error: string;
  /** Optional resource metadata — present in gateway envelope shape. */
  resource?: { url: string; method: string };
}

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
