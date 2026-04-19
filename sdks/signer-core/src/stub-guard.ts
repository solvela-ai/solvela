/**
 * Stub payment header guard.
 *
 * Detects stub payment headers produced by the SDK when @solana/web3.js
 * is unresolvable at runtime (no real private key signing occurred).
 * Stub headers contain transactions starting with 'STUB_'.
 */

/**
 * Check whether a base64-encoded payment header is a stub transaction.
 *
 * Decodes the base64 string, parses as JSON, and checks whether
 * payload.transaction or payload.deposit_tx starts with 'STUB_'.
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
    if (typeof tx === 'string' && tx.startsWith('STUB_')) return true;
    if (typeof depositTx === 'string' && depositTx.startsWith('STUB_')) return true;
    return false;
  } catch {
    // Decode or parse failure — not a stub
    return false;
  }
}
