/**
 * @solvela/signer-core
 *
 * Shared x402 parsing, scheme filtering, stub-header guards, and error
 * sanitization. Extracted from the MCP server, OpenClaw provider, and
 * AI SDK provider to eliminate duplication.
 *
 * Public API surface:
 *   - Types: PaymentRequired, PaymentAccept, CostBreakdown
 *   - parse402: accepts both envelope and direct shapes, throws on malformed
 *   - filterAccepts: scheme-based filtering with mode support
 *   - isStubHeader: detects stub payment headers
 *   - sanitizeGatewayError: slices + redacts gateway error bodies
 *   - redactBase58, redactHex: byte-pattern redactors
 */

export type { PaymentRequired, PaymentAccept, CostBreakdown } from './types.js';
export { parse402 } from './parse-402.js';
export { filterAccepts } from './scheme-filter.js';
export { isStubHeader } from './stub-guard.js';
export { sanitizeGatewayError, redactBase58, redactHex } from './redact.js';
