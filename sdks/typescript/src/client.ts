import { ChatMessage, ChatResponse, ClientOptions, PaymentRequired } from './types';
import { createPaymentHeader } from './x402';

const DEFAULT_API_URL = 'https://api.rustyclawrouter.com';
const DEFAULT_TIMEOUT = 60000;

/**
 * RustyClawRouter LLM client with transparent x402 payment handling.
 *
 * Usage:
 *   const client = new LLMClient({ apiUrl: 'http://localhost:8402' });
 *   const reply = await client.chat('gpt-4o', 'Explain x402');
 */
export class LLMClient {
  private apiUrl: string;
  private sessionBudget?: number;
  private sessionSpent: number = 0;
  private timeout: number;

  constructor(options: ClientOptions = {}) {
    this.apiUrl = (
      options.apiUrl || process.env.RCR_API_URL || DEFAULT_API_URL
    ).replace(/\/$/, '');
    this.sessionBudget = options.sessionBudget;
    this.timeout = options.timeout || DEFAULT_TIMEOUT;
  }

  /**
   * Simple one-shot chat. Returns the assistant's text reply.
   */
  async chat(model: string, prompt: string): Promise<string> {
    const response = await this.chatCompletion({
      model,
      messages: [{ role: 'user', content: prompt }],
    });
    return response.choices[0].message.content;
  }

  /**
   * Full OpenAI-compatible chat completion with x402 payment handling.
   *
   * Flow:
   *  1. POST request to /v1/chat/completions
   *  2. If 402 → parse PaymentRequired, check budget, create PAYMENT-SIGNATURE header
   *  3. Retry the request with the payment header
   */
  async chatCompletion(request: {
    model: string;
    messages: ChatMessage[];
    maxTokens?: number;
    temperature?: number;
    stream?: boolean;
  }): Promise<ChatResponse> {
    const body = {
      model: request.model,
      messages: request.messages,
      max_tokens: request.maxTokens,
      temperature: request.temperature,
      stream: request.stream || false,
    };

    const url = `${this.apiUrl}/v1/chat/completions`;

    let resp = await this.fetchWithTimeout(url, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body),
    });

    // Handle x402 payment flow
    if (resp.status === 402) {
      const paymentInfo = await this.parse402(resp);
      if (!paymentInfo) {
        throw new PaymentError('Failed to parse 402 response');
      }

      const cost = parseFloat(paymentInfo.cost_breakdown.total);
      if (this.sessionBudget && this.sessionSpent + cost > this.sessionBudget) {
        throw new BudgetExceededError(
          `Session budget $${this.sessionBudget} exceeded ` +
          `(spent: $${this.sessionSpent.toFixed(6)}, request cost: $${cost.toFixed(6)})`
        );
      }

      const paymentHeader = createPaymentHeader(paymentInfo, url);
      resp = await this.fetchWithTimeout(url, {
        method: 'POST',
        headers: {
          'content-type': 'application/json',
          'payment-signature': paymentHeader,
        },
        body: JSON.stringify(body),
      });

      this.sessionSpent += cost;
    }

    if (!resp.ok) {
      const errorText = await resp.text().catch(() => '');
      throw new Error(
        `Request failed: ${resp.status} ${resp.statusText}${errorText ? ` — ${errorText}` : ''}`
      );
    }

    return resp.json() as Promise<ChatResponse>;
  }

  /**
   * Smart chat: uses the gateway's smart router to pick the cheapest capable model.
   * The profile parameter selects the routing profile: 'auto', 'eco', 'premium', 'free'.
   */
  async smartChat(prompt: string, profile: string = 'auto'): Promise<ChatResponse> {
    return this.chatCompletion({
      model: profile,
      messages: [{ role: 'user', content: prompt }],
    });
  }

  /**
   * List available models with pricing information.
   */
  async listModels(): Promise<unknown> {
    const resp = await this.fetchWithTimeout(`${this.apiUrl}/v1/models`);
    if (!resp.ok) {
      throw new Error(`Failed to list models: ${resp.status} ${resp.statusText}`);
    }
    return resp.json();
  }

  /**
   * Gateway health check. Returns status and Solana RPC connectivity.
   */
  async health(): Promise<unknown> {
    const resp = await this.fetchWithTimeout(`${this.apiUrl}/health`);
    if (!resp.ok) {
      throw new Error(`Health check failed: ${resp.status} ${resp.statusText}`);
    }
    return resp.json();
  }

  /** Total USDC spent in this client session. */
  getSessionSpent(): number {
    return this.sessionSpent;
  }

  /** The configured API URL (trailing slash removed). */
  getApiUrl(): string {
    return this.apiUrl;
  }

  /** Remaining session budget, or undefined if no budget was set. */
  getRemainingBudget(): number | undefined {
    if (this.sessionBudget === undefined) return undefined;
    return Math.max(0, this.sessionBudget - this.sessionSpent);
  }

  /**
   * Fetch wrapper with timeout via AbortController.
   */
  private async fetchWithTimeout(url: string, init?: RequestInit): Promise<Response> {
    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), this.timeout);
    try {
      return await fetch(url, { ...init, signal: controller.signal });
    } finally {
      clearTimeout(timeoutId);
    }
  }

  /**
   * Parse a 402 Payment Required response.
   *
   * The gateway returns:
   *   { "error": { "message": "<JSON-encoded PaymentRequired>" } }
   */
  private async parse402(resp: Response): Promise<PaymentRequired | null> {
    try {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      const body: any = await resp.json();
      const errorMsg = body?.error?.message;
      if (typeof errorMsg === 'string') {
        return JSON.parse(errorMsg) as PaymentRequired;
      }
      // Fallback: body itself might be the PaymentRequired
      if (body?.x402_version && body?.accepts) {
        return body as PaymentRequired;
      }
      return null;
    } catch {
      return null;
    }
  }
}

/**
 * Thrown when the x402 payment handshake fails.
 */
export class PaymentError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'PaymentError';
  }
}

/**
 * Thrown when a request would exceed the configured session budget.
 */
export class BudgetExceededError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'BudgetExceededError';
  }
}
