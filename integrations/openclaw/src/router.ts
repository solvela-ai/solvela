/**
 * Core routing logic for @rustyclaw/rcr.
 *
 * Forwards OpenClaw chat requests to Solvela, handling the full
 * x402 payment flow: initial request → 402 response → sign payment → retry.
 * Supports both streaming (SSE) and non-streaming responses.
 *
 * Payment logic is inlined from the Solvela TypeScript SDK so this
 * plugin has zero runtime dependencies.
 */

import type { RcrConfig } from './config.js';

// ── Types (inlined from SDK) ──────────────────────────────────────────────────

export interface ChatMessage {
  role: 'system' | 'user' | 'assistant' | 'tool';
  content: string;
  name?: string;
}

export interface ChatRequest {
  model?: string;
  messages: ChatMessage[];
  max_tokens?: number;
  temperature?: number;
  top_p?: number;
  stream?: boolean;
}

export interface ChatChoice {
  index: number;
  message: ChatMessage;
  finish_reason: string | null;
}

export interface ChatResponse {
  id: string;
  object: string;
  created: number;
  model: string;
  choices: ChatChoice[];
  usage?: {
    prompt_tokens: number;
    completion_tokens: number;
    total_tokens: number;
  };
}

interface PaymentAccept {
  scheme: string;
  network: string;
  amount: string;
  asset: string;
  pay_to: string;
  max_timeout_seconds: number;
}

interface PaymentRequired {
  x402_version: number;
  accepts: PaymentAccept[];
  cost_breakdown: { total: string; currency: string; provider_cost: string; platform_fee: string; fee_percent: number };
  error: string;
}

// ── Errors ────────────────────────────────────────────────────────────────────

export class PaymentError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'PaymentError';
  }
}

export class RouterError extends Error {
  constructor(
    message: string,
    public readonly status?: number,
  ) {
    super(message);
    this.name = 'RouterError';
  }
}

// ── x402 payment header builder (self-contained) ──────────────────────────────

const X402_VERSION = 2;

/**
 * Creates a base64-encoded payment-signature header value.
 *
 * When @solana/web3.js and @solana/spl-token are available and walletKey is
 * set, this builds and signs a real USDC-SPL TransferChecked transaction.
 * If @solana/web3.js is not installed, a stub payload is used (dev/CI mode).
 * If the key is present but signing fails, PaymentError is thrown.
 */
async function createPaymentHeader(
  paymentInfo: PaymentRequired,
  resourceUrl: string,
  walletKey: string,
): Promise<string> {
  if (!paymentInfo.accepts || paymentInfo.accepts.length === 0) {
    throw new PaymentError('No payment accept options in 402 response');
  }

  const accept = paymentInfo.accepts[0];
  let transaction = 'STUB_BASE64_TX';

  if (walletKey) {
    const solanaAvailable = isSolanaAvailable();
    if (solanaAvailable) {
      transaction = await buildSolanaTransferChecked(accept.pay_to, accept.amount, walletKey);
    }
    // If @solana/web3.js is not installed, fall through to stub (dev mode).
  }

  const payload = {
    x402_version: X402_VERSION,
    resource: { url: resourceUrl, method: 'POST' },
    accepted: accept,
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

  // USDC mainnet mint (6 decimals)
  const USDC_MINT = new PublicKey('EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v');
  const USDC_DECIMALS = 6;

  const amount = BigInt(amountStr);
  if (amount <= 0n) {
    throw new PaymentError(`Payment amount must be positive, got: ${amountStr}`);
  }

  let secretKey: Uint8Array | null = null;
  try {
    secretKey = bs58.decode(privateKey) as Uint8Array;
    const payer = Keypair.fromSecretKey(secretKey);
    const recipientPubkey = new PublicKey(payTo);

    const senderAta = await getAssociatedTokenAddress(USDC_MINT, payer.publicKey);
    const recipientAta = await getAssociatedTokenAddress(USDC_MINT, recipientPubkey);

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

// ── 402 response parser ───────────────────────────────────────────────────────

async function parse402(resp: Response): Promise<PaymentRequired | null> {
  try {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const body: any = await resp.json();
    // Gateway wraps the PaymentRequired as a JSON string inside error.message
    const errorMsg = body?.error?.message;
    if (typeof errorMsg === 'string') {
      return JSON.parse(errorMsg) as PaymentRequired;
    }
    // Fallback: body itself is the PaymentRequired
    if (body?.x402_version && body?.accepts) {
      return body as PaymentRequired;
    }
    return null;
  } catch {
    return null;
  }
}

// ── Fetch with timeout ────────────────────────────────────────────────────────

const DEFAULT_TIMEOUT_MS = 120_000;

async function fetchWithTimeout(
  url: string,
  init: RequestInit,
  timeoutMs = DEFAULT_TIMEOUT_MS,
): Promise<Response> {
  const controller = new AbortController();
  const id = setTimeout(() => controller.abort(), timeoutMs);
  try {
    return await fetch(url, { ...init, signal: controller.signal });
  } finally {
    clearTimeout(id);
  }
}

// ── Main routing function ─────────────────────────────────────────────────────

/**
 * Route a chat completion request through Solvela.
 *
 * Handles the x402 payment flow transparently:
 *   1. POST to /v1/chat/completions
 *   2. On 402 → parse PaymentRequired, sign payment, retry with header
 *   3. Return the ChatResponse (non-streaming) or the raw Response (streaming)
 *
 * @param request  - The chat request to forward
 * @param config   - Plugin configuration (gateway URL, wallet key, default model)
 * @returns ChatResponse for non-streaming requests, or a streaming Response
 */
export async function routeRequest(
  request: ChatRequest,
  config: RcrConfig,
): Promise<ChatResponse> {
  const body = {
    model: request.model ?? config.defaultModel,
    messages: request.messages,
    max_tokens: request.max_tokens,
    temperature: request.temperature,
    top_p: request.top_p,
    stream: false,
  };

  const url = `${config.gatewayUrl}/v1/chat/completions`;
  const init: RequestInit = {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  };

  let resp = await fetchWithTimeout(url, init);

  if (resp.status === 402) {
    const paymentInfo = await parse402(resp);
    if (!paymentInfo) {
      throw new PaymentError('Received 402 but could not parse payment details from response');
    }

    const paymentHeader = await createPaymentHeader(paymentInfo, url, config.walletKey);

    resp = await fetchWithTimeout(url, {
      ...init,
      headers: {
        'content-type': 'application/json',
        'payment-signature': paymentHeader,
      },
    });
  }

  if (!resp.ok) {
    const errorText = await resp.text().catch(() => '');
    throw new RouterError(
      `Gateway returned ${resp.status} ${resp.statusText}${errorText ? ` — ${errorText}` : ''}`,
      resp.status,
    );
  }

  return resp.json() as Promise<ChatResponse>;
}

/**
 * Route a streaming chat completion request through Solvela.
 *
 * Returns the raw Response with the SSE body so the caller can stream
 * chunks directly to the OpenClaw client. The x402 payment is handled
 * before the stream is opened.
 *
 * @param request  - The chat request (stream will be forced to true)
 * @param config   - Plugin configuration
 * @returns The streaming Response (caller must consume the body)
 */
export async function routeStreamingRequest(
  request: ChatRequest,
  config: RcrConfig,
): Promise<Response> {
  const body = {
    model: request.model ?? config.defaultModel,
    messages: request.messages,
    max_tokens: request.max_tokens,
    temperature: request.temperature,
    top_p: request.top_p,
    stream: true,
  };

  const url = `${config.gatewayUrl}/v1/chat/completions`;
  const init: RequestInit = {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  };

  // For streaming we must probe cost first with a non-streaming preflight
  // only if we receive a 402 on the streaming request itself.
  let resp = await fetchWithTimeout(url, init);

  if (resp.status === 402) {
    const paymentInfo = await parse402(resp);
    if (!paymentInfo) {
      throw new PaymentError('Received 402 but could not parse payment details from response');
    }

    const paymentHeader = await createPaymentHeader(paymentInfo, url, config.walletKey);

    resp = await fetchWithTimeout(url, {
      ...init,
      headers: {
        'content-type': 'application/json',
        'payment-signature': paymentHeader,
      },
    });
  }

  if (!resp.ok) {
    const errorText = await resp.text().catch(() => '');
    throw new RouterError(
      `Gateway returned ${resp.status} ${resp.statusText}${errorText ? ` — ${errorText}` : ''}`,
      resp.status,
    );
  }

  return resp;
}
