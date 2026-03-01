import { PaymentRequired } from './types';

const X402_VERSION = 2;

/**
 * Creates a base64-encoded PAYMENT-SIGNATURE header value from a 402 response.
 *
 * When @solana/web3.js and @solana/spl-token are installed and a `privateKey`
 * is supplied, this builds and signs a real USDC-SPL TransferChecked versioned
 * transaction.
 *
 * Without those optional dependencies (or without a private key) it falls back
 * to a stub payload suitable for protocol-level testing.
 *
 * Header value format: base64(JSON({ x402_version, resource, accepted, payload }))
 */
export async function createPaymentHeader(
  paymentInfo: PaymentRequired,
  resourceUrl: string,
  privateKey?: string,
): Promise<string> {
  if (!paymentInfo.accepts || paymentInfo.accepts.length === 0) {
    throw new Error('No payment accept options in 402 response');
  }

  const accept = paymentInfo.accepts[0];

  // Attempt real Solana transaction signing when a key is available
  let transaction = 'STUB_BASE64_TX';
  if (privateKey) {
    try {
      transaction = await buildSolanaTransferChecked(accept.pay_to, accept.amount, privateKey);
    } catch {
      // @solana/web3.js not installed or signing failed — fall back to stub
      transaction = 'STUB_BASE64_TX';
    }
  }

  const payload = {
    x402_version: X402_VERSION,
    resource: { url: resourceUrl, method: 'POST' },
    accepted: accept,
    payload: { transaction },
  };

  const json = JSON.stringify(payload);

  if (typeof btoa === 'function') {
    return btoa(json);
  }
  return Buffer.from(json, 'utf-8').toString('base64');
}

/**
 * Build and sign a USDC-SPL TransferChecked versioned transaction.
 *
 * Requires @solana/web3.js ^1.87 and @solana/spl-token to be installed.
 * Throws if those packages are missing.
 *
 * @param payTo       - Gateway recipient wallet (base58)
 * @param amountStr   - Amount in USDC micro-units (6 decimals), e.g. "2625" = $0.002625
 * @param privateKey  - Agent's base58-encoded 64-byte Solana keypair secret key
 * @returns Base64-encoded serialised VersionedTransaction
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

  const secretKey: Uint8Array = bs58.decode(privateKey);
  const payer = Keypair.fromSecretKey(secretKey);

  const recipientPubkey = new PublicKey(payTo);
  const amount = BigInt(amountStr);

  // Derive associated token accounts
  const senderAta = await getAssociatedTokenAddress(USDC_MINT, payer.publicKey);
  const recipientAta = await getAssociatedTokenAddress(USDC_MINT, recipientPubkey);

  // Fetch a recent blockhash from mainnet
  const rpcUrl = process.env.SOLANA_RPC_URL || clusterApiUrl('mainnet-beta');
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
  if (typeof btoa === 'function') {
    return btoa(String.fromCharCode(...serialized));
  }
  return Buffer.from(serialized).toString('base64');
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
