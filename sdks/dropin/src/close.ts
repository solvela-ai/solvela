/**
 * `solvela-dropin close` — cooperative channel close.
 *
 * Signs the canonical close message (`solvela-channel-close-v1 || channel_id`,
 * signer-core's `buildCloseMessage`) with the session key and POSTs
 * `{channel_id, signature}` to `POST /v1/channel/close`. The refund goes to
 * the gateway-stored `agent_wallet` — there is deliberately no destination
 * field. Re-running close is the documented refund-status polling surface
 * (the close is idempotent gateway-side).
 */

import bs58 from 'bs58';

import { buildCloseMessage, signVoucher, sanitizeGatewayError } from '@solvela/signer-core';

import { assertNotPending, loadState, defaultStatePath } from './state.js';

export interface CloseOptions {
  statePath?: string;
  log?: (line: string) => void;
}

export async function closeChannel(opts: CloseOptions = {}): Promise<void> {
  const log = opts.log ?? ((line: string) => console.log(line));
  const statePath = opts.statePath ?? defaultStatePath();
  const state = await loadState(statePath);
  assertNotPending(state, statePath);

  const channelId = bs58.decode(state.channel_id);
  if (channelId.length !== 32) {
    throw new Error(`state file ${statePath} channel_id is not a 32-byte base58 value`);
  }
  const seed = bs58.decode(state.session_seed_b58);
  let signatureB64: string;
  try {
    const message = buildCloseMessage(channelId);
    signatureB64 = Buffer.from(signVoucher(message, seed)).toString('base64');
  } finally {
    seed.fill(0); // our decoded copy only; the state file remains the owner
  }

  let resp: Response;
  try {
    resp = await fetch(`${state.gateway_url}/v1/channel/close`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ channel_id: state.channel_id, signature: signatureB64 }),
      // Short deadline is safe here: close is idempotent gateway-side, so a
      // timed-out close is simply re-run (that is also the polling surface).
      signal: AbortSignal.timeout(10_000),
    });
  } catch {
    throw new Error(
      `could not reach ${state.gateway_url}/v1/channel/close — the close is idempotent; simply re-run it`,
    );
  }
  const text = await resp.text();
  if (!resp.ok) {
    throw new Error(`channel close failed (${resp.status}): ${sanitizeGatewayError(text)}`);
  }
  const body = JSON.parse(text) as Record<string, unknown>;

  log(`Channel ${state.channel_id}:`);
  log(`  status:            ${body['status']}`);
  log(`  refund_status:     ${body['refund_status']}`);
  log(`  deposited_atomic:  ${body['deposited_atomic']}`);
  log(`  refundable_atomic: ${body['refundable_atomic']}`);
  log(`  tx_signature:      ${body['tx_signature'] ?? '(pending — re-run `solvela-dropin close` to poll)'}`);
}
