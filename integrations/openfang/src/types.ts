/**
 * Public types for @solvela/openfang-router.
 *
 * The plugin shape mirrors @solvela/router (OpenClaw) deliberately —
 * OpenFang inherits the same plugin loading model from the OpenClaw lineage.
 */

export interface SolvelaRouterConfig {
  /** Solvela gateway base URL (no trailing slash). Defaults to LLM_ROUTER_API_URL env. */
  gatewayUrl: string;
  /**
   * Base58-encoded Solana private key for signing x402 payments.
   * Defaults to LLM_ROUTER_WALLET_KEY env. Optional in dev mode (stub signing).
   */
  walletKey?: string;
  /** Default model id (e.g. "auto", "demo", "anthropic/claude-sonnet-4-6"). */
  defaultModel?: string;
  /**
   * Routing profile. `agentic` is auto-selected when the request includes a
   * non-empty `tools` array regardless of this setting.
   */
  profile?: 'eco' | 'auto' | 'premium' | 'free' | 'agentic';
  /** Per-request timeout (ms). Default 120_000. */
  timeoutMs?: number;
}

export interface ChatMessage {
  role: 'system' | 'user' | 'assistant' | 'tool';
  content: string;
  name?: string;
}

export interface ChatTool {
  type: 'function';
  function: {
    name: string;
    description?: string;
    parameters?: Record<string, unknown>;
  };
}

export interface ChatRequest {
  model?: string;
  messages: ChatMessage[];
  max_tokens?: number;
  temperature?: number;
  top_p?: number;
  stream?: boolean;
  tools?: ChatTool[];
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

export interface PaymentAccept {
  scheme: string;
  network: string;
  amount: string;
  asset: string;
  pay_to: string;
  max_timeout_seconds: number;
}

export interface PaymentRequired {
  x402_version: number;
  accepts: PaymentAccept[];
  cost_breakdown: {
    total: string;
    currency: string;
    provider_cost: string;
    platform_fee: string;
    fee_percent: number;
  };
  error: string;
}

/** A single SSE chunk surfaced to OpenFang's streaming consumers. */
export interface Chunk {
  /** The raw event line ("data: ..." minus the trailing newline). */
  raw: string;
  /** Parsed delta payload when JSON-decodable. */
  data?: unknown;
}

/** OpenFang plugin descriptor. */
export interface OpenFangPlugin {
  name: string;
  version: string;
  description: string;
  /** Non-streaming completion. */
  complete: (req: ChatRequest) => Promise<ChatResponse>;
  /** Streaming completion. Returns an async iterable of SSE chunks. */
  completeStream: (req: ChatRequest) => AsyncIterable<Chunk>;
}

export class ConfigError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'ConfigError';
  }
}

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
