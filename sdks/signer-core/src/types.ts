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
