/**
 * Strict parser for USDC amount strings supplied by MCP tool callers.
 *
 * Accepts only plain decimal strings — digits with an optional fractional
 * part. Rejects:
 *   - scientific notation ("1e10")
 *   - trailing/leading garbage ("5.00abc", " 5")
 *   - negatives ("-1.5")
 *   - zero ("0", "0.0")
 *   - the "Infinity"/"NaN" literals (rejected by the regex, never reach Number())
 *
 * Throws `McpError(InvalidParams, ...)` on any rejection so the MCP host
 * surfaces a clean JSON-RPC error.
 *
 * Extracted from the inline check inside the `deposit_escrow` tool handler
 * so it can be imported and unit-tested directly. The previous shape — a
 * verbatim copy of the regex inside the test file — meant a change to the
 * production regex would not have failed the tests.
 */

import { ErrorCode, McpError } from '@modelcontextprotocol/sdk/types.js';

export function parseStrictUsdc(raw: string, fieldName = 'amount_usdc'): number {
  if (!/^\d+(\.\d+)?$/.test(raw)) {
    throw new McpError(
      ErrorCode.InvalidParams,
      `${fieldName} must be a positive decimal number (e.g. "5.00"), got: ${JSON.stringify(raw)}`,
    );
  }
  const n = Number(raw);
  if (!Number.isFinite(n) || n <= 0) {
    throw new McpError(
      ErrorCode.InvalidParams,
      `${fieldName} must be positive, got: ${raw}`,
    );
  }
  return n;
}
