/**
 * @solvela/openfang-router — OpenFang plugin for Solvela
 *
 * Routes OpenFang LLM requests through Solvela with Solana-native x402
 * USDC micropayments. Mirrors the @solvela/router (OpenClaw) plugin —
 * use this one for OpenFang tenants, the other for legacy OpenClaw.
 *
 * Required env vars (already set on Telsi tenant VMs):
 *   LLM_ROUTER_API_URL     — Solvela gateway base URL (or 127.0.0.1:8402 sidecar)
 *   LLM_ROUTER_WALLET_KEY  — Base58 Solana private key for x402 payments
 *
 * Optional env vars:
 *   SOLANA_RPC_URL         — Solana RPC endpoint for on-chain signing
 *
 * Usage:
 *   import { createSolvelaRouter } from '@solvela/openfang-router';
 *
 *   const router = createSolvelaRouter({
 *     gatewayUrl: 'http://127.0.0.1:8402',
 *     walletKey: process.env.LLM_ROUTER_WALLET_KEY,
 *   });
 *   const resp = await router.complete({ messages: [{ role: 'user', content: 'hi' }] });
 */

import { createPaymentHeader, parse402 } from './sign.js';
import {
  ConfigError,
  PaymentError,
  RouterError,
  type ChatMessage,
  type ChatRequest,
  type ChatResponse,
  type Chunk,
  type OpenFangPlugin,
  type SolvelaRouterConfig,
} from './types.js';

export type {
  SolvelaRouterConfig,
  ChatMessage,
  ChatRequest,
  ChatResponse,
  Chunk,
  OpenFangPlugin,
} from './types.js';
export { ConfigError, PaymentError, RouterError } from './types.js';

const LOG_PREFIX = '[solvela]';
const DEFAULT_TIMEOUT_MS = 120_000;

interface ResolvedConfig {
  gatewayUrl: string;
  walletKey: string | undefined;
  defaultModel: string;
  profile: SolvelaRouterConfig['profile'];
  timeoutMs: number;
}

function resolveConfig(config: SolvelaRouterConfig): ResolvedConfig {
  const gatewayUrl = (config.gatewayUrl || process.env.LLM_ROUTER_API_URL || '').replace(/\/$/, '');
  const walletKey = config.walletKey ?? process.env.LLM_ROUTER_WALLET_KEY ?? undefined;
  const defaultModel = config.defaultModel || 'auto';
  const profile = config.profile;
  const timeoutMs = config.timeoutMs ?? DEFAULT_TIMEOUT_MS;

  if (!gatewayUrl) {
    throw new ConfigError(
      'gatewayUrl is required. Set LLM_ROUTER_API_URL or pass `gatewayUrl` to createSolvelaRouter().',
    );
  }

  return { gatewayUrl, walletKey, defaultModel, profile, timeoutMs };
}

/** Choose the routing profile, auto-selecting `agentic` when tools are present. */
function chooseProfile(
  request: ChatRequest,
  configured: SolvelaRouterConfig['profile'],
): SolvelaRouterConfig['profile'] | undefined {
  if (request.tools && request.tools.length > 0) {
    return 'agentic';
  }
  return configured;
}

/** Normalize OpenAI-style content arrays to plain strings (parity with OpenClaw plugin). */
function normalizeMessages(messages: ChatMessage[]): ChatMessage[] {
  return messages.map((msg) => {
    const m = msg as unknown as Record<string, unknown>;
    if (Array.isArray(m.content)) {
      const textParts = (m.content as Array<{ type: string; text?: string }>)
        .filter((part) => part.type === 'text')
        .map((part) => part.text ?? '');
      return { ...msg, content: textParts.join('\n') };
    }
    return msg;
  });
}

async function fetchWithTimeout(
  url: string,
  init: RequestInit,
  timeoutMs: number,
): Promise<Response> {
  const controller = new AbortController();
  const id = setTimeout(() => controller.abort(), timeoutMs);
  try {
    return await fetch(url, { ...init, signal: controller.signal });
  } finally {
    clearTimeout(id);
  }
}

interface RoutedFetch {
  resp: Response;
  url: string;
}

async function routedFetch(
  request: ChatRequest,
  config: ResolvedConfig,
  stream: boolean,
): Promise<RoutedFetch> {
  const profile = chooseProfile(request, config.profile);
  const messages = normalizeMessages(request.messages);
  const body = {
    model: request.model ?? config.defaultModel,
    messages,
    max_tokens: request.max_tokens,
    temperature: request.temperature,
    top_p: request.top_p,
    tools: request.tools,
    stream,
    ...(profile ? { profile } : {}),
  };

  const url = `${config.gatewayUrl}/v1/chat/completions`;
  const init: RequestInit = {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  };

  let resp = await fetchWithTimeout(url, init, config.timeoutMs);

  if (resp.status === 402) {
    const paymentInfo = await parse402(resp);
    if (!paymentInfo) {
      throw new PaymentError('Received 402 but could not parse payment details from response');
    }
    const header = await createPaymentHeader(paymentInfo, url, config.walletKey);

    resp = await fetchWithTimeout(
      url,
      {
        ...init,
        headers: {
          'content-type': 'application/json',
          'payment-signature': header,
        },
      },
      config.timeoutMs,
    );
  }

  if (!resp.ok) {
    const errorText = await resp.text().catch(() => '');
    throw new RouterError(
      `${LOG_PREFIX} Gateway returned ${resp.status} ${resp.statusText}` +
        (errorText ? ` — ${errorText}` : ''),
      resp.status,
    );
  }

  return { resp, url };
}

async function* streamChunks(resp: Response): AsyncIterable<Chunk> {
  if (!resp.body) {
    return;
  }
  const reader = resp.body.getReader();
  const decoder = new TextDecoder();
  let buffer = '';
  try {
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      buffer += decoder.decode(value, { stream: true });
      let nl = buffer.indexOf('\n');
      while (nl !== -1) {
        const line = buffer.slice(0, nl).trimEnd();
        buffer = buffer.slice(nl + 1);
        if (line.startsWith('data:')) {
          const raw = line.replace(/^data:\s?/, '');
          let data: unknown;
          try {
            data = JSON.parse(raw);
          } catch {
            data = undefined;
          }
          yield { raw, data };
        }
        nl = buffer.indexOf('\n');
      }
    }
  } finally {
    reader.releaseLock();
  }
}

/**
 * Create a Solvela router plugin instance for OpenFang.
 *
 * Returned object can be plugged into OpenFang's plugin loader OR used
 * directly as a high-level client (`router.complete(...)`).
 */
export function createSolvelaRouter(config: SolvelaRouterConfig = { gatewayUrl: '' }): OpenFangPlugin {
  const resolved = resolveConfig(config);

  return {
    name: '@solvela/openfang-router',
    version: '0.1.0',
    description: 'Solvela router plugin for OpenFang — pay LLM calls in USDC via x402',

    async complete(request: ChatRequest): Promise<ChatResponse> {
      const { resp } = await routedFetch(request, resolved, false);
      return (await resp.json()) as ChatResponse;
    },

    completeStream(request: ChatRequest): AsyncIterable<Chunk> {
      // Eagerly start the request; iteration begins once the body arrives.
      const fetched = routedFetch(request, resolved, true);
      return {
        async *[Symbol.asyncIterator]() {
          const { resp } = await fetched;
          yield* streamChunks(resp);
        },
      };
    },
  };
}

export default createSolvelaRouter;
