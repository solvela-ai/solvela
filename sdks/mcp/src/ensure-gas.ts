/**
 * Best-effort gas top-up for the Solvela MCP server.
 *
 * Before the first signing attempt (when `signingMode !== 'off'`), the agent
 * needs a little SOL to pay network gas as the on-chain fee payer. The gateway
 * runs a gas-drip faucet that funds USDC-holding wallets with a dust of SOL so
 * the user only ever has to fund USDC. This module asks for that drip once on
 * startup.
 *
 * SECURITY / SAFETY:
 *  - Best-effort ONLY. Any failure (faucet disabled, unreachable, declined, RPC
 *    error, timeout) is logged to stderr and swallowed — it NEVER blocks
 *    startup. Without the drip the agent simply gets the existing
 *    insufficient-SOL behaviour on its first signed call.
 *  - No private key ever leaves the host. We POST only the public address and
 *    read only the public SOL balance. The faucet builds + signs the SOL
 *    transfer server-side from the gateway's own gas wallet.
 *  - stdout is reserved for the stdio transport; all logging goes to stderr.
 */

import { Connection, PublicKey, LAMPORTS_PER_SOL } from '@solana/web3.js';

/** Below this SOL balance (lamports) we ask the faucet for a drip. */
const GAS_THRESHOLD_LAMPORTS = 3_000_000; // 0.003 SOL — matches the gateway low-water default

/** How long to poll for the drip tx to confirm before giving up (ms). */
const CONFIRM_TIMEOUT_MS = 8_000;
/** Poll interval while waiting for confirmation (ms). */
const CONFIRM_POLL_MS = 1_000;

export interface EnsureGasOptions {
  /** Base58 wallet address to (maybe) fund. */
  address: string;
  /** Gateway base URL (e.g. https://api.solvela.ai). */
  gatewayUrl: string;
  /** Solana RPC URL for the balance read + confirmation poll. */
  rpcUrl: string;
  /** Optional injected fetch (tests). Defaults to global fetch. */
  fetchImpl?: typeof fetch;
  /** Optional injected Connection (tests). Defaults to a real Connection. */
  connection?: Pick<Connection, 'getBalance' | 'getSignatureStatus'>;
  /** Override the low-balance threshold (lamports). */
  thresholdLamports?: number;
}

/** Outcome of an ensureGas attempt — returned for tests; ignored in prod. */
export type EnsureGasResult =
  | { action: 'skipped'; reason: 'sufficient_balance' }
  | { action: 'requested'; funded: boolean; reason?: string; txSignature?: string }
  | { action: 'error'; reason: string };

/**
 * Ensure the wallet has a little SOL for gas. Best-effort: returns a result for
 * test assertions, but production callers should ignore it and never throw.
 */
export async function ensureGas(opts: EnsureGasOptions): Promise<EnsureGasResult> {
  const fetchImpl = opts.fetchImpl ?? fetch;
  const threshold = opts.thresholdLamports ?? GAS_THRESHOLD_LAMPORTS;

  const connection =
    opts.connection ?? new Connection(opts.rpcUrl, 'confirmed');

  // 1. Read current SOL balance. If we already have enough, do nothing.
  let balance: number;
  try {
    balance = await connection.getBalance(new PublicKey(opts.address), 'confirmed');
  } catch (err) {
    const reason = err instanceof Error ? err.message : String(err);
    process.stderr.write(`[solvela-mcp] gas: balance read failed (${reason}); skipping faucet.\n`);
    return { action: 'error', reason };
  }

  if (balance >= threshold) {
    return { action: 'skipped', reason: 'sufficient_balance' };
  }

  // 2. Ask the faucet for a drip.
  let funded = false;
  let txSignature: string | undefined;
  let declineReason: string | undefined;
  try {
    const url = `${opts.gatewayUrl.replace(/\/+$/, '')}/v1/faucet/gas`;
    const resp = await fetchImpl(url, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ wallet: opts.address }),
    });
    // Treat ANY response as untrusted external data — parse defensively.
    const json = (await resp.json().catch(() => ({}))) as {
      funded?: boolean;
      reason?: string;
      tx_signature?: string;
    };
    funded = json.funded === true;
    txSignature = typeof json.tx_signature === 'string' ? json.tx_signature : undefined;
    declineReason = typeof json.reason === 'string' ? json.reason : undefined;
  } catch (err) {
    const reason = err instanceof Error ? err.message : String(err);
    process.stderr.write(
      `[solvela-mcp] gas: faucet request failed (${reason}); continuing without a drip.\n`,
    );
    return { action: 'error', reason };
  }

  if (!funded) {
    process.stderr.write(
      `[solvela-mcp] gas: faucet did not fund (${declineReason ?? 'declined'}); ` +
        `continuing. If signing fails for lack of SOL, fund a little SOL manually.\n`,
    );
    return { action: 'requested', funded: false, reason: declineReason };
  }

  // 3. Poll for confirmation (bounded). A timeout is NOT an error — the drip is
  // in flight; we just proceed and let the first signed call benefit from it.
  if (txSignature) {
    const deadline = Date.now() + CONFIRM_TIMEOUT_MS;
    while (Date.now() < deadline) {
      try {
        const st = await connection.getSignatureStatus(txSignature);
        const status = st?.value?.confirmationStatus;
        if (status === 'confirmed' || status === 'finalized') {
          process.stderr.write(
            `[solvela-mcp] gas: faucet dripped ~${(GAS_THRESHOLD_LAMPORTS / LAMPORTS_PER_SOL).toFixed(3)} SOL ` +
              `(tx ${txSignature}, ${status}).\n`,
          );
          break;
        }
      } catch {
        // transient — keep polling
      }
      await sleep(CONFIRM_POLL_MS);
    }
  }

  return { action: 'requested', funded: true, txSignature };
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
