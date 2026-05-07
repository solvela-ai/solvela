/**
 * Unified 402 gateway envelope parser.
 *
 * Supports two input shapes:
 *   1. Envelope: { error: { message: "<JSON PaymentRequired>" } } — legacy/Phase1
 *   2. Direct:   { x402_version, accepts: [...], cost_breakdown: {...} }
 *
 * Throws with contextual messages on malformed input. Never returns null.
 *
 * Note: The AI SDK provider has its own stricter parseGateway402 that
 * requires the envelope shape and throws SolvelaPaymentError. This parser
 * is for MCP server and OpenClaw provider which accept both shapes and
 * throw plain Error.
 */

import type { PaymentRequired } from './types.js';

/**
 * Parse a raw JSON string from a 402 response into a PaymentRequired object.
 *
 * Accepts both the envelope shape `{ error: { message: "<json>" } }` and
 * the direct shape `{ x402_version, accepts, cost_breakdown }`.
 *
 * @param raw - The raw response body text from a 402 response.
 * @returns Parsed PaymentRequired.
 * @throws Error with context on: non-JSON, invalid inner JSON, missing
 *   x402_version, missing/empty accepts, non-finite cost_breakdown.total.
 */
export function parse402(raw: string): PaymentRequired {
  let body: unknown;
  try {
    body = JSON.parse(raw);
  } catch {
    throw new Error(
      `Gateway returned 402 with non-JSON body (first 200 chars): ${raw.slice(0, 200)}`,
    );
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const typed: any = body;

  // Shape 1: envelope { error: { message: "<JSON string>" } }
  const msg = typed?.error?.message;
  if (typeof msg === 'string') {
    let inner: unknown;
    try {
      inner = JSON.parse(msg);
    } catch {
      throw new Error(
        `Gateway 402 error.message is not valid JSON (first 200 chars): ${msg.slice(0, 200)}`,
      );
    }
    return validatePaymentRequired(inner, `(parsed from error.message)`);
  }

  // Shape 2: direct { x402_version, accepts, cost_breakdown }
  if (typed !== null && typeof typed === 'object' && !Array.isArray(typed)) {
    return validatePaymentRequired(typed, `(direct shape)`);
  }

  throw new Error(
    `Gateway 402 body is not a JSON object (first 200 chars): ${raw.slice(0, 200)}`,
  );
}

function validatePaymentRequired(inner: unknown, context: string): PaymentRequired {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const obj: any = inner;

  if (obj === null || typeof obj !== 'object' || Array.isArray(obj)) {
    throw new Error(`Gateway 402 payload ${context} is not an object`);
  }

  // Accept x402_version: 0 (draft/legacy gateways) as well as any truthy version number.
  // The condition `!obj.x402_version && obj.x402_version !== 0` is intentional:
  //   - rejects missing/null/undefined (falsy, not 0)
  //   - accepts 0  (draft protocol, pre-release gateways)
  //   - accepts any positive integer (current: 2)
  if (!obj.x402_version && obj.x402_version !== 0) {
    throw new Error(
      `Gateway 402 body missing x402_version ${context} (received keys: ${Object.keys(obj).join(',')})`,
    );
  }

  if (!Array.isArray(obj.accepts) || obj.accepts.length === 0) {
    throw new Error(
      `Gateway 402 body missing or empty accepts ${context} (received keys: ${Object.keys(obj).join(',')})`,
    );
  }

  // Validate each accepts[] element has the minimum shape downstream code
  // assumes (an object with a string `scheme`). Without this, accepts: [null]
  // or accepts: [{}] would parse cleanly here and then crash in
  // `scheme-filter.ts` when `a.scheme === 'escrow'` derefs the bad element.
  // A real schema (zod/valibot) would do this and more — tracked as a
  // follow-up; this surgical loop closes the immediate bug class.
  for (let i = 0; i < obj.accepts.length; i++) {
    const a = obj.accepts[i];
    if (a === null || typeof a !== 'object' || Array.isArray(a)) {
      throw new Error(
        `Gateway 402 accepts[${i}] is not a JSON object ${context}: ${JSON.stringify(a)}`,
      );
    }
    if (typeof (a as { scheme?: unknown }).scheme !== 'string') {
      throw new Error(
        `Gateway 402 accepts[${i}] missing or invalid 'scheme' field ${context} (expected string)`,
      );
    }
  }

  // Validate cost_breakdown.total parses to a finite non-negative number.
  //
  // We use Number() (not parseFloat) so trailing garbage like "1.5USDC" or
  // "0.001SOL" is rejected — parseFloat would happily return 1.5 / 0.001 and
  // silently strip the suffix. The typeof guard is needed because Number()
  // coerces several non-string values to 0 (Number("") === Number(null) ===
  // Number([]) === 0, Number(true) === 1), which would otherwise pass the
  // `total < 0` check. The x402 wire format requires `total` to be a string
  // (decimal USDC kept as text to avoid float precision drift), so anything
  // else is malformed by definition.
  const totalRaw = obj.cost_breakdown?.total;
  const total =
    typeof totalRaw === 'string' && totalRaw.length > 0
      ? Number(totalRaw)
      : NaN;
  if (!Number.isFinite(total) || total < 0) {
    throw new Error(
      `Gateway 402 has invalid cost_breakdown.total ${context}: ${totalRaw}`,
    );
  }

  return obj as PaymentRequired;
}
