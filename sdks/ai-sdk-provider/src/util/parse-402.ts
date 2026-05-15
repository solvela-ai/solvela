/**
 * 402 gateway parser and v1 scheme selector.
 *
 * Accepts BOTH 402 body shapes the gateway may emit:
 *
 *   1. Envelope (legacy, current default):
 *      { "error": { "type": "invalid_payment",
 *                   "message": "<JSON PaymentRequired>" } }
 *
 *   2. Direct (x402-spec compliant; gateway emits this post-#217):
 *      { "x402_version": 2, "resource": {...}, "accepts": [...],
 *        "cost_breakdown": {...}, "error": "Payment required" }
 *
 * Validation is delegated to the valibot schema in `schema.ts` — that
 * file is the single source of truth for the wire format. Issue #129's
 * acceptance criteria are met by this two-PR split:
 *   - PR #304 landed the signer-core half (its own schema, in its own
 *     package, since signer-core has no allowlist requirement).
 *   - This file is the ai-sdk-provider half — same library (valibot),
 *     separate schema because `SolvelaPaymentAccept` in
 *     `wallet-adapter.ts` is the v1 contract this package commits to,
 *     and it differs from signer-core's wire type (no escrow_program_id).
 *
 * Allowlist behavior (§4.3 T2-G): valibot's default `object()` strips
 * unknown keys silently. Fields like `internal_trace_id` (top-level),
 * `debug_hash` (accept), `trace_id` (cost_breakdown), `region`
 * (resource) never appear in the returned object and therefore cannot
 * leak into `SolvelaPaymentError.responseBody` downstream.
 */

import { safeParse, type BaseIssue } from 'valibot';

import { SolvelaPaymentError } from '../errors.js';
import type {
  SolvelaPaymentAccept,
  SolvelaPaymentCostBreakdown,
  SolvelaPaymentRequired,
} from '../wallet-adapter.js';
import {
  Envelope402Schema,
  PaymentRequiredSchema,
} from './schema.js';

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/**
 * The post-allowlist, parsed payment-required payload.
 * Structurally identical to `SolvelaPaymentRequired` so wallet adapters
 * receive exactly the contract declared in `wallet-adapter.ts`.
 */
export type ParsedPaymentRequired = SolvelaPaymentRequired;

/**
 * Output of `selectAccept`: the chosen `accepts[]` entry plus its `amount`
 * parsed into a `bigint` for the budget state machine.
 */
export interface SelectedAccept {
  /** The chosen `accepts[]` entry (post-allowlist). */
  accept: SolvelaPaymentAccept;
  /** Parsed cost in USDC atomic units for budget reservation. */
  cost: bigint;
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/**
 * USDC-SPL mainnet mint address. v1 scope is mainnet-only.
 * Matches the fixture at `tests/fixtures/402-envelope.json`.
 */
export const USDC_MINT_MAINNET =
  'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v';

/** v1 requires `scheme === 'exact'`. */
const REQUIRED_SCHEME = 'exact';

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function isObject(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v);
}

/**
 * Render valibot's issue list as `path: message; …` so the failing
 * field is preserved no matter how deeply nested.
 */
function formatIssues(issues: readonly BaseIssue<unknown>[]): string {
  return issues
    .map((issue) => {
      const path = issuePath(issue);
      return path ? `${path}: ${issue.message}` : issue.message;
    })
    .join('; ');
}

function issuePath(issue: BaseIssue<unknown>): string {
  if (!issue.path || issue.path.length === 0) return '';
  return issue.path
    .map((segment, i) => {
      const key = (segment as { key?: unknown }).key;
      if (typeof key === 'number') return `[${key}]`;
      if (typeof key === 'string') return i === 0 ? key : `.${key}`;
      return '';
    })
    .join('');
}

// ---------------------------------------------------------------------------
// parseGateway402
// ---------------------------------------------------------------------------

/**
 * Parse a gateway 402 body into a `ParsedPaymentRequired`.
 *
 * Accepted input shapes:
 *   1. Envelope: { error: { type: "invalid_payment",
 *                           message: "<JSON PaymentRequired>" } }
 *   2. Direct:   { x402_version, resource, accepts, cost_breakdown, error }
 *
 * @param body - Raw JSON-parsed body from the 402 response.
 * @param url  - The request URL, used to populate `SolvelaPaymentError.url`
 *               on any thrown errors. Defaults to `''` for callers that
 *               don't have a URL (legacy / tests); the fetch-wrapper passes
 *               the resolved URL.
 * @returns Allowlisted `ParsedPaymentRequired`.
 * @throws SolvelaPaymentError if neither shape is recognized or the
 *         extracted payload is malformed.
 */
export function parseGateway402(
  body: unknown,
  url: string = '',
): ParsedPaymentRequired {
  if (!isObject(body)) {
    throw new SolvelaPaymentError({
      message: '[solvela] 402 body: not a JSON object',
      url,
      requestBodyValues: undefined,
    });
  }

  // Duck-type shape detection. The envelope shape is identified by a
  // top-level `error.message` string; otherwise we expect a direct
  // PaymentRequired body. Probe envelope first — it's the historical
  // default, and a direct-shape body would never coincidentally carry
  // an `error.message` string at the top level (the direct shape's
  // `error` field is a plain string like "Payment required").
  const errorField = body['error'];
  const looksLikeEnvelope =
    isObject(errorField) && typeof errorField['message'] === 'string';

  let inner: unknown;
  if (looksLikeEnvelope) {
    const envelopeResult = safeParse(Envelope402Schema, body);
    if (!envelopeResult.success) {
      throw new SolvelaPaymentError({
        message: `[solvela] 402 envelope: ${formatIssues(envelopeResult.issues)}`,
        url,
        requestBodyValues: undefined,
      });
    }
    // Enforce the historical `error.type === "invalid_payment"` constraint.
    const type = envelopeResult.output.error.type;
    if (type !== 'invalid_payment') {
      throw new SolvelaPaymentError({
        message: `[solvela] 402 envelope: unsupported error.type "${type}"; expected "invalid_payment"`,
        url,
        requestBodyValues: undefined,
      });
    }
    try {
      inner = JSON.parse(envelopeResult.output.error.message);
    } catch {
      throw new SolvelaPaymentError({
        message: '[solvela] 402 envelope: `error.message` is not valid JSON',
        url,
        requestBodyValues: undefined,
      });
    }
    if (!isObject(inner)) {
      throw new SolvelaPaymentError({
        message: '[solvela] 402 envelope: inner PaymentRequired is not an object',
        url,
        requestBodyValues: undefined,
      });
    }
  } else {
    inner = body;
  }

  const innerResult = safeParse(PaymentRequiredSchema, inner);
  if (!innerResult.success) {
    // Distinguish the two shapes in the error message so caller debug
    // output points at the right place. For the direct-shape failure we
    // also call out the dual-shape rejection so a body that matches
    // neither shape doesn't read as a partial validation of one.
    const context = looksLikeEnvelope
      ? '402 inner PaymentRequired'
      : '402 body matches neither envelope shape (`error.message` with stringified JSON) nor direct PaymentRequired shape — validation against direct';
    throw new SolvelaPaymentError({
      message: `[solvela] ${context}: ${formatIssues(innerResult.issues)}`,
      url,
      requestBodyValues: undefined,
    });
  }

  // valibot's default object() strips unknown keys, so innerResult.output
  // already contains only the allowlisted fields per the schema. The cast
  // here just narrows InferOutput<PaymentRequiredSchema> to the public
  // SolvelaPaymentRequired alias — they are structurally identical.
  return innerResult.output as ParsedPaymentRequired;
}

// ---------------------------------------------------------------------------
// selectAccept
// ---------------------------------------------------------------------------

/**
 * Apply the v1 scheme-selection rule:
 *   first `accepts[]` entry with `scheme === 'exact'` AND `asset === USDC`.
 *
 * @param parsed - Result of `parseGateway402`.
 * @param url    - Request URL for `SolvelaPaymentError.url` on thrown errors.
 *                 Defaults to `''` for callers without URL context.
 * @returns `{ accept, cost }` where `cost` is the chosen entry's `amount`
 *          parsed as a `bigint` (USDC atomic units).
 * @throws SolvelaPaymentError if no allowlisted entry matches the v1 rule.
 */
export function selectAccept(
  parsed: ParsedPaymentRequired,
  url: string = '',
): SelectedAccept {
  for (const accept of parsed.accepts) {
    if (accept.scheme === REQUIRED_SCHEME && accept.asset === USDC_MINT_MAINNET) {
      // Parse amount → bigint. Amount is USDC atomic units as a decimal string.
      let cost: bigint;
      try {
        cost = BigInt(accept.amount);
      } catch {
        throw new SolvelaPaymentError({
          message: `[solvela] 402 envelope: selected accept.amount "${accept.amount}" is not a valid integer`,
          url,
          requestBodyValues: undefined,
        });
      }
      if (cost < 0n) {
        throw new SolvelaPaymentError({
          message: '[solvela] 402 envelope: selected accept.amount is negative',
          url,
          requestBodyValues: undefined,
        });
      }
      return { accept, cost };
    }
  }

  throw new SolvelaPaymentError({
    message:
      'no supported payment scheme in accepts[]: v1 requires scheme=exact + asset=USDC',
    url,
    requestBodyValues: undefined,
  });
}

// Re-export the cost_breakdown type for downstream callers that previously
// imported it from this module's old hand-rolled shape.
export type ParsedCostBreakdown = SolvelaPaymentCostBreakdown;
