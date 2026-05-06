/**
 * x402 PAYMENT-SIGNATURE header construction.
 *
 * Wire format (matches `solvela_protocol::PaymentPayload` in crates/protocol/src/payment.rs):
 *
 *   base64(JSON({
 *     x402_version: 2,
 *     resource:  { url, method: "POST" },
 *     accepted:  PaymentAccept,
 *     payload:   { transaction: "<base64 signed VersionedTransaction>" }   // direct (scheme = "exact")
 *              | { deposit_tx, service_id, agent_pubkey }                  // escrow
 *   }))
 *
 * History: this module was a standalone @solvela/sdk/x402 export until commit
 * d2824e0a, which deleted the local typescript SDK snapshot in favour of a
 * published @solvela/sdk@0.2.x. The published 0.2.1 turned out to be wire-
 * incompatible with the production gateway (it emits camelCase top-level
 * `scheme`/`network` instead of the gateway's required `resource`/`accepted`
 * envelope, and `transaction_signature` instead of `transaction`). Until a
 * future gateway release accepts both formats, signer-core owns the signing
 * code so the MCP server, OpenClaw provider, and AI SDK provider have a
 * production-compatible signer with no dead workspace dependency.
 */

import { createHash, randomBytes } from 'node:crypto';

import {
  Connection,
  Keypair,
  PublicKey,
  SystemProgram,
  TransactionMessage,
  VersionedTransaction,
  type TransactionInstruction,
} from '@solana/web3.js';
import {
  ASSOCIATED_TOKEN_PROGRAM_ID,
  TOKEN_PROGRAM_ID,
  createTransferCheckedInstruction,
  getAssociatedTokenAddress,
} from '@solana/spl-token';
import bs58 from 'bs58';

import type { PaymentAccept, PaymentRequired } from './types.js';

const X402_VERSION = 2;

/** USDC mainnet mint (6 decimals). */
const USDC_MINT = new PublicKey('EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v');
const USDC_DECIMALS = 6;

/**
 * Error thrown when Solana transaction signing fails.
 *
 * Only `message` is exposed. The underlying error from `@solana/web3.js`,
 * `@solana/spl-token`, or `bs58` is intentionally NOT preserved — its
 * `.stack` / internal buffers could leak transient secret material if a
 * downstream consumer serialized the error or chained it via a logging
 * framework that traverses `cause`. The wrapping message captures the
 * underlying `.message` text, which is sufficient for diagnosis.
 */
export class SigningError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'SigningError';
  }
}

/**
 * Build a base64-encoded PAYMENT-SIGNATURE header from a 402 response.
 *
 * Stub mode: when `privateKey` is undefined, returns a header with the
 * stub transaction marker (`STUB_BASE64_TX` direct / `STUB_ESCROW_DEPOSIT_TX`
 * escrow) so callers can exercise the protocol-level flow without on-chain
 * signing. The gateway will reject stub headers; consumers should guard
 * with `isStubHeader()` before sending.
 *
 * Real-signing mode: when `privateKey` is supplied, builds a USDC-SPL
 * `TransferChecked` versioned transaction (direct scheme) or an Anchor
 * escrow `deposit` instruction (escrow scheme), signs it, and embeds the
 * base64 transaction bytes in the wire payload.
 *
 * `SOLANA_RPC_URL` must be set when `privateKey` is supplied — both real-
 * signing paths fetch a recent blockhash from RPC.
 *
 * @throws Error              on empty `accepts` array
 * @throws SigningError       on any signing/RPC failure (real-signing mode)
 */
export async function createPaymentHeader(
  paymentInfo: PaymentRequired,
  resourceUrl: string,
  privateKey?: string,
  requestBody?: string,
): Promise<string> {
  if (!paymentInfo.accepts || paymentInfo.accepts.length === 0) {
    throw new Error('No payment accept options in 402 response');
  }

  // Prefer escrow when offered AND the program ID is set; otherwise first accept.
  const escrowAccept = paymentInfo.accepts.find(
    (a) => a.scheme === 'escrow' && a.escrow_program_id,
  );
  const accept = escrowAccept ?? paymentInfo.accepts[0];

  let payload: Record<string, unknown>;

  if (accept.scheme === 'escrow' && accept.escrow_program_id) {
    const escrowPayload = await buildEscrowPaymentHeader(accept, privateKey, requestBody);
    payload = {
      x402_version: X402_VERSION,
      resource: { url: resourceUrl, method: 'POST' },
      accepted: accept,
      payload: escrowPayload,
    };
  } else {
    let transaction = 'STUB_BASE64_TX';
    if (privateKey) {
      transaction = await buildSolanaTransferChecked(accept.pay_to, accept.amount, privateKey);
    }
    payload = {
      x402_version: X402_VERSION,
      resource: { url: resourceUrl, method: 'POST' },
      accepted: accept,
      payload: { transaction },
    };
  }

  return Buffer.from(JSON.stringify(payload), 'utf-8').toString('base64');
}

/**
 * Decode a base64 PAYMENT-SIGNATURE header back to its JSON payload.
 * Returns `unknown` — callers should validate the shape before use.
 */
export function decodePaymentHeader(header: string): unknown {
  return JSON.parse(Buffer.from(header, 'base64').toString('utf-8'));
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

interface EscrowInner {
  deposit_tx: string;
  service_id: string;
  agent_pubkey: string;
}

async function buildEscrowPaymentHeader(
  accept: PaymentAccept,
  privateKey: string | undefined,
  requestBody: string | undefined,
): Promise<EscrowInner> {
  const bodyBytes = Buffer.from(requestBody ?? '', 'utf-8');
  const random = randomBytes(8);
  const serviceId = createHash('sha256').update(bodyBytes).update(random).digest();
  const serviceIdB64 = serviceId.toString('base64');

  if (!privateKey) {
    return {
      deposit_tx: 'STUB_ESCROW_DEPOSIT_TX',
      service_id: serviceIdB64,
      agent_pubkey: 'STUB_AGENT_PUBKEY',
    };
  }

  const { depositTx, agentPubkey } = await buildEscrowDeposit(
    accept.pay_to,
    accept.amount,
    accept.escrow_program_id!,
    privateKey,
    serviceId,
    accept.max_timeout_seconds,
  );
  return { deposit_tx: depositTx, service_id: serviceIdB64, agent_pubkey: agentPubkey };
}

async function buildEscrowDeposit(
  providerWallet: string,
  amountStr: string,
  programIdStr: string,
  privateKey: string,
  serviceId: Buffer,
  maxTimeoutSeconds: number,
): Promise<{ depositTx: string; agentPubkey: string }> {
  const amount = parsePositiveAmount(amountStr, 'Escrow deposit');

  let secretKey: Uint8Array | null = null;
  try {
    secretKey = bs58.decode(privateKey);
    const payer = Keypair.fromSecretKey(secretKey);
    const agentPubkey = payer.publicKey.toBase58();

    const providerPubkey = new PublicKey(providerWallet);
    const programId = new PublicKey(programIdStr);

    // Escrow PDA seeds: ["escrow", agent, serviceId]
    const [escrowPda] = PublicKey.findProgramAddressSync(
      [Buffer.from('escrow'), payer.publicKey.toBuffer(), serviceId],
      programId,
    );

    const agentAta = await getAssociatedTokenAddress(USDC_MINT, payer.publicKey);
    const vaultAta = await getAssociatedTokenAddress(USDC_MINT, escrowPda, true);

    const connection = mustConnection();
    const [{ blockhash }, currentSlot] = await Promise.all([
      connection.getLatestBlockhash('finalized'),
      connection.getSlot('confirmed'),
    ]);

    // Solana ~400ms/slot; floor to integer; minimum 10 slots.
    const timeoutSlots = Math.max(Math.floor((maxTimeoutSeconds * 1000) / 400), 10);
    const expirySlot = BigInt(currentSlot + timeoutSlots);

    // Anchor instruction discriminator: sha256("global:deposit")[0:8]
    const discriminator = createHash('sha256')
      .update(Buffer.from('global:deposit', 'utf-8'))
      .digest()
      .subarray(0, 8);

    const data = Buffer.concat([
      discriminator,
      u64LE(amount),
      serviceId,
      u64LE(expirySlot),
    ]);

    const ix: TransactionInstruction = {
      programId,
      keys: [
        { pubkey: payer.publicKey, isSigner: true, isWritable: true },
        { pubkey: providerPubkey, isSigner: false, isWritable: false },
        { pubkey: USDC_MINT, isSigner: false, isWritable: false },
        { pubkey: escrowPda, isSigner: false, isWritable: true },
        { pubkey: agentAta, isSigner: false, isWritable: true },
        { pubkey: vaultAta, isSigner: false, isWritable: true },
        { pubkey: TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
        { pubkey: ASSOCIATED_TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
        { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
      ],
      data,
    };

    const message = new TransactionMessage({
      payerKey: payer.publicKey,
      recentBlockhash: blockhash,
      instructions: [ix],
    }).compileToV0Message();

    const tx = new VersionedTransaction(message);
    tx.sign([payer]);

    return {
      depositTx: Buffer.from(tx.serialize()).toString('base64'),
      agentPubkey,
    };
  } catch (err) {
    if (err instanceof SigningError) throw err;
    throw new SigningError(
      `Failed to build escrow deposit transaction: ${err instanceof Error ? err.message : String(err)}`,
    );
  } finally {
    // Zero the original Uint8Array so the bytes are not retained in this
    // specific buffer if the heap is later snapshotted. Note: `Keypair`
    // copies the bytes internally — a copy still exists in the V8 heap
    // until the `payer` reference goes out of scope. This zero is
    // best-effort, not full erasure.
    if (secretKey) secretKey.fill(0);
  }
}

async function buildSolanaTransferChecked(
  payTo: string,
  amountStr: string,
  privateKey: string,
): Promise<string> {
  const amount = parsePositiveAmount(amountStr, 'Payment');

  let secretKey: Uint8Array | null = null;
  try {
    secretKey = bs58.decode(privateKey);
    const payer = Keypair.fromSecretKey(secretKey);
    const recipientPubkey = new PublicKey(payTo);

    const senderAta = await getAssociatedTokenAddress(USDC_MINT, payer.publicKey);
    const recipientAta = await getAssociatedTokenAddress(USDC_MINT, recipientPubkey);

    const connection = mustConnection();
    const { blockhash } = await connection.getLatestBlockhash('finalized');

    const ix = createTransferCheckedInstruction(
      senderAta,
      USDC_MINT,
      recipientAta,
      payer.publicKey,
      amount,
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
  } catch (err) {
    if (err instanceof SigningError) throw err;
    throw new SigningError(
      `Failed to build Solana payment transaction: ${err instanceof Error ? err.message : String(err)}`,
    );
  } finally {
    // See note in buildEscrowDeposit re: best-effort vs full erasure.
    if (secretKey) secretKey.fill(0);
  }
}

function parsePositiveAmount(amountStr: string, label: string): bigint {
  // BigInt() throws SyntaxError on decimals/scientific/whitespace; pre-validate
  // so the failure surfaces as a SigningError with a useful message instead.
  // Wire format: amounts are always integer atomic-unit strings (USDC has
  // 6 decimals, so $0.002625 USDC is the string "2625").
  if (!/^\d+$/.test(amountStr)) {
    throw new SigningError(
      `${label} amount must be a non-negative integer string (atomic units), got: ${amountStr}`,
    );
  }
  const amount = BigInt(amountStr);
  if (amount <= 0n) {
    throw new SigningError(`${label} amount must be positive, got: ${amountStr}`);
  }
  return amount;
}

function mustConnection(): Connection {
  const rpcUrl = process.env['SOLANA_RPC_URL'];
  if (!rpcUrl) {
    throw new SigningError(
      'SOLANA_RPC_URL environment variable is required for on-chain signing. ' +
        'Set it to your RPC endpoint (e.g. https://api.mainnet-beta.solana.com).',
    );
  }
  return new Connection(rpcUrl, 'confirmed');
}

/** Encode a u64 as 8-byte little-endian. Avoids `BigInt64Array` for older Node. */
function u64LE(value: bigint): Buffer {
  const buf = Buffer.allocUnsafe(8);
  const low = Number(value & 0xffffffffn);
  const high = Number((value >> 32n) & 0xffffffffn);
  buf.writeUInt32LE(low, 0);
  buf.writeUInt32LE(high, 4);
  return buf;
}
