/**
 * 402 gateway envelope parser and v1 scheme selector.
 *
 * Targets ONLY the gateway envelope emitted by
 * `crates/gateway/src/error.rs:75-80`:
 *
 *   { "error": { "type": "invalid_payment", "message": "<JSON PaymentRequired>" } }
 *
 * The direct `{x402_version, accepts, ...}` shape is NOT supported in v1 (T1-B).
 *
 * Allowlist (§4.3 T2-G) is applied here for the wire envelope: any field on the
 * top-level `PaymentRequired` other than `x402_version`, `accepts[]`,
 * `cost_breakdown`, `resource`, `error` is stripped; within each `accepts[]`
 * entry only the plan-allowlisted fields survive. Non-allowlisted keys such as
 * `internal_trace_id` never reach the returned object and therefore cannot
 * leak into `SolvelaPaymentError.responseBody` downstream.
 *
 * IMPORTANT: the returned `ParsedPaymentRequired` remains structurally
 * compatible with the already-committed `SolvelaPaymentRequired` from
 * `wallet-adapter.ts` (the contract the adapter consumes). The allowlist only
 * drops unknown/extra fields; every plan-level known field is preserved.
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
function applyAcceptAllowlist(raw: unknown, index: number): SolvelaPaymentAccept {
  if (!isObject(raw)) {
    throw new SolvelaPaymentError({
      message: `[solvela] 402 envelope: accepts[${index}] is not an object`,
      url: '',
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
        url: '',
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
function applyCostBreakdownAllowlist(raw: unknown): SolvelaPaymentCostBreakdown {
  if (!isObject(raw)) {
    throw new SolvelaPaymentError({
      message: '[solvela] 402 envelope: cost_breakdown is not an object',
      url: '',
      requestBodyValues: undefined,
    });
  }

  const strings = ['provider_cost', 'platform_fee', 'total', 'currency'];
  for (const key of strings) {
    if (!isStringField(raw, key)) {
      throw new SolvelaPaymentError({
        message: `[solvela] 402 envelope: cost_breakdown.${key} missing or wrong type`,
        url: '',
        requestBodyValues: undefined,
      });
    }
  }
  if (!isNumberField(raw, 'fee_percent')) {
    throw new SolvelaPaymentError({
      message: '[solvela] 402 envelope: cost_breakdown.fee_percent missing or wrong type',
      url: '',
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
function applyResourceAllowlist(raw: unknown): { url: string; method: string } {
  if (!isObject(raw)) {
    throw new SolvelaPaymentError({
      message: '[solvela] 402 envelope: resource is not an object',
      url: '',
      requestBodyValues: undefined,
    });
  }
  if (!isStringField(raw, 'url') || !isStringField(raw, 'method')) {
    throw new SolvelaPaymentError({
      message: '[solvela] 402 envelope: resource.url/method missing or wrong type',
      url: '',
      requestBodyValues: undefined,
    });
  }
  return { url: raw['url'] as string, method: raw['method'] as string };
}

// ---------------------------------------------------------------------------
// parseGateway402
// ---------------------------------------------------------------------------

/**
 * Parse the gateway 402 envelope into a `ParsedPaymentRequired`.
 *
 * Input shape (the ONLY supported shape):
 *   { error: { type: "invalid_payment", message: "<JSON PaymentRequired>" } }
 *
 * @param body - Raw JSON-parsed body from the 402 response.
 * @returns Allowlisted `ParsedPaymentRequired`.
 * @throws SolvelaPaymentError if the envelope is unrecognized or malformed.
 */
export function parseGateway402(body: unknown): ParsedPaymentRequired {
  if (!isObject(body)) {
    throw new SolvelaPaymentError({
      message: '[solvela] 402 envelope: body is not a JSON object',
      url: '',
      requestBodyValues: undefined,
    });
  }

  const errorField = body['error'];
  if (!isObject(errorField)) {
    throw new SolvelaPaymentError({
      message: '[solvela] 402 envelope: missing `error` object',
      url: '',
      requestBodyValues: undefined,
    });
  }

  const type = errorField['type'];
  const messageJson = errorField['message'];
  if (typeof type !== 'string' || typeof messageJson !== 'string') {
    throw new SolvelaPaymentError({
      message: '[solvela] 402 envelope: `error.type` or `error.message` missing or wrong type',
      url: '',
      requestBodyValues: undefined,
    });
  }
  if (type !== 'invalid_payment') {
    throw new SolvelaPaymentError({
      message: `[solvela] 402 envelope: unsupported error.type "${type}"; expected "invalid_payment"`,
      url: '',
      requestBodyValues: undefined,
    });
  }

  // The inner PaymentRequired is a JSON-stringified object.
  let inner: unknown;
  try {
    inner = JSON.parse(messageJson);
  } catch {
    throw new SolvelaPaymentError({
      message: '[solvela] 402 envelope: `error.message` is not valid JSON',
      url: '',
      requestBodyValues: undefined,
    });
  }

  if (!isObject(inner)) {
    throw new SolvelaPaymentError({
      message: '[solvela] 402 envelope: inner PaymentRequired is not an object',
      url: '',
      requestBodyValues: undefined,
    });
  }

  // Required top-level fields.
  if (!isNumberField(inner, 'x402_version')) {
    throw new SolvelaPaymentError({
      message: '[solvela] 402 envelope: `x402_version` missing or wrong type',
      url: '',
      requestBodyValues: undefined,
    });
  }
  if (!Array.isArray(inner['accepts'])) {
    throw new SolvelaPaymentError({
      message: '[solvela] 402 envelope: `accepts` is not an array',
      url: '',
      requestBodyValues: undefined,
    });
  }
  if (!isStringField(inner, 'error')) {
    throw new SolvelaPaymentError({
      message: '[solvela] 402 envelope: inner `error` missing or wrong type',
      url: '',
      requestBodyValues: undefined,
    });
  }

  const accepts = (inner['accepts'] as unknown[]).map((entry, i) =>
    applyAcceptAllowlist(entry, i),
  );
  const cost_breakdown = applyCostBreakdownAllowlist(inner['cost_breakdown']);
  const resource = applyResourceAllowlist(inner['resource']);

  return {
    x402_version: inner['x402_version'] as number,
    resource,
    accepts,
    cost_breakdown,
    error: inner['error'] as string,
  };
}

// ---------------------------------------------------------------------------
// selectAccept
// ---------------------------------------------------------------------------

/**
 * Apply the v1 scheme-selection rule:
 *   first `accepts[]` entry with `scheme === 'exact'` AND `asset === USDC`.
 *
 * @param parsed - Result of `parseGateway402`.
 * @returns The matching entry plus its `amount` as a `bigint`.
 * @throws SolvelaPaymentError if no entry matches.
 */
export function selectAccept(parsed: ParsedPaymentRequired): SelectedAccept {
  for (const accept of parsed.accepts) {
    if (accept.scheme === REQUIRED_SCHEME && accept.asset === USDC_MINT_MAINNET) {
      // Parse amount → bigint. Amount is USDC atomic units as a decimal string.
      let cost: bigint;
      try {
        cost = BigInt(accept.amount);
      } catch {
        throw new SolvelaPaymentError({
          message: `[solvela] 402 envelope: selected accept.amount "${accept.amount}" is not a valid integer`,
          url: '',
          requestBodyValues: undefined,
        });
      }
      if (cost < 0n) {
        throw new SolvelaPaymentError({
          message: '[solvela] 402 envelope: selected accept.amount is negative',
          url: '',
          requestBodyValues: undefined,
        });
      }
      return { accept, cost };
    }
  }

  throw new SolvelaPaymentError({
    message:
      'no supported payment scheme in accepts[]: v1 requires scheme=exact + asset=USDC',
    url: '',
    requestBodyValues: undefined,
  });
}
