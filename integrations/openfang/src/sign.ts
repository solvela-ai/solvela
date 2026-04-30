/**
 * x402 payment signing for @solvela/openfang-router.
 *
 * Logic ported byte-for-byte from @solvela/router (OpenClaw plugin) —
 * see integrations/openclaw/src/router.ts. Both plugins speak the same
 * x402 wire format, so the signing path is intentionally shared.
 *
 * The Solana SDKs are loaded lazily via dynamic require so the plugin
 * stays usable in dev/CI environments that do not install @solana/web3.js.
 */

import { PaymentError, type PaymentAccept, type PaymentRequired } from './types.js';

const X402_VERSION = 2;

/** USDC mainnet mint (6 decimals). */
const USDC_MINT = 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v';
const USDC_DECIMALS = 6;

/**
 * Build a base64-encoded `payment-signature` header value from a 402 response.
 *
 * When @solana/web3.js + @solana/spl-token + bs58 are installed AND walletKey
 * is non-empty, signs a real USDC-SPL TransferChecked transaction.
 * Otherwise falls back to a stub payload (dev mode).
 */
export async function createPaymentHeader(
  paymentInfo: PaymentRequired,
  resourceUrl: string,
  walletKey: string | undefined,
): Promise<string> {
  if (!paymentInfo.accepts || paymentInfo.accepts.length === 0) {
    throw new PaymentError('No payment accept options in 402 response');
  }

  const accept = paymentInfo.accepts[0];
  let transaction = 'STUB_BASE64_TX';

  if (walletKey && isSolanaAvailable()) {
    transaction = await buildSolanaTransferChecked(
      accept.pay_to,
      accept.amount,
      walletKey,
    );
  }

  const payload = {
    x402_version: X402_VERSION,
    resource: { url: resourceUrl, method: 'POST' },
    accepted: accept satisfies PaymentAccept,
    payload: { transaction },
  };

  const json = JSON.stringify(payload);
  return typeof Buffer !== 'undefined'
    ? Buffer.from(json, 'utf-8').toString('base64')
    : btoa(json);
}

function isSolanaAvailable(): boolean {
  try {
    // eslint-disable-next-line @typescript-eslint/no-require-imports
    require.resolve('@solana/web3.js');
    return true;
  } catch {
    return false;
  }
}

async function buildSolanaTransferChecked(
  payTo: string,
  amountStr: string,
  privateKey: string,
): Promise<string> {
  // eslint-disable-next-line @typescript-eslint/no-require-imports
  const solanaWeb3 = require('@solana/web3.js');
  // eslint-disable-next-line @typescript-eslint/no-require-imports
  const splToken = require('@solana/spl-token');
  // eslint-disable-next-line @typescript-eslint/no-require-imports
  const bs58 = require('bs58');

  const {
    Connection,
    Keypair,
    PublicKey,
    TransactionMessage,
    VersionedTransaction,
  } = solanaWeb3;
  const { createTransferCheckedInstruction, getAssociatedTokenAddress } = splToken;

  const amount = BigInt(amountStr);
  if (amount <= 0n) {
    throw new PaymentError(`Payment amount must be positive, got: ${amountStr}`);
  }

  let secretKey: Uint8Array | null = null;
  try {
    secretKey = bs58.decode(privateKey) as Uint8Array;
    const payer = Keypair.fromSecretKey(secretKey);
    const recipientPubkey = new PublicKey(payTo);
    const mint = new PublicKey(USDC_MINT);

    const senderAta = await getAssociatedTokenAddress(mint, payer.publicKey);
    const recipientAta = await getAssociatedTokenAddress(mint, recipientPubkey);

    const rpcUrl = process.env.SOLANA_RPC_URL;
    if (!rpcUrl) {
      throw new PaymentError(
        'SOLANA_RPC_URL is required for on-chain signing. ' +
        'Set it to your Solana RPC endpoint (e.g. https://api.mainnet-beta.solana.com).',
      );
    }

    const connection = new Connection(rpcUrl, 'confirmed');
    const { blockhash } = await connection.getLatestBlockhash('finalized');

    const ix = createTransferCheckedInstruction(
      senderAta,
      mint,
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

    const serialized = tx.serialize();
    return typeof Buffer !== 'undefined'
      ? Buffer.from(serialized).toString('base64')
      : btoa(String.fromCharCode(...serialized));
  } catch (err) {
    if (err instanceof PaymentError) throw err;
    throw new PaymentError(
      `Failed to build Solana payment transaction: ${err instanceof Error ? err.message : String(err)}`,
    );
  } finally {
    if (secretKey) secretKey.fill(0);
  }
}

/** Parse a 402 body into PaymentRequired, returning null if it cannot be decoded. */
export async function parse402(resp: Response): Promise<PaymentRequired | null> {
  try {
    const body: unknown = await resp.json();
    const errorMsg = (body as { error?: { message?: unknown } })?.error?.message;
    if (typeof errorMsg === 'string') {
      return JSON.parse(errorMsg) as PaymentRequired;
    }
    if (
      typeof body === 'object' &&
      body !== null &&
      'x402_version' in body &&
      'accepts' in body
    ) {
      return body as PaymentRequired;
    }
    return null;
  } catch {
    return null;
  }
}
