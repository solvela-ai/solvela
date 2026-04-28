/**
 * Configuration for the @solvela/router OpenClaw plugin.
 *
 * Reads from the same env vars already present on all tenant VPSes:
 *   LLM_ROUTER_API_URL     — Solvela gateway base URL
 *   LLM_ROUTER_WALLET_KEY  — Base58 Solana private key for x402 payments
 */
interface SolvelaConfig {
    /** Solvela gateway base URL (no trailing slash). */
    gatewayUrl: string;
    /** Base58-encoded Solana private key for signing x402 payments. */
    walletKey: string;
    /**
     * Default model to route requests to.
     * "auto" lets the Solvela smart router pick the cheapest capable model.
     */
    defaultModel: string;
}
/** @deprecated Use {@link SolvelaConfig} instead. Will be removed by 2026-08-01. */
type RcrConfig = SolvelaConfig;
declare class ConfigError extends Error {
    constructor(message: string);
}

/**
 * Core routing logic for @solvela/router.
 *
 * Forwards OpenClaw chat requests to Solvela, handling the full
 * x402 payment flow: initial request → 402 response → sign payment → retry.
 * Supports both streaming (SSE) and non-streaming responses.
 *
 * Payment logic is inlined from the Solvela TypeScript SDK so this
 * plugin has zero runtime dependencies.
 */

interface ChatMessage {
    role: 'system' | 'user' | 'assistant' | 'tool';
    content: string;
    name?: string;
}
interface ChatRequest {
    model?: string;
    messages: ChatMessage[];
    max_tokens?: number;
    temperature?: number;
    top_p?: number;
    stream?: boolean;
}
interface ChatChoice {
    index: number;
    message: ChatMessage;
    finish_reason: string | null;
}
interface ChatResponse {
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
declare class PaymentError extends Error {
    constructor(message: string);
}
declare class RouterError extends Error {
    readonly status?: number | undefined;
    constructor(message: string, status?: number | undefined);
}

/**
 * @solvela/router — OpenClaw plugin
 *
 * Routes OpenClaw LLM requests
 * through Solvela with Solana-native x402 USDC micropayments.
 *
 * Installation (on tenant VPS):
 *   openclaw plugins install @solvela/router
 *
 * Required env vars (already present on all Telsi tenant VPSes):
 *   LLM_ROUTER_API_URL     — Solvela gateway base URL
 *   LLM_ROUTER_WALLET_KEY  — Base58 Solana private key for x402 payments
 *
 * Optional env vars:
 *   SOLANA_RPC_URL         — Solana RPC endpoint for on-chain signing
 *                            (required when @solana/web3.js is installed)
 *
 * Usage as a standalone client:
 *   import { createRouter } from '@solvela/router';
 *
 *   const router = createRouter();
 *   const response = await router.chat([{ role: 'user', content: 'Hello!' }]);
 *   console.log(response.choices[0].message.content);
 */

/**
 * OpenClaw plugin descriptor.
 *
 * OpenClaw loads plugins via this default export and calls `intercept` for
 * every outbound LLM request. Returning a response short-circuits the default
 * provider, routing the call through Solvela instead.
 */
interface OpenClawPlugin {
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
 * Create the SolvelaClient OpenClaw plugin.
 *
 * @param overrides - Optional config overrides (useful for testing).
 */
declare function createPlugin(overrides?: Partial<SolvelaConfig>): OpenClawPlugin;
/**
 * High-level router client with a clean async API.
 * Useful when importing the plugin as a library rather than via OpenClaw.
 */
declare class SolvelaClient {
    private readonly config;
    constructor(overrides?: Partial<SolvelaConfig>);
    /**
     * Send a non-streaming chat completion through Solvela.
     *
     * @param messages     - Conversation messages
     * @param model        - Model ID (defaults to config.defaultModel, i.e. "auto")
     * @param options      - Optional max_tokens / temperature overrides
     */
    chat(messages: ChatMessage[], model?: string, options?: {
        max_tokens?: number;
        temperature?: number;
        top_p?: number;
    }): Promise<ChatResponse>;
    /**
     * Send a streaming chat completion through Solvela.
     * Returns the raw SSE Response — iterate with a ReadableStream reader.
     */
    chatStream(messages: ChatMessage[], model?: string, options?: {
        max_tokens?: number;
        temperature?: number;
        top_p?: number;
    }): Promise<Response>;
    /** The resolved configuration (gateway URL, default model). */
    getConfig(): Readonly<SolvelaConfig>;
}
/**
 * @deprecated Use {@link SolvelaClient} instead. Will be removed by 2026-08-01.
 */
declare const RcrClient: new (...args: any[]) => SolvelaClient;
type RcrClient = SolvelaClient;
/**
 * Create a Solvela router client using environment variables.
 * Shorthand for `new SolvelaClient()`.
 */
declare function createRouter(overrides?: Partial<SolvelaConfig>): SolvelaClient;

export { type ChatMessage, type ChatRequest, type ChatResponse, ConfigError, type OpenClawPlugin, PaymentError, RcrClient, type RcrConfig, RouterError, SolvelaClient, type SolvelaConfig, createPlugin, createRouter, createPlugin as default };
