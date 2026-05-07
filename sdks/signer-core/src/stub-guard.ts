/**
 * Stub payment guards.
 *
 * Detects stub payment headers / transactions produced by the SDK when
 * @solana/web3.js is unresolvable at runtime (no real private key signing
 * occurred). Stub transactions contain markers starting with 'STUB_'.
 *
 * The convention lives here as a single source of truth so callers can
 * check at either layer — the outer envelope (isStubHeader) or an
 * already-extracted inner transaction string (isStubTransaction).
 */

/**
 * Check whether a transaction string is a stub marker.
 *
 * Stub markers begin with 'STUB_' (e.g. 'STUB_BASE64_TX',
 * 'STUB_ESCROW_DEPOSIT_TX'). Real transactions are standard base64 — alphabet
 * is `A-Z a-z 0-9 + / =`, which excludes `_`, so the prefix check has zero
 * false positives. (If the wire ever switches to base64url, revisit.)
 *
 * @param tx - The transaction string from a decoded payment payload's
 *             `transaction` or `deposit_tx` field.
 * @returns true if `tx` is a stub marker, false otherwise.
 */
export function isStubTransaction(tx: string): boolean {
  return tx.startsWith('STUB_');
}

/**
 * Check whether a base64-encoded payment header is a stub transaction.
 *
 * Decodes the base64 string, parses as JSON, and checks whether
 * payload.transaction or payload.deposit_tx is a stub marker via
 * isStubTransaction.
 *
 * Returns false (not a stub) if the header cannot be decoded or parsed —
 * malformed headers are a separate concern handled downstream by the gateway.
 *
 * @param headerBase64 - The base64-encoded PAYMENT-SIGNATURE header value.
 * @returns true if the header contains a stub transaction, false otherwise.
 */
export function isStubHeader(headerBase64: string): boolean {
  try {
    const decoded =
      typeof atob === 'function'
        ? atob(headerBase64)
        : Buffer.from(headerBase64, 'base64').toString('utf-8');
    const parsed = JSON.parse(decoded) as Record<string, unknown>;
    const payload = parsed?.payload as Record<string, unknown> | undefined;
    const tx = payload?.transaction;
    const depositTx = payload?.deposit_tx;
    if (typeof tx === 'string' && isStubTransaction(tx)) return true;
    if (typeof depositTx === 'string' && isStubTransaction(depositTx)) return true;
    return false;
  } catch {
    // Decode or parse failure — not a stub
    return false;
  }
}
