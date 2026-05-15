/**
 * Valibot schemas for the gateway 402 payload — ai-sdk-provider edition.
 *
 * Mirrors `@solvela/signer-core`'s `schema.ts` but with two intentional
 * differences:
 *
 *   1. `PaymentAcceptSchema` does NOT include `escrow_program_id`.
 *      `SolvelaPaymentAccept` in `wallet-adapter.ts` is the v1 contract
 *      this package commits to, and v1 is exact-scheme-only. The escrow
 *      field would survive valibot's default object() (which strips
 *      unknown keys) but the public type doesn't expose it.
 *
 *   2. Errors are wrapped in `SolvelaPaymentError` instead of plain
 *      `Error`, preserving the existing contract for downstream callers
 *      that catch and inspect this error type.
 *
 * valibot's default `object()` silently strips unknown keys — this gives
 * the allowlist behavior the previous hand-rolled `applyAcceptAllowlist`
 * /`applyCostBreakdownAllowlist`/`applyResourceAllowlist` functions used
 * to enforce explicitly, so unknown fields like `internal_trace_id` /
 * `debug_hash` / `trace_id` / `region` never reach the output.
 *
 * See issue #129 (signer-core half landed in PR #304; this is the
 * follow-up for the ai-sdk-provider half).
 */

import {
  array,
  finite,
  minLength,
  minValue,
  nonEmpty,
  number,
  object,
  pipe,
  rawCheck,
  string,
  type BaseSchema,
  type InferOutput,
} from 'valibot';

/**
 * Decimal/atomic amount validator. Same rules as signer-core's:
 *   - non-empty string
 *   - `Number(s)` is finite + non-negative
 *
 * Catches trailing-garbage cases like `"1.5USDC"`, `"0.001SOL"`, plus
 * the empty-string / null / boolean / array coercion holes that the
 * pre-schema validator had to defend against with a `typeof` guard.
 */
const decimalAmount: BaseSchema<unknown, string, any> = pipe(
  string('expected decimal amount string'),
  nonEmpty('decimal amount must be non-empty'),
  rawCheck(({ dataset, addIssue }) => {
    if (dataset.typed) {
      const n = Number(dataset.value);
      if (!Number.isFinite(n) || n < 0) {
        addIssue({
          message: `invalid numeric amount "${dataset.value}" (must parse to a finite non-negative number)`,
        });
      }
    }
  }),
);

export const CostBreakdownSchema = object({
  provider_cost: decimalAmount,
  platform_fee: decimalAmount,
  total: decimalAmount,
  currency: pipe(string(), nonEmpty()),
  fee_percent: pipe(number(), finite(), minValue(0)),
});

export const PaymentAcceptSchema = object({
  scheme: pipe(string(), nonEmpty()),
  network: pipe(string(), nonEmpty()),
  amount: decimalAmount,
  asset: pipe(string(), nonEmpty()),
  pay_to: pipe(string(), nonEmpty()),
  max_timeout_seconds: pipe(number(), finite(), minValue(0)),
});

export const ResourceSchema = object({
  url: string(),
  method: string(),
});

export const PaymentRequiredSchema = object({
  x402_version: pipe(number(), finite(), minValue(0)),
  resource: ResourceSchema,
  accepts: pipe(array(PaymentAcceptSchema), minLength(1, 'accepts must be non-empty')),
  cost_breakdown: CostBreakdownSchema,
  error: string(),
});

/**
 * The 402 envelope shape: `{ error: { type: "invalid_payment", message: "<JSON>" } }`.
 * Outer-structure-only — the inner `message` string is JSON-parsed by
 * the parser and re-validated against `PaymentRequiredSchema`.
 */
export const Envelope402Schema = object({
  error: object({
    type: pipe(string(), nonEmpty()),
    message: string(),
  }),
});

// ---------------------------------------------------------------------------
// Schema-derived types (internal). The public-facing `ParsedPaymentRequired`
// in `parse-402.ts` still aliases `SolvelaPaymentRequired` from
// `wallet-adapter.ts` — the schema output is structurally compatible.
// ---------------------------------------------------------------------------
export type ParsedAccept = InferOutput<typeof PaymentAcceptSchema>;
export type ParsedCostBreakdown = InferOutput<typeof CostBreakdownSchema>;
export type ParsedResource = InferOutput<typeof ResourceSchema>;
export type ParsedPayment = InferOutput<typeof PaymentRequiredSchema>;
