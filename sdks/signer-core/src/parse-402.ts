/**
 * Unified 402 gateway envelope parser.
 *
 * Supports two input shapes:
 *   1. Envelope: { error: { message: "<JSON PaymentRequired>" } } — legacy/Phase1
 *   2. Direct:   { x402_version, accepts: [...], cost_breakdown: {...} }
 *
 * Validation is delegated to the valibot schema in `schema.ts` — that file
 * is the single source of truth for the wire format. See issue #129 for
 * the rationale (replaced hand-rolled field-by-field checks).
 *
 * Throws plain `Error` with a message naming the failing JSON path so
 * downstream `err.message` propagation (e.g. mcp stderr) stays readable.
 *
 * Note: The AI SDK provider has its own parseGateway402 in
 * `sdks/ai-sdk-provider/src/util/parse-402.ts` with stricter allowlist
 * semantics (drops unknown fields) — that file is intentionally not
 * sharing this schema yet; #129 leaves cross-SDK consolidation as a
 * follow-up.
 */

import { safeParse, type BaseIssue } from 'valibot';

import {
  Envelope402Schema,
  PaymentRequiredSchema,
  X402_VERSION_CLIENT,
  type PaymentRequired,
} from './schema.js';

/**
 * Parse a raw JSON string from a 402 response into a PaymentRequired object.
 *
 * Accepts both the envelope shape `{ error: { message: "<json>" } }` and
 * the direct shape `{ x402_version, accepts, cost_breakdown }`.
 *
 * @param raw - The raw response body text from a 402 response.
 * @returns Parsed PaymentRequired.
 * @throws Error with context on: non-JSON body, invalid inner JSON, or
 *   any schema violation (missing/wrong-typed field, malformed amount,
 *   empty accepts array, etc.) — the message names the failing path.
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

  if (body === null || typeof body !== 'object' || Array.isArray(body)) {
    throw new Error(
      `Gateway 402 body is not a JSON object (first 200 chars): ${raw.slice(0, 200)}`,
    );
  }

  // Shape 1: envelope { error: { message: "<JSON>" } }
  // Probe with the envelope schema first; if it matches, parse the inner
  // string and re-validate against PaymentRequiredSchema. If the envelope
  // probe fails, fall through to the direct shape.
  const envelopeProbe = safeParse(Envelope402Schema, body);
  if (envelopeProbe.success) {
    const innerRaw = envelopeProbe.output.error.message;
    let inner: unknown;
    try {
      inner = JSON.parse(innerRaw);
    } catch {
      throw new Error(
        `Gateway 402 error.message is not valid JSON (first 200 chars): ${innerRaw.slice(0, 200)}`,
      );
    }
    return validateOrThrow(inner, '(parsed from error.message)');
  }

  // Shape 2: direct PaymentRequired at top level.
  return validateOrThrow(body, '(direct shape)');
}

function validateOrThrow(value: unknown, context: string): PaymentRequired {
  const result = safeParse(PaymentRequiredSchema, value);
  if (result.success) {
    warnIfDowngrade(result.output.x402_version);
    return result.output;
  }
  throw new Error(
    `Gateway 402 body failed validation ${context}: ${formatIssues(result.issues)}`,
  );
}

/**
 * Emit a single `console.warn` per unique downgraded version observed,
 * so operators can alert on protocol regression without log-flooding
 * a tight request loop. The first occurrence of every distinct
 * downgraded version warns; subsequent identical downgrades stay
 * silent until the process restarts.
 *
 * Bumps to X402_VERSION_CLIENT in this codebase reset the dedup set
 * implicitly (a previously-warned version may stop being a downgrade).
 */
const _warnedVersions = new Set<number>();
function warnIfDowngrade(gatewayVersion: number): void {
  if (gatewayVersion >= X402_VERSION_CLIENT) return;
  if (_warnedVersions.has(gatewayVersion)) return;
  _warnedVersions.add(gatewayVersion);
  // eslint-disable-next-line no-console
  console.warn(
    `[solvela] x402 protocol downgrade detected: gateway responded with ` +
      `x402_version=${gatewayVersion}, client speaks ${X402_VERSION_CLIENT}. ` +
      `Continuing for compatibility, but check that the gateway is up to date.`,
  );
}

/**
 * Test-only: clear the per-process downgrade-warn dedup set so each test
 * can assert console.warn was called. Not intended for production code.
 *
 * @internal
 */
export function _resetDowngradeWarnDedup(): void {
  _warnedVersions.clear();
}

/**
 * Render valibot's issue list into a single human-readable string. We
 * format each issue as `path: message` (e.g.
 * `accepts[1].scheme: Invalid type: Expected string but received null`)
 * so the failing field stays visible no matter how deeply nested.
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
