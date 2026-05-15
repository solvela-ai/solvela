/**
 * Valibot schemas for the Solvela x402 wire format.
 *
 * Single source of truth for `PaymentRequired`, `PaymentAccept`, and
 * `CostBreakdown`. The TypeScript types in `types.ts` are derived from
 * these schemas via `v.InferOutput<...>`, so a field added here flows
 * into both the runtime check and the type system without any chance
 * of drift.
 *
 * Replaces the hand-rolled validator that previously lived in
 * `parse-402.ts`. See issue #129 for the rationale.
 *
 * Bundle-size note: this file imports only the named valibot APIs it
 * uses, so the published bundle ships ~1kb of valibot tree-shaken,
 * not the full library.
 */

import {
  array,
  finite,
  minLength,
  minValue,
  nonEmpty,
  number,
  object,
  optional,
  pipe,
  rawCheck,
  string,
  type BaseSchema,
  type InferOutput,
} from 'valibot';

/**
 * Wire format for USDC amounts (and other monetary fields): decimal
 * string. Stored as a string to avoid float-precision drift on the
 * Solana side where atomic units are u64.
 *
 * Validation rules:
 *   - Must be a non-empty string.
 *   - `Number(s)` must parse to a finite non-negative number — this
 *     rules out trailing garbage like `"1.5USDC"`, `"0.001SOL"`,
 *     `"0.5 USDC"` (Number rejects, parseFloat would silently strip),
 *     and also `"NaN"`, `"Infinity"`, `"-1"`.
 *
 * The hand-rolled validator had to use `typeof === 'string'` AND
 * `length > 0` to defend against `Number("") === 0`, `Number(null) === 0`,
 * `Number([]) === 0`, `Number(true) === 1`. valibot's `string()` step
 * means those non-string inputs never reach the numeric check.
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
  escrow_program_id: optional(string()),
});

export const ResourceSchema = object({
  url: string(),
  method: string(),
});

export const PaymentRequiredSchema = object({
  x402_version: pipe(number(), finite(), minValue(0)),
  accepts: pipe(array(PaymentAcceptSchema), minLength(1, 'accepts must be non-empty')),
  cost_breakdown: CostBreakdownSchema,
  error: string(),
  resource: optional(ResourceSchema),
});

/**
 * The 402 envelope shape: `{ error: { message: "<JSON PaymentRequired>" } }`.
 *
 * Only validates the outer envelope structure; the inner `message`
 * string is JSON-parsed and re-validated against `PaymentRequiredSchema`
 * by `parse402`.
 */
export const Envelope402Schema = object({
  error: object({
    message: string('error.message must be a string'),
  }),
});

// ---------------------------------------------------------------------------
// Derived TypeScript types
// ---------------------------------------------------------------------------
//
// Types are exported from this file rather than `types.ts` so adding
// a field in the schema is the *only* edit needed — the type updates
// automatically. `types.ts` now re-exports these to preserve the
// existing public surface (`@solvela/signer-core` consumers import
// types from the package root, not from a deep path).

export type CostBreakdown = InferOutput<typeof CostBreakdownSchema>;
export type PaymentAccept = InferOutput<typeof PaymentAcceptSchema>;
export type PaymentRequired = InferOutput<typeof PaymentRequiredSchema>;
