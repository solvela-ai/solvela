import { PaymentRequired } from './types';

const X402_VERSION = 2;

/**
 * Creates a base64-encoded PAYMENT-SIGNATURE header value from a 402 response.
 *
 * In a full implementation this would sign a Solana USDC-SPL TransferChecked
 * transaction using the agent's private key. Currently returns a stub payload
 * suitable for protocol-level testing.
 *
 * The header value is: base64(JSON({ x402_version, resource, accepted, payload }))
 */
export function createPaymentHeader(paymentInfo: PaymentRequired, resourceUrl: string): string {
  if (!paymentInfo.accepts || paymentInfo.accepts.length === 0) {
    throw new Error('No payment accept options in 402 response');
  }

  const accept = paymentInfo.accepts[0];

  const payload = {
    x402_version: X402_VERSION,
    resource: { url: resourceUrl, method: 'POST' },
    accepted: accept,
    payload: { transaction: 'STUB_BASE64_TX' },
  };

  const json = JSON.stringify(payload);

  // Use btoa in browser/Node 16+, or Buffer fallback
  if (typeof btoa === 'function') {
    return btoa(json);
  }
  return Buffer.from(json, 'utf-8').toString('base64');
}

/**
 * Decodes a base64-encoded PAYMENT-SIGNATURE header back to its JSON payload.
 * Useful for debugging and testing.
 */
export function decodePaymentHeader(header: string): unknown {
  let json: string;
  if (typeof atob === 'function') {
    json = atob(header);
  } else {
    json = Buffer.from(header, 'base64').toString('utf-8');
  }
  return JSON.parse(json);
}
