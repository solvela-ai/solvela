/**
 * @solvela/signer-core
 *
 * Shared x402 protocol library: signing, parsing, scheme filtering, stub-
 * header guards, and error sanitization. Used by the MCP server, OpenClaw
 * provider, and AI SDK provider as the single source of truth for x402
 * wire-format compatibility with the production gateway.
 *
 * Public API surface:
 *   - Types: PaymentRequired, PaymentAccept, CostBreakdown
 *   - createPaymentHeader: build a base64 PAYMENT-SIGNATURE header (real or stub)
 *   - decodePaymentHeader: round-trip helper for tests/debug
 *   - SigningError: thrown by createPaymentHeader on signing failure
 *   - parse402: accepts both envelope and direct shapes, throws on malformed
 *   - filterAccepts: scheme-based filtering with mode support
 *   - isStubHeader: detects stub payment headers
 *   - sanitizeGatewayError: slices + redacts gateway error bodies
 *   - redactBase58, redactHex: byte-pattern redactors
 */

export type { PaymentRequired, PaymentAccept, CostBreakdown } from './types.js';
export { createPaymentHeader, decodePaymentHeader, SigningError } from './sign.js';
export { parse402 } from './parse-402.js';
export { filterAccepts } from './scheme-filter.js';
export { isStubHeader } from './stub-guard.js';
export { sanitizeGatewayError, redactBase58, redactHex } from './redact.js';
