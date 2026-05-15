/**
 * 402 gateway parser and v1 scheme selector.
 *
 * Accepts BOTH 402 body shapes the gateway may emit:
 *
 *   1. Envelope (legacy, current default):
 *      { "error": { "type": "invalid_payment",
 *                   "message": "<JSON PaymentRequired>" } }
 *
 *   2. Direct (x402-spec compliant; target shape after issue #217):
 *      { "x402_version": 2, "resource": {...}, "accepts": [...],
 *        "cost_breakdown": {...}, "error": "Payment required" }
 *
 * Shape detection is duck-typed: a top-level `error.message` string triggers
 * the envelope path; otherwise we expect `x402_version` + `accepts` at the
 * top level. Both shapes flow through the same allowlist + validation.
 *
 * This dual-shape support is deliberately defensive — issue #217 (gateway
 * still emits envelope today, but plans to switch to direct). Landing this
 * parser change first lets the gateway flip unilaterally without a
 * coordinated SDK release.
 *
 * Allowlist (§4.3 T2-G) is applied here for the wire payload: any field on
 * the top-level `PaymentRequired` other than `x402_version`, `accepts[]`,
 * `cost_breakdown`, `resource`, `error` is stripped; within each `accepts[]`
 * entry only the plan-allowlisted fields survive. Non-allowlisted keys such
 * as `internal_trace_id` never reach the returned object and therefore
 * cannot leak into `SolvelaPaymentError.responseBody` downstream.
 *
 * IMPORTANT: the returned `ParsedPaymentRequired` remains structurally
 * compatible with the already-committed `SolvelaPaymentRequired` from
 * `wallet-adapter.ts` (the contract the adapter consumes). The allowlist
 * only drops unknown/extra fields; every plan-level known field is
 * preserved.
 */

import { SolvelaPaymentError } from '../errors.js';
import type {
  SolvelaPaymentAccept,
  SolvelaPaymentCostBreakdown,
  SolvelaPaymentRequired,
} from '../wallet-adapter.js';

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
// Type guards and primitive extractors
// ---------------------------------------------------------------------------

function isObject(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v);
}

function isStringField(obj: Record<string, unknown>, key: string): boolean {
  return typeof obj[key] === 'string';
}

function isNumberField(obj: Record<string, unknown>, key: string): boolean {
  return typeof obj[key] === 'number' && Number.isFinite(obj[key] as number);
}

// ---------------------------------------------------------------------------
// Allowlist application
// ---------------------------------------------------------------------------

/**
 * Extract an allowlisted `SolvelaPaymentAccept` from an unknown object.
 * Drops every field not in the plan-level allowlist (T2-G).
 * Throws if required fields are missing or mistyped.
 */
function applyAcceptAllowlist(
  raw: unknown,
  index: number,
  url: string,
): SolvelaPaymentAccept {
  if (!isObject(raw)) {
    throw new SolvelaPaymentError({
      message: `[solvela] 402 envelope: accepts[${index}] is not an object`,
      url,
      requestBodyValues: undefined,
    });
  }

  const required: Array<[string, 'string' | 'number']> = [
    ['scheme', 'string'],
    ['network', 'string'],
    ['amount', 'string'],
    ['asset', 'string'],
    ['pay_to', 'string'],
    ['max_timeout_seconds', 'number'],
  ];

  for (const [key, kind] of required) {
    const present =
      kind === 'string' ? isStringField(raw, key) : isNumberField(raw, key);
    if (!present) {
      throw new SolvelaPaymentError({
        message: `[solvela] 402 envelope: accepts[${index}].${key} missing or wrong type`,
        url,
        requestBodyValues: undefined,
      });
    }
  }

  // Construct the allowlisted entry (any extra fields are dropped by this
  // explicit field-by-field copy).
  return {
    scheme: raw['scheme'] as string,
    network: raw['network'] as string,
    amount: raw['amount'] as string,
    asset: raw['asset'] as string,
    pay_to: raw['pay_to'] as string,
    max_timeout_seconds: raw['max_timeout_seconds'] as number,
  };
}

/**
 * Extract an allowlisted `SolvelaPaymentCostBreakdown`.
 * Every field mirrors `wallet-adapter.ts`'s declared shape.
 */
function applyCostBreakdownAllowlist(
  raw: unknown,
  url: string,
): SolvelaPaymentCostBreakdown {
  if (!isObject(raw)) {
    throw new SolvelaPaymentError({
      message: '[solvela] 402 envelope: cost_breakdown is not an object',
      url,
      requestBodyValues: undefined,
    });
  }

  const strings = ['provider_cost', 'platform_fee', 'total', 'currency'];
  for (const key of strings) {
    if (!isStringField(raw, key)) {
      throw new SolvelaPaymentError({
        message: `[solvela] 402 envelope: cost_breakdown.${key} missing or wrong type`,
        url,
        requestBodyValues: undefined,
      });
    }
  }
  if (!isNumberField(raw, 'fee_percent')) {
    throw new SolvelaPaymentError({
      message: '[solvela] 402 envelope: cost_breakdown.fee_percent missing or wrong type',
      url,
      requestBodyValues: undefined,
    });
  }

  return {
    provider_cost: raw['provider_cost'] as string,
    platform_fee: raw['platform_fee'] as string,
    total: raw['total'] as string,
    currency: raw['currency'] as string,
    fee_percent: raw['fee_percent'] as number,
  };
}

/**
 * Extract an allowlisted `resource` object: `{ url: string, method: string }`.
 */
function applyResourceAllowlist(
  raw: unknown,
  url: string,
): { url: string; method: string } {
  if (!isObject(raw)) {
    throw new SolvelaPaymentError({
      message: '[solvela] 402 envelope: resource is not an object',
      url,
      requestBodyValues: undefined,
    });
  }
  if (!isStringField(raw, 'url') || !isStringField(raw, 'method')) {
    throw new SolvelaPaymentError({
      message: '[solvela] 402 envelope: resource.url/method missing or wrong type',
      url,
      requestBodyValues: undefined,
    });
  }
  return { url: raw['url'] as string, method: raw['method'] as string };
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
  // top-level `error.message` string. The direct shape has `x402_version`
  // and `accepts` at the top level. We probe envelope first since it's
  // the current default and a direct-shape body would never coincidentally
  // carry an `error.message` string at the top level (the direct shape's
  // `error` field is a plain string like "Payment required").
  const errorField = body['error'];
  const looksLikeEnvelope =
    isObject(errorField) && typeof errorField['message'] === 'string';

  const inner = looksLikeEnvelope
    ? extractFromEnvelope(errorField as Record<string, unknown>, url)
    : extractFromDirect(body, url);

  // Required top-level fields on the extracted payload (both shapes).
  if (!isNumberField(inner, 'x402_version')) {
    throw new SolvelaPaymentError({
      message: '[solvela] 402 body: `x402_version` missing or wrong type',
      url,
      requestBodyValues: undefined,
    });
  }
  if (!Array.isArray(inner['accepts'])) {
    throw new SolvelaPaymentError({
      message: '[solvela] 402 body: `accepts` is not an array',
      url,
      requestBodyValues: undefined,
    });
  }
  if (!isStringField(inner, 'error')) {
    throw new SolvelaPaymentError({
      message: '[solvela] 402 body: `error` missing or wrong type',
      url,
      requestBodyValues: undefined,
    });
  }

  const accepts = (inner['accepts'] as unknown[]).map((entry, i) =>
    applyAcceptAllowlist(entry, i, url),
  );
  const cost_breakdown = applyCostBreakdownAllowlist(inner['cost_breakdown'], url);
  const resource = applyResourceAllowlist(inner['resource'], url);

  return {
    x402_version: inner['x402_version'] as number,
    resource,
    accepts,
    cost_breakdown,
    error: inner['error'] as string,
  };
}

/**
 * Extract the inner PaymentRequired from the envelope shape:
 *   { error: { type: "invalid_payment", message: "<JSON>" } }
 */
function extractFromEnvelope(
  errorField: Record<string, unknown>,
  url: string,
): Record<string, unknown> {
  const type = errorField['type'];
  const messageJson = errorField['message'];
  if (typeof type !== 'string' || typeof messageJson !== 'string') {
    throw new SolvelaPaymentError({
      message: '[solvela] 402 envelope: `error.type` or `error.message` missing or wrong type',
      url,
      requestBodyValues: undefined,
    });
  }
  if (type !== 'invalid_payment') {
    throw new SolvelaPaymentError({
      message: `[solvela] 402 envelope: unsupported error.type "${type}"; expected "invalid_payment"`,
      url,
      requestBodyValues: undefined,
    });
  }

  let inner: unknown;
  try {
    inner = JSON.parse(messageJson);
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

  return inner;
}

/**
 * Direct-shape probe: the body itself is the PaymentRequired payload
 * (x402-spec compliant). We don't validate field contents here — the
 * common code path that follows runs the same allowlist + type checks
 * the envelope shape gets.
 */
function extractFromDirect(
  body: Record<string, unknown>,
  url: string,
): Record<string, unknown> {
  // Direct shape requires at minimum `x402_version` (number) and `accepts`
  // (array) at the top level — otherwise we don't recognize the shape and
  // emit a single clear error pointing at both supported shapes so callers
  // can diagnose pre-vs-post #217 gateway responses.
  if (!isNumberField(body, 'x402_version') || !Array.isArray(body['accepts'])) {
    throw new SolvelaPaymentError({
      message:
        '[solvela] 402 body matches neither envelope shape (`error.message` with stringified JSON) nor direct PaymentRequired shape (top-level `x402_version` + `accepts`)',
      url,
      requestBodyValues: undefined,
    });
  }
  return body;
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
 * @returns The matching entry plus its `amount` as a `bigint`.
 * @throws SolvelaPaymentError if no entry matches.
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
