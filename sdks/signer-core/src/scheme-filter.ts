/**
 * Payment scheme filtering for x402 payment accepts.
 *
 * Filters the accepts array based on the requested signing mode.
 * Throws with an actionable message if the filter produces an empty array.
 */

import type { PaymentAccept } from './types.js';

/**
 * Filter the accepts array by signing mode.
 *
 * - 'auto':   return accepts unchanged (SDK's createPaymentHeader prefers escrow)
 * - 'escrow': filter to only scheme === 'escrow'
 * - 'direct': filter to only scheme === 'exact' (future-proof against new non-escrow schemes)
 *
 * @param accepts - The accepts array from a PaymentRequired payload.
 * @param mode - The signing mode to filter by.
 * @returns Filtered accepts array.
 * @throws Error with actionable message when the filter produces an empty array.
 */
export function filterAccepts(
  accepts: PaymentAccept[],
  mode: 'auto' | 'escrow' | 'direct',
): PaymentAccept[] {
  if (mode === 'auto') return accepts;

  const filtered = accepts.filter((a) => {
    if (mode === 'escrow') return a.scheme === 'escrow';
    if (mode === 'direct') return a.scheme === 'exact';
    return true;
  });

  if (filtered.length === 0) {
    throw new Error(
      `No payment accepts match signing mode '${mode}'. ` +
        'Gateway offered: ' + accepts.map((a) => a.scheme).join(', '),
    );
  }

  return filtered;
}
