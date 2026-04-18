/**
 * Minimal Solvela gateway client for the MCP server.
 *
 * Handles the x402 payment flow (402 → build payment header → retry)
 * using the real signer from @solvela/sdk. Private key material
 * is read from env only and never logged, serialized into errors, or
 * included in tool responses — SigningError.message is the safe surface.
 */

import { Mutex } from 'async-mutex';
import { createPaymentHeader, SigningError } from '@solvela/sdk/x402';
import type { PaymentRequired, PaymentAccept } from '@solvela/sdk/types';

export type { PaymentRequired, PaymentAccept };

export interface ChatMessage {
  role: 'system' | 'user' | 'assistant';
  content: string;
}

export interface ChatChoice {
  index: number;
  message: ChatMessage;
  finish_reason: string | null;
}

export interface Usage {
  prompt_tokens: number;
  completion_tokens: number;
  total_tokens: number;
}

export interface ChatResponse {
  id: string;
  object: string;
  created: number;
  model: string;
  choices: ChatChoice[];
  usage?: Usage;
}

export interface ModelInfo {
  id: string;
  object: string;
  owned_by: string;
  usdc_price_per_million_input?: string;
  usdc_price_per_million_output?: string;
}

export interface ModelsResponse {
  object: string;
  data: ModelInfo[];
}

export interface HealthResponse {
  status: string;
  version?: string;
  solana_rpc?: string;
  [key: string]: unknown;
}

export interface SpendSummary {
  wallet_address: string | null;
  total_requests: number;
  total_usdc_spent: string;
  session_usdc_spent: string;
  budget_remaining: string | null;
}

export interface GatewayClientOptions {
  /** Gateway URL. Defaults to SOLVELA_API_URL env var or https://api.solvela.ai */
  apiUrl?: string;
  /** Session spend budget in USDC. Requests are rejected if this would be exceeded. */
  sessionBudget?: number;
  /** Request timeout in ms. Defaults to 60000. */
  timeoutMs?: number;
  /**
   * Payment signing mode.
   * - 'auto'   — SDK prefers escrow when the gateway advertises it (default).
   * - 'escrow' — Only accept escrow payment schemes.
   * - 'direct' — Only accept direct TransferChecked payment schemes.
   * - 'off'    — Do not sign; just POST without a payment header (gateway will 402 unless
   *              dev_bypass_payment is enabled).
   */
  signingMode?: 'auto' | 'escrow' | 'direct' | 'off';
}

/**
 * Lightweight gateway client used by the MCP server.
 *
 * Tracks session spend and exposes spend summary for the `spending` tool.
 * Uses createPaymentHeader from @solvela/sdk for real on-chain USDC signing.
 */
export class GatewayClient {
  readonly apiUrl: string;
  private readonly sessionBudget?: number;
  private readonly timeoutMs: number;
  private readonly signingMode: 'auto' | 'escrow' | 'direct' | 'off';
  private sessionSpent = 0;
  private requestCount = 0;
  /** Mutex ensures budget check + increment is atomic across parallel chat calls (T1-H). */
  private readonly budgetMutex = new Mutex();

  constructor(opts: GatewayClientOptions = {}) {
    this.apiUrl = (
      opts.apiUrl ??
      process.env['SOLVELA_API_URL'] ??
      process.env['RCR_API_URL'] ?? // compat — silently accepted
      'https://api.solvela.ai'
    ).replace(/\/$/, '');
    this.sessionBudget = opts.sessionBudget;
    this.timeoutMs = opts.timeoutMs ?? 60_000;
    this.signingMode = opts.signingMode ?? 'auto';
  }

  // ---------------------------------------------------------------------------
  // Public API
  // ---------------------------------------------------------------------------

  async chat(
    model: string,
    messages: ChatMessage[],
    opts: { maxTokens?: number; temperature?: number } = {},
  ): Promise<ChatResponse> {
    const body = {
      model,
      messages,
      max_tokens: opts.maxTokens,
      temperature: opts.temperature,
      stream: false,
    };
    const bodyStr = JSON.stringify(body);

    const url = `${this.apiUrl}/v1/chat/completions`;
    let resp = await this.fetchWithTimeout(url, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: bodyStr,
    });

    if (resp.status === 402) {
      // HF5: parse402 now throws instead of returning null.
      const paymentInfo = await this.parse402(resp);

      const cost = parseFloat(paymentInfo.cost_breakdown.total);

      // T1-H: Atomic budget reservation — both check and increment must be in the same
      // critical section to prevent two parallel calls from both passing the budget check.
      await this.budgetMutex.runExclusive(async () => {
        if (this.sessionBudget !== undefined && this.sessionSpent + cost > this.sessionBudget) {
          throw new Error(
            `Session budget $${this.sessionBudget.toFixed(6)} USDC exceeded ` +
              `(spent: $${this.sessionSpent.toFixed(6)}, request cost: $${cost.toFixed(6)})`,
          );
        }
        this.sessionSpent += cost;
      });

      if (this.signingMode === 'off') {
        // off mode: send without payment header; gateway will likely 402 again
        resp = await this.fetchWithTimeout(url, {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: bodyStr,
        });
      } else {
        // Filter accepts by signing mode before handing off to the SDK signer.
        const filteredPaymentInfo = this.filterAccepts(paymentInfo);
        const privateKey = process.env['SOLANA_WALLET_KEY'];

        let paymentHeader: string;
        try {
          paymentHeader = await createPaymentHeader(filteredPaymentInfo, url, privateKey, bodyStr);
        } catch (err) {
          // HF2: Refund the reserved budget — signer never succeeded.
          await this.budgetMutex.runExclusive(async () => {
            const before = this.sessionSpent;
            this.sessionSpent = Math.max(0, this.sessionSpent - cost);
            if (this.sessionSpent === 0 && before > 0 && before < cost) {
              // Math.max(0, ...) clamped — indicates a race condition in budget accounting.
              process.stderr.write(
                `[solvela-mcp] WARN: budget refund clamped to 0 (before=${before}, cost=${cost}) — possible race condition\n`,
              );
            }
          });
          // HF1: Wrap ALL signer exceptions uniformly — never propagate raw err.
          if (err instanceof SigningError) {
            // SigningError.message is safe — the SDK constructs it without including key material.
            // Do NOT propagate err.cause: it may contain raw byte arrays from web3.js / bs58.
            throw new Error(`Payment signing failed: ${err.message}`);
          }
          throw new Error(
            `Unexpected error during payment signing: ${err instanceof Error ? err.message : String(err)}`,
          );
        }

        // HF3: Reject stub payment headers before sending — prevents burning budget on a call
        // the gateway will reject. Occurs if @solana/web3.js becomes unresolvable at runtime
        // despite a private key being set. Only check when privateKey is defined: without a
        // key the SDK intentionally returns a stub and the gateway mock may return 200 in tests.
        if (privateKey !== undefined) {
          try {
            const decoded = JSON.parse(
              typeof atob === 'function'
                ? atob(paymentHeader)
                : Buffer.from(paymentHeader, 'base64').toString('utf-8'),
            );
            const tx = decoded?.payload?.transaction;
            const depositTx = decoded?.payload?.deposit_tx;
            if (
              (typeof tx === 'string' && tx.startsWith('STUB_')) ||
              (typeof depositTx === 'string' && depositTx.startsWith('STUB_'))
            ) {
              // Refund the reserved budget before throwing.
              await this.budgetMutex.runExclusive(async () => {
                this.sessionSpent = Math.max(0, this.sessionSpent - cost);
              });
              throw new Error(
                'Payment signing returned a stub transaction. This means signing silently ' +
                'degraded to stub mode — likely @solana/web3.js or peer deps are unresolvable ' +
                'at runtime. Reinstall @solvela/mcp-server and retry.',
              );
            }
          } catch (err) {
            // Only re-throw our specific error; decode failures from malformed headers
            // are a separate concern surfaced downstream by the gateway.
            if (err instanceof Error && err.message.startsWith('Payment signing returned a stub')) {
              throw err;
            }
          }
        }

        let retryResp: Response;
        try {
          retryResp = await this.fetchWithTimeout(url, {
            method: 'POST',
            headers: {
              'content-type': 'application/json',
              'payment-signature': paymentHeader,
            },
            body: bodyStr,
          });
        } catch (err) {
          // HF2: Refund on retry-fetch failure (network error / timeout).
          await this.budgetMutex.runExclusive(async () => {
            this.sessionSpent = Math.max(0, this.sessionSpent - cost);
          });
          throw err;
        }

        // HF2: If retry returns non-2xx, refund before surfacing error.
        if (!retryResp.ok) {
          await this.budgetMutex.runExclusive(async () => {
            this.sessionSpent = Math.max(0, this.sessionSpent - cost);
          });
          // HF8: Truncate + sanitize gateway error body.
          const text = (await retryResp.text().catch(() => '')).slice(0, 500);
          const sanitized = text.replace(/payment-signature[^\s,}"]+/gi, '[redacted]');
          throw new Error(`Gateway error ${retryResp.status}: ${sanitized}`);
        }

        resp = retryResp;
      }

      this.requestCount += 1;
    } else if (resp.ok) {
      this.requestCount += 1;
    }

    if (!resp.ok) {
      // HF8: Truncate + sanitize gateway error body.
      const text = (await resp.text().catch(() => '')).slice(0, 500);
      const sanitized = text.replace(/payment-signature[^\s,}"]+/gi, '[redacted]');
      throw new Error(`Gateway error ${resp.status}: ${sanitized}`);
    }

    return resp.json() as Promise<ChatResponse>;
  }

  async listModels(): Promise<ModelsResponse> {
    const resp = await this.fetchWithTimeout(`${this.apiUrl}/v1/models`);
    if (!resp.ok) throw new Error(`Failed to list models: ${resp.status}`);
    return resp.json() as Promise<ModelsResponse>;
  }

  async health(): Promise<HealthResponse> {
    const resp = await this.fetchWithTimeout(`${this.apiUrl}/health`);
    if (!resp.ok) throw new Error(`Health check failed: ${resp.status}`);
    return resp.json() as Promise<HealthResponse>;
  }

  spendSummary(): SpendSummary {
    return {
      wallet_address: process.env['SOLANA_WALLET_ADDRESS'] ?? null,
      total_requests: this.requestCount,
      total_usdc_spent: this.sessionSpent.toFixed(6),
      session_usdc_spent: this.sessionSpent.toFixed(6),
      budget_remaining:
        this.sessionBudget !== undefined
          ? Math.max(0, this.sessionBudget - this.sessionSpent).toFixed(6)
          : null,
    };
  }

  // ---------------------------------------------------------------------------
  // Internals
  // ---------------------------------------------------------------------------

  // HF9: Translate AbortError to a descriptive timeout message.
  private async fetchWithTimeout(url: string, init?: RequestInit): Promise<Response> {
    const controller = new AbortController();
    const id = setTimeout(() => controller.abort(), this.timeoutMs);
    try {
      return await fetch(url, { ...init, signal: controller.signal });
    } catch (err) {
      if (controller.signal.aborted) {
        throw new Error(`Gateway request to ${url} timed out after ${this.timeoutMs}ms`);
      }
      throw err;
    } finally {
      clearTimeout(id);
    }
  }

  /**
   * Parse a 402 response body into a PaymentRequired object.
   * HF5: Throws with context instead of returning null.
   * The gateway wraps the JSON in error.message or returns it directly.
   */
  async parse402(resp: Response): Promise<PaymentRequired> {
    const raw = await resp.text();
    let body: unknown;
    try {
      body = JSON.parse(raw);
    } catch (e) {
      throw new Error(
        `Gateway returned 402 with non-JSON body (first 200 chars): ${raw.slice(0, 200)}`,
      );
    }
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    const typed: any = body;
    const msg = typed?.error?.message;
    if (typeof msg === 'string') {
      try {
        return JSON.parse(msg) as PaymentRequired;
      } catch (e) {
        throw new Error(
          `Gateway 402 error.message is not valid JSON (first 200 chars): ${msg.slice(0, 200)}`,
        );
      }
    }
    if (
      typed?.x402_version &&
      Array.isArray(typed?.accepts) &&
      typed.accepts.length > 0
    ) {
      return typed as PaymentRequired;
    }
    throw new Error(
      `Gateway 402 body missing x402_version/accepts (received keys: ${Object.keys(
        (body as object) ?? {},
      ).join(',')})`,
    );
  }

  /**
   * Filter the accepts array by signing mode before passing to the SDK signer.
   * In 'auto' mode, the SDK itself prefers escrow — no filtering needed.
   */
  private filterAccepts(paymentInfo: PaymentRequired): PaymentRequired {
    if (this.signingMode === 'auto') return paymentInfo;

    const filtered = paymentInfo.accepts.filter((a) => {
      if (this.signingMode === 'escrow') return a.scheme === 'escrow';
      if (this.signingMode === 'direct') return a.scheme !== 'escrow';
      return true;
    });

    if (filtered.length === 0) {
      throw new Error(
        `No payment accepts match signing mode '${this.signingMode}'. ` +
          'Gateway offered: ' + paymentInfo.accepts.map((a) => a.scheme).join(', '),
      );
    }

    return { ...paymentInfo, accepts: filtered };
  }
}
