/**
 * `solvela-dropin open` — fund and open a spend-down channel.
 *
 * Flow: parse the funding amount (fail-closed integer USDC math) → load the
 * user's Solana keypair → discover recipient wallet + USDC mint from
 * `GET {gateway}/v1/escrow/config` → **verify-what-you-sign**: print the
 * amount + recipient and require explicit confirmation (`--yes` skips the
 * prompt) BEFORE anything is signed → build + sign the USDC `TransferChecked`
 * funding transfer (the exact-scheme artifact `POST /v1/channel/open`
 * verifies + broadcasts) → POST open → write the 0600 state file → print the
 * two env vars for Claude Code.
 *
 * The gateway credits ONLY the on-chain-verified transfer amount
 * (`routes/channel.rs credit_deposit`) — this command asserts nothing.
 *
 * SECRETS: the wallet key is read, used to sign, and its buffer zeroed
 * best-effort; the generated session SEED goes ONLY into the 0600 state file
 * — it is never logged or printed.
 */

import { generateKeyPairSync, randomBytes } from 'node:crypto';
import * as fs from 'node:fs/promises';
import * as readline from 'node:readline/promises';

import {
  Connection,
  Keypair,
  PublicKey,
  TransactionMessage,
  VersionedTransaction,
} from '@solana/web3.js';
import {
  createTransferCheckedInstruction,
  getAssociatedTokenAddress,
} from '@solana/spl-token';
import bs58 from 'bs58';

import { sanitizeGatewayError } from '@solvela/signer-core';

import { saveState, defaultStatePath } from './state.js';

const USDC_DECIMALS = 6;
const U64_MAX = 0xffffffffffffffffn;

/** Canonical mainnet USDC mint — the same client-side pin signer-core's
 * sign.ts carries. The gateway-reported mint must match this (or an explicit
 * `--expected-mint`) before anything is signed. */
const USDC_MINT_MAINNET = 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v';

/** Deadline for the config fetch (fast metadata read). */
const CONFIG_TIMEOUT_MS = 10_000;
/** Deadline for POST /v1/channel/open. Deliberately LONG: the gateway
 * broadcasts + confirms the funding transfer on-chain before responding, and
 * abandoning the request after the broadcast would lose the channel_id for a
 * deposit that still lands. */
const OPEN_TIMEOUT_MS = 120_000;

/**
 * Parse a human-entered USDC decimal into atomic units (6 decimals) with
 * INTEGER math only — no float ever touches the amount (solvela-fintech §1).
 *
 * Fail-closed: rejects empty/non-numeric/negative/zero, more than 6 decimal
 * places (rejecting beats silently truncating what the user typed), and
 * anything over u64. Never returns 0.
 */
export function usdcDecimalToAtomic(amount: string): bigint {
  const strict = /^(\d+)(?:\.(\d{1,6}))?$/.exec(amount.trim());
  if (!strict) {
    throw new Error(
      `amount must be a positive USDC decimal with at most ${USDC_DECIMALS} decimal places (e.g. "5" or "2.50"), got: ${JSON.stringify(amount)}`,
    );
  }
  const whole = BigInt(strict[1]);
  const frac = BigInt((strict[2] ?? '').padEnd(USDC_DECIMALS, '0') || '0');
  const atomic = whole * 1_000_000n + frac;
  if (atomic <= 0n) {
    throw new Error('amount must be positive — a zero deposit cannot fund a channel');
  }
  if (atomic > U64_MAX) {
    throw new Error('amount exceeds the u64 atomic range');
  }
  return atomic;
}

/**
 * Load a Solana keypair file. Accepts the standard CLI `id.json` (JSON array
 * of 64 bytes) or the Solvela `wallet.json` shape (`{"private_key": "<base58
 * 64-byte>"}` — shared with the MCP server / Rust CLI). Fails closed; error
 * messages never echo key material.
 */
export async function loadKeypair(walletPath: string): Promise<Keypair> {
  let raw: string;
  try {
    raw = await fs.readFile(walletPath, 'utf-8');
  } catch {
    throw new Error(`could not read wallet file at ${walletPath}`);
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    throw new Error(`wallet file ${walletPath} is not valid JSON`);
  }
  let secret: Uint8Array;
  if (Array.isArray(parsed)) {
    if (parsed.length !== 64 || !parsed.every((n) => Number.isInteger(n) && n >= 0 && n <= 255)) {
      throw new Error(`wallet file ${walletPath} must be a JSON array of exactly 64 bytes`);
    }
    secret = Uint8Array.from(parsed as number[]);
  } else if (
    typeof parsed === 'object' &&
    parsed !== null &&
    typeof (parsed as Record<string, unknown>)['private_key'] === 'string'
  ) {
    try {
      secret = bs58.decode((parsed as Record<string, string>)['private_key']);
    } catch {
      throw new Error(`wallet file ${walletPath} private_key is not valid base58`);
    }
    if (secret.length !== 64) {
      throw new Error(`wallet file ${walletPath} private_key must decode to exactly 64 bytes`);
    }
  } else {
    throw new Error(
      `wallet file ${walletPath} is neither a Solana id.json (64-byte array) nor a Solvela wallet.json ({"private_key": ...})`,
    );
  }
  try {
    // fromSecretKey validates the seed→pubkey relationship (rejects tampering).
    // BLOCKER regression guard (round-1 review): fromSecretKey ALIASES its
    // input array — it does NOT copy. Hand it a defensive copy so zeroing OUR
    // buffer below cannot wipe the live key the funding tx is later signed
    // with (a zeroed key still "signs" without throwing; the signature just
    // fails verification on-chain).
    return Keypair.fromSecretKey(new Uint8Array(secret));
  } catch {
    throw new Error(`wallet file ${walletPath} does not contain a valid ed25519 secret key`);
  } finally {
    // Best-effort: wipe the decoded buffer; the Keypair's copy stays live.
    secret.fill(0);
  }
}

/** Fresh delegated session keypair (node:crypto ed25519 — no new dependency).
 * Returns the raw 32-byte seed and the base58 pubkey the gateway registers. */
function generateSessionKey(): { seed: Uint8Array; pubkeyB58: string } {
  const { publicKey, privateKey } = generateKeyPairSync('ed25519');
  const spki = publicKey.export({ type: 'spki', format: 'der' });
  const pkcs8 = privateKey.export({ type: 'pkcs8', format: 'der' });
  const seed = new Uint8Array(pkcs8.subarray(pkcs8.length - 32));
  const pub = new Uint8Array(spki.subarray(spki.length - 32));
  return { seed, pubkeyB58: bs58.encode(pub) };
}

async function fetchEscrowConfig(gateway: string): Promise<{ usdcMint: string; providerWallet: string }> {
  let resp: Response;
  try {
    resp = await fetch(`${gateway}/v1/escrow/config`, {
      signal: AbortSignal.timeout(CONFIG_TIMEOUT_MS),
    });
  } catch {
    throw new Error(`could not reach ${gateway}/v1/escrow/config — is the gateway URL correct?`);
  }
  if (!resp.ok) {
    await resp.text().catch(() => {});
    throw new Error(
      `gateway /v1/escrow/config returned ${resp.status} — the gateway has no escrow/channel configuration to open a channel against`,
    );
  }
  const body = (await resp.json()) as Record<string, unknown>;
  const usdcMint = body['usdc_mint'];
  const providerWallet = body['provider_wallet'];
  if (typeof usdcMint !== 'string' || usdcMint === '' || typeof providerWallet !== 'string' || providerWallet === '') {
    throw new Error('gateway /v1/escrow/config is missing usdc_mint / provider_wallet');
  }
  return { usdcMint, providerWallet };
}

export interface OpenOptions {
  gateway: string;
  /** USDC decimal amount to fund, e.g. "5" or "2.50". */
  amount: string;
  /** Path to a Solana keypair JSON (id.json array or Solvela wallet.json). */
  walletPath: string;
  /** Solana RPC for the funding tx's recent blockhash (or SOLANA_RPC_URL). */
  rpcUrl?: string;
  /** Skip the interactive confirmation. */
  yes: boolean;
  /** Sidecar port to advertise in ANTHROPIC_BASE_URL (recorded in state). */
  port: number;
  statePath?: string;
  log?: (line: string) => void;
  /** Confirmation hook (default: interactive terminal prompt). */
  confirm?: (summary: string) => Promise<boolean>;
  /**
   * Pin for the gateway-reported USDC mint. Defaults to the canonical
   * MAINNET mint — pass explicitly for devnet etc. A mismatch is a HARD
   * reject that `--yes` never bypasses.
   */
  expectedMint?: string;
  /**
   * Optional pin for the gateway-reported recipient wallet. When set, a
   * mismatch is a HARD reject that `--yes` never bypasses. Without it the
   * recipient is trusted from the gateway config (state that risk to the
   * user in the confirmation).
   */
  expectedRecipient?: string;
}

export async function openChannel(opts: OpenOptions): Promise<void> {
  const log = opts.log ?? ((line: string) => console.log(line));
  const statePath = opts.statePath ?? defaultStatePath();
  const gateway = opts.gateway.replace(/\/+$/, '');

  const atomic = usdcDecimalToAtomic(opts.amount);
  const wallet = await loadKeypair(opts.walletPath);
  const { usdcMint, providerWallet } = await fetchEscrowConfig(gateway);

  // Independent anchors BEFORE anything is signed (round-1 HIGH): the config
  // response is only as trustworthy as --gateway — a phished/MITM'd gateway
  // controls both the mint and the recipient. The mint is pinned to canonical
  // mainnet USDC unless --expected-mint overrides; the recipient is pinned
  // only when --expected-recipient is given. These are HARD failures that
  // --yes never bypasses (--yes only skips the interactive prompt AFTER all
  // checks pass).
  const expectedMint = opts.expectedMint ?? USDC_MINT_MAINNET;
  if (usdcMint !== expectedMint) {
    throw new Error(
      `gateway reported USDC mint ${usdcMint} but ${
        opts.expectedMint !== undefined ? '--expected-mint pins' : 'the canonical mainnet USDC mint is'
      } ${expectedMint}; refusing to sign (pass --expected-mint to pin a non-mainnet mint deliberately)`,
    );
  }
  if (opts.expectedRecipient !== undefined && providerWallet !== opts.expectedRecipient) {
    throw new Error(
      `gateway reported recipient ${providerWallet} but --expected-recipient pins ${opts.expectedRecipient}; refusing to sign`,
    );
  }

  // Verify-what-you-sign, BEFORE any signature exists.
  const recipientProvenance =
    opts.expectedRecipient !== undefined
      ? '(matches --expected-recipient)'
      : '(gateway-reported — pass --expected-recipient to verify it independently)';
  const summary =
    `About to sign a USDC funding transfer:\n` +
    `  amount:    ${opts.amount} USDC (${atomic} atomic)\n` +
    `  from:      ${wallet.publicKey.toBase58()}\n` +
    `  recipient: ${providerWallet} ${recipientProvenance}\n` +
    `  mint:      ${usdcMint}\n` +
    `  gateway:   ${gateway}\n` +
    `This deposit funds a spend-down channel; the unspent balance is refundable via \`solvela-dropin close\`.`;
  log(summary);
  if (!opts.yes) {
    const confirm = opts.confirm ?? defaultConfirm;
    if (!(await confirm(summary))) {
      throw new Error('aborted: confirmation declined (pass --yes to skip the prompt)');
    }
  }

  const rpcUrl = opts.rpcUrl ?? process.env['SOLANA_RPC_URL'];
  if (!rpcUrl) {
    throw new Error(
      'a Solana RPC is required to build the funding transfer — pass --rpc-url or set SOLANA_RPC_URL',
    );
  }

  const fundingTx = await buildFundingTx(new Connection(rpcUrl, 'confirmed'), wallet, providerWallet, usdcMint, atomic);
  const session = generateSessionKey();
  const localToken = randomBytes(32).toString('base64url');

  let resp: Response;
  try {
    resp = await fetch(`${gateway}/v1/channel/open`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        agent_wallet: wallet.publicKey.toBase58(),
        session_key: session.pubkeyB58,
        funding_tx: fundingTx,
      }),
      // Long deadline: the gateway confirms the funding on-chain before
      // replying; abandoning early would lose the channel_id for a deposit
      // that may still land.
      signal: AbortSignal.timeout(OPEN_TIMEOUT_MS),
    });
  } catch {
    throw new Error(
      `could not reach ${gateway}/v1/channel/open (or it did not respond within ${OPEN_TIMEOUT_MS / 1000}s). ` +
        'NOTE: the funding transfer may still have been broadcast — check your wallet before retrying.',
    );
  }
  const respText = await resp.text();
  if (!resp.ok) {
    throw new Error(`channel open failed (${resp.status}): ${sanitizeGatewayError(respText)}`);
  }
  const opened = JSON.parse(respText) as Record<string, unknown>;
  const channelId = opened['channel_id'];
  if (typeof channelId !== 'string' || channelId === '') {
    throw new Error('gateway open response carried no channel_id');
  }

  await saveState(statePath, {
    gateway_url: gateway,
    port: opts.port,
    channel_id: channelId,
    session_seed_b58: bs58.encode(session.seed),
    local_token: localToken,
    last_cumulative: '0',
  });
  session.seed.fill(0); // state file owns it now

  log('');
  log(`Channel open: ${channelId}`);
  log(`  deposited_atomic: ${opened['deposited_atomic']}`);
  log(`  funding_tx_sig:   ${opened['funding_tx_sig'] ?? '(none)'}`);
  log(`  state file:       ${statePath} (mode 0600 — controls the channel; keep it private)`);
  log('');
  log('Point Claude Code at the sidecar with these two env vars:');
  log('');
  log(`  export ANTHROPIC_BASE_URL=http://127.0.0.1:${opts.port}`);
  log(`  export ANTHROPIC_AUTH_TOKEN=${localToken}`);
  log('');
  log('Then start the sidecar:  solvela-dropin serve');
  return;

  async function defaultConfirm(_summary: string): Promise<boolean> {
    const rl = readline.createInterface({ input: process.stdin, output: process.stdout });
    try {
      const answer = (await rl.question('Sign and fund? (yes/no) ')).trim().toLowerCase();
      return answer === 'yes' || answer === 'y';
    } finally {
      rl.close();
    }
  }
}

/**
 * Signed USDC `TransferChecked` (wallet ATA → recipient ATA) — the SAME
 * exact-scheme transfer shape signer-core's signer builds; the gateway's
 * facilitator verifies destination/mint/amount on-chain before crediting.
 */
async function buildFundingTx(
  connection: Connection,
  payer: Keypair,
  recipientWallet: string,
  mintB58: string,
  amountAtomic: bigint,
): Promise<string> {
  const mint = new PublicKey(mintB58);
  const recipient = new PublicKey(recipientWallet);
  const senderAta = await getAssociatedTokenAddress(mint, payer.publicKey);
  const recipientAta = await getAssociatedTokenAddress(mint, recipient);
  const { blockhash } = await connection.getLatestBlockhash('finalized');

  const ix = createTransferCheckedInstruction(
    senderAta,
    mint,
    recipientAta,
    payer.publicKey,
    amountAtomic,
    USDC_DECIMALS,
  );
  const message = new TransactionMessage({
    payerKey: payer.publicKey,
    recentBlockhash: blockhash,
    instructions: [ix],
  }).compileToV0Message();
  const tx = new VersionedTransaction(message);
  tx.sign([payer]);
  return Buffer.from(tx.serialize()).toString('base64');
}
