/**
 * @rustyclaw/rcr — OpenClaw plugin
 *
 * Routes OpenClaw LLM requests
 * through RustyClawRouter with Solana-native x402 USDC micropayments.
 *
 * Installation (on tenant VPS):
 *   openclaw plugins install @rustyclaw/rcr
 *
 * Required env vars (already present on all Telsi tenant VPSes):
 *   LLM_ROUTER_API_URL     — RustyClawRouter gateway base URL
 *   LLM_ROUTER_WALLET_KEY  — Base58 Solana private key for x402 payments
 *
 * Optional env vars:
 *   SOLANA_RPC_URL         — Solana RPC endpoint for on-chain signing
 *                            (required when @solana/web3.js is installed)
 *
 * Usage as a standalone client:
 *   import { createRouter } from '@rustyclaw/rcr';
 *
 *   const router = createRouter();
 *   const response = await router.chat([{ role: 'user', content: 'Hello!' }]);
 *   console.log(response.choices[0].message.content);
 */

import { loadConfig, type RcrConfig } from './config.js';
import {
  routeRequest,
  routeStreamingRequest,
  type ChatMessage,
  type ChatRequest,
  type ChatResponse,
} from './router.js';

export type { RcrConfig } from './config.js';
export { ConfigError } from './config.js';
export type { ChatMessage, ChatRequest, ChatResponse } from './router.js';
export { PaymentError, RouterError } from './router.js';

// ── OpenClaw plugin interface ─────────────────────────────────────────────────

/**
 * OpenClaw plugin descriptor.
 *
 * OpenClaw loads plugins via this default export and calls `intercept` for
 * every outbound LLM request. Returning a response short-circuits the default
 * provider, routing the call through RustyClawRouter instead.
 */
export interface OpenClawPlugin {
  name: string;
  version: string;
  description: string;
  /**
   * Intercept an outbound LLM request.
   * Return a ChatResponse to short-circuit the default provider.
   * Return null to pass the request through unchanged.
   */
  intercept: (request: ChatRequest) => Promise<ChatResponse | null>;
  /**
   * Intercept an outbound streaming LLM request.
   * Return a Response (SSE stream) to short-circuit the default provider.
   * Return null to pass the request through unchanged.
   */
  interceptStream: (request: ChatRequest) => Promise<Response | null>;
}

/**
 * Create the RcrClient OpenClaw plugin.
 *
 * @param overrides - Optional config overrides (useful for testing).
 */
export function createPlugin(overrides: Partial<RcrConfig> = {}): OpenClawPlugin {
  const config = loadConfig(overrides);

  return {
    name: '@rustyclaw/rcr',
    version: '0.1.0',
    description: 'RustyClawRouter — Solana-native LLM routing with x402 USDC payments',

    async intercept(request: ChatRequest): Promise<ChatResponse | null> {
      return routeRequest(request, config);
    },

    async interceptStream(request: ChatRequest): Promise<Response | null> {
      return routeStreamingRequest(request, config);
    },
  };
}

// ── Convenience client ────────────────────────────────────────────────────────

/**
 * High-level router client with a clean async API.
 * Useful when importing the plugin as a library rather than via OpenClaw.
 */
export class RcrClient {
  private readonly config: RcrConfig;

  constructor(overrides: Partial<RcrConfig> = {}) {
    this.config = loadConfig(overrides);
  }

  /**
   * Send a non-streaming chat completion through RustyClawRouter.
   *
   * @param messages     - Conversation messages
   * @param model        - Model ID (defaults to config.defaultModel, i.e. "auto")
   * @param options      - Optional max_tokens / temperature overrides
   */
  async chat(
    messages: ChatMessage[],
    model?: string,
    options: { max_tokens?: number; temperature?: number; top_p?: number } = {},
  ): Promise<ChatResponse> {
    return routeRequest(
      { messages, model: model ?? this.config.defaultModel, ...options },
      this.config,
    );
  }

  /**
   * Send a streaming chat completion through RustyClawRouter.
   * Returns the raw SSE Response — iterate with a ReadableStream reader.
   */
  async chatStream(
    messages: ChatMessage[],
    model?: string,
    options: { max_tokens?: number; temperature?: number; top_p?: number } = {},
  ): Promise<Response> {
    return routeStreamingRequest(
      { messages, model: model ?? this.config.defaultModel, ...options, stream: true },
      this.config,
    );
  }

  /** The resolved configuration (gateway URL, default model). */
  getConfig(): Readonly<RcrConfig> {
    return this.config;
  }
}

/**
 * Create an RCR client using environment variables.
 * Shorthand for `new RcrClient()`.
 */
export function createRouter(overrides: Partial<RcrConfig> = {}): RcrClient {
  return new RcrClient(overrides);
}

// ── Default export (OpenClaw plugin entry point) ──────────────────────────────

export default createPlugin;
