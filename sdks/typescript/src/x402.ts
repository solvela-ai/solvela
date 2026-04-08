import * as crypto from 'crypto';
import { PaymentAccept, PaymentRequired } from './types';

const X402_VERSION = 2;

/**
 * Error thrown when Solana transaction signing fails.
 * Distinct from a missing @solana/web3.js dependency so callers can
 * distinguish "package not installed" (degrade gracefully) from
 * "key is wrong / RPC is down" (surface to user).
 */
export class SigningError extends Error {
  constructor(message: string, public readonly cause?: unknown) {
    super(message);
    this.name = 'SigningError';
  }
}

/**
 * Creates a base64-encoded PAYMENT-SIGNATURE header value from a 402 response.
 *
 * When @solana/web3.js and @solana/spl-token are installed and a `privateKey`
 * is supplied, this builds and signs a real USDC-SPL TransferChecked versioned
 * transaction.
 *
 * Without a private key (no-key mode) it returns a stub payload for
 * protocol-level testing. Without @solana/web3.js installed it also falls back
 * to the stub. **If a key is supplied but signing fails (bad key format, RPC
 * unreachable, invalid amount) a SigningError is thrown** — the caller must
 * handle it rather than silently sending an invalid payment.
 *
 * Header value format: base64(JSON({ x402_version, resource, accepted, payload }))
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

  // Prefer escrow scheme if available, fall back to first accept
  const escrowAccept = paymentInfo.accepts.find(
    (a) => a.scheme === 'escrow' && a.escrow_program_id,
  );
  const accept = escrowAccept ?? paymentInfo.accepts[0];

  let payload: Record<string, unknown>;

  if (accept.scheme === 'escrow' && accept.escrow_program_id) {
    const escrowPayload = await buildEscrowPaymentHeader(
      accept,
      privateKey,
      requestBody,
    );
    payload = {
      x402_version: X402_VERSION,
      resource: { url: resourceUrl, method: 'POST' },
      accepted: accept,
      payload: escrowPayload,
    };
  } else {
    let transaction = 'STUB_BASE64_TX';
    if (privateKey) {
      // Attempt real signing; if @solana/web3.js is missing, degrade to stub.
      // If the package IS available but signing fails, propagate as SigningError.
      const solanaAvailable = isSolanaAvailable();
      if (solanaAvailable) {
        // Throws SigningError on failure — do not catch here.
        transaction = await buildSolanaTransferChecked(accept.pay_to, accept.amount, privateKey);
      }
      // else: package not installed → stub is acceptable (development / CI mode)
    }

    payload = {
      x402_version: X402_VERSION,
      resource: { url: resourceUrl, method: 'POST' },
      accepted: accept,
      payload: { transaction },
    };
  }

  const json = JSON.stringify(payload);

  if (typeof btoa === 'function') {
    return btoa(json);
  }
  return Buffer.from(json, 'utf-8').toString('base64');
}

/**
 * Build the escrow payment payload (stub or real).
 *
 * Generates a unique service_id from the request body and random bytes,
 * then either builds a real escrow deposit transaction (if @solana/web3.js
 * is available and a private key is supplied) or returns stubs.
 */
async function buildEscrowPaymentHeader(
  accept: PaymentAccept,
  privateKey?: string,
  requestBody?: string,
): Promise<{ deposit_tx: string; service_id: string; agent_pubkey: string }> {
  // Generate service_id: sha256(bodyBytes + randomBytes(8))
  const bodyBytes = Buffer.from(requestBody ?? '', 'utf-8');
  const randomBytes = crypto.randomBytes(8);
  const serviceId = crypto
    .createHash('sha256')
    .update(bodyBytes)
    .update(randomBytes)
    .digest();

  const serviceIdB64 = serviceId.toString('base64');

  if (privateKey && isSolanaAvailable()) {
    // Real escrow deposit — throws SigningError on failure
    const { depositTx, agentPubkey } = await buildEscrowDeposit(
      accept.pay_to,
      accept.amount,
      accept.escrow_program_id!,
      privateKey,
      serviceId,
    );
    return { deposit_tx: depositTx, service_id: serviceIdB64, agent_pubkey: agentPubkey };
  }

  // Stub mode (no key or no @solana/web3.js)
  return {
    deposit_tx: 'STUB_ESCROW_DEPOSIT_TX',
    service_id: serviceIdB64,
    agent_pubkey: 'STUB_AGENT_PUBKEY',
  };
}

/**
 * Build and sign a real escrow deposit VersionedTransaction.
 *
 * Derives the escrow PDA and ATAs, constructs the Anchor deposit instruction,
 * signs with the agent keypair, and returns the base64-encoded transaction
 * plus the agent's base58 public key.
 *
 * @param providerWallet   - Gateway/provider wallet (base58)
 * @param amountStr        - Amount in USDC micro-units (6 decimals)
 * @param programIdStr     - Escrow program ID (base58)
 * @param privateKey       - Agent's base58-encoded 64-byte Solana keypair secret key
 * @param serviceId        - 32-byte service ID buffer
 * @throws SigningError on any signing or RPC failure
 */
async function buildEscrowDeposit(
  providerWallet: string,
  amountStr: string,
  programIdStr: string,
  privateKey: string,
  serviceId: Buffer,
): Promise<{ depositTx: string; agentPubkey: string }> {
  // eslint-disable-next-line @typescript-eslint/no-var-requires
  const solanaWeb3 = require('@solana/web3.js');
  // eslint-disable-next-line @typescript-eslint/no-var-requires
  const splToken = require('@solana/spl-token');
  // eslint-disable-next-line @typescript-eslint/no-var-requires
  const bs58 = require('bs58');

  const {
    Connection,
    Keypair,
    PublicKey,
    SystemProgram,
    TransactionMessage,
    VersionedTransaction,
  } = solanaWeb3;
  const { getAssociatedTokenAddress, TOKEN_PROGRAM_ID, ASSOCIATED_TOKEN_PROGRAM_ID } = splToken;

  // USDC mainnet mint (6 decimals)
  const USDC_MINT = new PublicKey('EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v');

  // Validate amount before touching the key
  const amount = BigInt(amountStr);
  if (amount <= 0n) {
    throw new SigningError(`Escrow deposit amount must be positive, got: ${amountStr}`);
  }

  let secretKey: Uint8Array | null = null;
  try {
    secretKey = bs58.decode(privateKey) as Uint8Array;
    const payer = Keypair.fromSecretKey(secretKey);
    const agentPubkey = payer.publicKey.toBase58() as string;

    const providerPubkey = new PublicKey(providerWallet);
    const programId = new PublicKey(programIdStr);

    // Derive escrow PDA: ["escrow", agent, serviceId]
    const [escrowPda] = PublicKey.findProgramAddressSync(
      [Buffer.from('escrow'), payer.publicKey.toBuffer(), serviceId],
      programId,
    );

    // Derive ATAs
    const agentAta = await getAssociatedTokenAddress(USDC_MINT, payer.publicKey);
    const vaultAta = await getAssociatedTokenAddress(USDC_MINT, escrowPda, true);

    // Fetch recent blockhash
    const rpcUrl = process.env.SOLANA_RPC_URL;
    if (!rpcUrl) {
      throw new SigningError(
        'SOLANA_RPC_URL environment variable is required for on-chain signing. ' +
        'Set it to your RPC endpoint (e.g. https://api.mainnet-beta.solana.com).',
      );
    }
    const connection = new Connection(rpcUrl, 'confirmed');
    const { blockhash } = await connection.getLatestBlockhash('finalized');

    // Build instruction data: sha256("global:deposit")[0:8] + amount(u64 LE) + serviceId + expirySlot(u64 LE)
    const discriminator = crypto
      .createHash('sha256')
      .update(Buffer.from('global:deposit', 'utf-8'))
      .digest()
      .subarray(0, 8);

    const amountBuf = Buffer.allocUnsafe(8);
    // Write u64 LE as two 32-bit halves to avoid BigInt64Array on older Node
    const amountLow = Number(amount & 0xffffffffn);
    const amountHigh = Number((amount >> 32n) & 0xffffffffn);
    amountBuf.writeUInt32LE(amountLow, 0);
    amountBuf.writeUInt32LE(amountHigh, 4);

    // expirySlot: 0 (no expiry)
    const expiryBuf = Buffer.alloc(8, 0);

    const data = Buffer.concat([discriminator, amountBuf, serviceId, expiryBuf]);

    // 9 account keys in order
    const keys = [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },   // agent
      { pubkey: providerPubkey, isSigner: false, isWritable: false },   // provider
      { pubkey: USDC_MINT, isSigner: false, isWritable: false },        // mint
      { pubkey: escrowPda, isSigner: false, isWritable: true },         // escrowPda
      { pubkey: agentAta, isSigner: false, isWritable: true },          // agentAta
      { pubkey: vaultAta, isSigner: false, isWritable: true },          // vaultAta
      { pubkey: TOKEN_PROGRAM_ID, isSigner: false, isWritable: false }, // token program
      { pubkey: ASSOCIATED_TOKEN_PROGRAM_ID, isSigner: false, isWritable: false }, // ATA program
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },     // system program
    ];

    const ix = { programId, keys, data };

    const message = new TransactionMessage({
      payerKey: payer.publicKey,
      recentBlockhash: blockhash,
      instructions: [ix],
    }).compileToV0Message();

    const tx = new VersionedTransaction(message);
    tx.sign([payer]);

    const serialized = tx.serialize();
    const depositTx = typeof btoa === 'function'
      ? btoa(String.fromCharCode(...serialized))
      : Buffer.from(serialized).toString('base64');

    return { depositTx, agentPubkey };
  } catch (err) {
    if (err instanceof SigningError) throw err;
    throw new SigningError(
      `Failed to build escrow deposit transaction: ${err instanceof Error ? err.message : String(err)}`,
      err,
    );
  } finally {
    // Zero the secret key bytes to minimise in-memory exposure window
    if (secretKey) secretKey.fill(0);
  }
}

/** Returns true if @solana/web3.js can be required (optional peer dep). */
function isSolanaAvailable(): boolean {
  try {
    require.resolve('@solana/web3.js');
    return true;
  } catch {
    return false;
  }
}

/**
 * Build and sign a USDC-SPL TransferChecked versioned transaction.
 *
 * Requires @solana/web3.js ^1.87 and @solana/spl-token to be installed.
 *
 * @param payTo       - Gateway recipient wallet (base58)
 * @param amountStr   - Amount in USDC micro-units (6 decimals), e.g. "2625" = $0.002625
 * @param privateKey  - Agent's base58-encoded 64-byte Solana keypair secret key
 * @returns Base64-encoded serialised VersionedTransaction
 * @throws SigningError on any signing or RPC failure
 */
async function buildSolanaTransferChecked(
  payTo: string,
  amountStr: string,
  privateKey: string,
): Promise<string> {
  // eslint-disable-next-line @typescript-eslint/no-var-requires
  const solanaWeb3 = require('@solana/web3.js');
  // eslint-disable-next-line @typescript-eslint/no-var-requires
  const splToken = require('@solana/spl-token');
  // eslint-disable-next-line @typescript-eslint/no-var-requires
  const bs58 = require('bs58');

  const {
    Connection,
    Keypair,
    PublicKey,
    TransactionMessage,
    VersionedTransaction,
    clusterApiUrl,
  } = solanaWeb3;
  const { createTransferCheckedInstruction, getAssociatedTokenAddress } = splToken;

  // USDC mainnet mint (6 decimals)
  const USDC_MINT = new PublicKey('EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v');
  const USDC_DECIMALS = 6;

  // Validate amount before touching the key
  const amount = BigInt(amountStr);
  if (amount <= 0n) {
    throw new SigningError(`Payment amount must be positive, got: ${amountStr}`);
  }

  let secretKey: Uint8Array | null = null;
  try {
    secretKey = bs58.decode(privateKey) as Uint8Array;
    const payer = Keypair.fromSecretKey(secretKey);

    const recipientPubkey = new PublicKey(payTo);

    // Derive associated token accounts (pure derivation, no RPC call)
    const senderAta = await getAssociatedTokenAddress(USDC_MINT, payer.publicKey);
    const recipientAta = await getAssociatedTokenAddress(USDC_MINT, recipientPubkey);

    // Fetch a recent blockhash — SOLANA_RPC_URL must be set for production use
    const rpcUrl = process.env.SOLANA_RPC_URL;
    if (!rpcUrl) {
      throw new SigningError(
        'SOLANA_RPC_URL environment variable is required for on-chain signing. ' +
        'Set it to your RPC endpoint (e.g. https://api.mainnet-beta.solana.com).'
      );
    }
    const connection = new Connection(rpcUrl, 'confirmed');
    const { blockhash } = await connection.getLatestBlockhash('finalized');

    // Build TransferChecked instruction
    const ix = createTransferCheckedInstruction(
      senderAta,        // source ATA
      USDC_MINT,        // token mint
      recipientAta,     // destination ATA
      payer.publicKey,  // authority (owner of source ATA)
      amount,           // amount in micro-USDC
      USDC_DECIMALS,    // decimals
    );

    // Build versioned transaction (v0)
    const message = new TransactionMessage({
      payerKey: payer.publicKey,
      recentBlockhash: blockhash,
      instructions: [ix],
    }).compileToV0Message();

    const tx = new VersionedTransaction(message);
    tx.sign([payer]);

    // Serialise to base64
    const serialized = tx.serialize();
    const encoded = typeof btoa === 'function'
      ? btoa(String.fromCharCode(...serialized))
      : Buffer.from(serialized).toString('base64');

    return encoded;
  } catch (err) {
    if (err instanceof SigningError) throw err;
    throw new SigningError(
      `Failed to build Solana payment transaction: ${err instanceof Error ? err.message : String(err)}`,
      err,
    );
  } finally {
    // Zero the secret key bytes to minimise in-memory exposure window
    if (secretKey) secretKey.fill(0);
  }
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
