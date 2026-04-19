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
import type { SessionStore, SessionState } from './session.js';
import { parse402, filterAccepts, isStubHeader, sanitizeGatewayError } from '@solvela/signer-core';

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
  /** Optional session persistence store. When provided, sessionSpent survives restarts. */
  sessionStore?: SessionStore;
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
  /**
   * Integer micro-USDC counters (1 USDC = 1_000_000 micro-USDC).
   * Using integers prevents floating-point drift (e.g. 7 × 0.1 ≠ 0.7 in IEEE-754).
   * Public API getters return floats (divided by 1_000_000) for display compatibility.
   * Persistence file stores floats; load path converts back via Math.round().
   */
  private sessionSpentMicro = 0;
  private escrowDepositsSessionMicro = 0;
  private requestCount = 0;
  /** Mutex ensures budget check + increment is atomic across parallel chat calls (T1-H). */
  private readonly budgetMutex = new Mutex();
  /** Optional session persistence. */
  private readonly sessionStore?: SessionStore;
  /**
   * Lazy-init promise: resolves to the persisted state on first mutex entry.
   * Initialized in constructor so the load runs once, not per-call.
   */
  private readonly sessionStatePromise: Promise<SessionState | null>;
  private sessionStateLoaded = false;

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
    this.sessionStore = opts.sessionStore;
    // Kick off the load immediately so the first mutex entry doesn't add latency.
    this.sessionStatePromise = opts.sessionStore
      ? opts.sessionStore.load().catch((err) => {
          process.stderr.write(
            `[solvela-mcp] WARN: failed to load session state: ${err instanceof Error ? err.message : String(err)}\n`,
          );
          return null;
        })
      : Promise.resolve(null);
  }

  /**
   * Apply persisted state to in-memory counters on first use.
   * Must be called WITHIN the budgetMutex to avoid races.
   */
  private async applyPersistedStateOnce(): Promise<void> {
    if (this.sessionStateLoaded) return;
    this.sessionStateLoaded = true;
    const state = await this.sessionStatePromise;
    if (state !== null) {
      // Convert float USDC from persistence to integer micro-USDC for internal accounting.
      this.sessionSpentMicro = Math.round(state.session_spent * 1_000_000);
      this.escrowDepositsSessionMicro = Math.round(state.escrow_deposits_session * 1_000_000);
      this.requestCount = state.request_count;
    }
  }

  /**
   * Persist current in-memory state. Must be called WITHIN the budgetMutex.
   *
   * @param critical When true, a disk/permission failure throws instead of logging WARN.
   *   Pass critical=true for escrow deposit accounting paths where a lost persist means
   *   the session cap may be stale after restart. Pass critical=false (default) for chat
   *   budget paths where the tool response is already the paid outcome.
   */
  private async persistState(critical = false): Promise<void> {
    if (!this.sessionStore) return;
    const state: SessionState = {
      // Convert integer micro-USDC back to float USDC for human-readable persistence.
      session_spent: this.sessionSpentMicro / 1_000_000,
      escrow_deposits_session: this.escrowDepositsSessionMicro / 1_000_000,
      request_count: this.requestCount,
      last_updated: new Date().toISOString(),
      version: 1,
    };
    try {
      await this.sessionStore.save(state);
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      process.stderr.write(
        `[solvela-mcp] WARN: failed to persist session state: ${msg}\n`,
      );
      if (critical) {
        throw new Error(
          `Failed to persist escrow deposit accounting: ${msg}. ` +
          `Session cap may be stale after restart. Fix the disk/permission issue and retry.`,
        );
      }
    }
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
      // HF5: parse402 throws instead of returning null.
      const paymentInfo = parse402(await resp.text());

      const cost = parseFloat(paymentInfo.cost_breakdown.total);

      // T1-H: Atomic budget reservation — both check and increment must be in the same
      // critical section to prevent two parallel calls from both passing the budget check.
      // Compare in integer micro-USDC space to avoid float drift.
      const costMicro = Math.round(cost * 1_000_000);
      await this.budgetMutex.runExclusive(async () => {
        await this.applyPersistedStateOnce();
        if (this.sessionBudget !== undefined) {
          const budgetMicro = Math.round(this.sessionBudget * 1_000_000);
          if (this.sessionSpentMicro + costMicro > budgetMicro) {
            throw new Error(
              `Session budget $${this.sessionBudget.toFixed(6)} USDC exceeded ` +
                `(spent: $${(this.sessionSpentMicro / 1_000_000).toFixed(6)}, request cost: $${cost.toFixed(6)})`,
            );
          }
        }
        this.sessionSpentMicro += costMicro;
        await this.persistState();
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
        const filteredAccepts = filterAccepts(
          paymentInfo.accepts,
          this.signingMode as 'auto' | 'escrow' | 'direct',
        );
        const filteredPaymentInfo = { ...paymentInfo, accepts: filteredAccepts };
        const privateKey = process.env['SOLANA_WALLET_KEY'];

        let paymentHeader: string;
        try {
          paymentHeader = await createPaymentHeader(filteredPaymentInfo, url, privateKey, bodyStr);
        } catch (err) {
          // HF2: Refund the reserved budget — signer never succeeded.
          await this.budgetMutex.runExclusive(async () => {
            const before = this.sessionSpentMicro;
            if (before > 0 && before < costMicro) {
              // Math.max(0, ...) would clamp — indicates a race condition in budget accounting.
              process.stderr.write(
                `[solvela-mcp] WARN: budget refund clamped to 0 (before=${before}, costMicro=${costMicro}) — possible race condition\n`,
              );
            }
            this.sessionSpentMicro = Math.max(0, this.sessionSpentMicro - costMicro);
            await this.persistState();
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
        if (privateKey !== undefined && isStubHeader(paymentHeader)) {
          // Refund the reserved budget before throwing.
          await this.budgetMutex.runExclusive(async () => {
            this.sessionSpentMicro = Math.max(0, this.sessionSpentMicro - costMicro);
            await this.persistState();
          });
          throw new Error(
            'Payment signing returned a stub transaction. This means signing silently ' +
            'degraded to stub mode — likely @solana/web3.js or peer deps are unresolvable ' +
            'at runtime. Reinstall @solvela/mcp-server and retry.',
          );
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
            this.sessionSpentMicro = Math.max(0, this.sessionSpentMicro - costMicro);
            await this.persistState();
          });
          throw err;
        }

        // HF2: If retry returns non-2xx, refund before surfacing error.
        if (!retryResp.ok) {
          await this.budgetMutex.runExclusive(async () => {
            this.sessionSpentMicro = Math.max(0, this.sessionSpentMicro - costMicro);
            await this.persistState();
          });
          // HF8: Truncate + sanitize gateway error body.
          const sanitized = sanitizeGatewayError(await retryResp.text().catch(() => ''));
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
      const sanitized = sanitizeGatewayError(await resp.text().catch(() => ''));
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
    const sessionSpent = this.sessionSpentMicro / 1_000_000;
    return {
      wallet_address: process.env['SOLANA_WALLET_ADDRESS'] ?? null,
      total_requests: this.requestCount,
      total_usdc_spent: sessionSpent.toFixed(6),
      session_usdc_spent: sessionSpent.toFixed(6),
      budget_remaining:
        this.sessionBudget !== undefined
          ? Math.max(0, this.sessionBudget - sessionSpent).toFixed(6)
          : null,
    };
  }

  /** Current cumulative escrow deposits this session (USDC float for display). */
  getEscrowDepositsSession(): number {
    return this.escrowDepositsSessionMicro / 1_000_000;
  }

  /**
   * Atomically reserve escrow deposit capacity, broadcast the deposit outside the mutex,
   * then roll back on failure. Returns the new cumulative total (USDC float).
   *
   * Race safety: Phase 1 atomically reserves capacity AND increments the counter so that
   * two concurrent calls cannot both pass the cap check for the same budget window.
   * Phase 2 (broadcast) runs outside the mutex. If broadcast throws, Phase 3-refund
   * rolls back the reservation under the mutex.
   *
   * Persist policy: persistState(critical=true) on Phase-1 reserve and Phase-3 refund
   * so that a disk failure surfaces immediately rather than silently leaving cap stale.
   *
   * Count-on-broadcast policy: onDeposit() MUST throw ONLY if sendRawTransaction itself
   * fails. Confirmation timeout is NOT a throw from onDeposit — it must be signalled via
   * a side-channel (e.g. returned flag) so the reservation is not rolled back.
   *
   * @param amount            USDC amount to deposit (positive finite number)
   * @param maxSessionDeposit Session cap in USDC (e.g. 20.0)
   * @param onDeposit         Async callback that performs the broadcast. Must throw only if
   *                          the transaction was never broadcast (pre-broadcast failure).
   */
  async runEscrowDeposit(
    amount: number,
    maxSessionDeposit: number,
    onDeposit: () => Promise<void>,
  ): Promise<number> {
    const amountMicro = Math.round(amount * 1_000_000);
    const maxSessionMicro = Math.round(maxSessionDeposit * 1_000_000);

    // Phase 1: atomically reserve capacity — check AND increment in one mutex section.
    await this.budgetMutex.runExclusive(async () => {
      await this.applyPersistedStateOnce();
      if (this.escrowDepositsSessionMicro + amountMicro > maxSessionMicro) {
        throw new Error(
          `Escrow session cap exceeded: cumulative $${((this.escrowDepositsSessionMicro + amountMicro) / 1_000_000).toFixed(6)} ` +
          `would exceed SOLVELA_MAX_ESCROW_SESSION=$${maxSessionDeposit.toFixed(6)}`,
        );
      }
      this.escrowDepositsSessionMicro += amountMicro;
      // critical=true: a disk failure here means the cap accounting is broken.
      await this.persistState(true);
    });

    // Phase 2: broadcast outside the mutex (may take seconds for RPC confirmation).
    try {
      await onDeposit();
    } catch (err) {
      // Phase 3-refund: reservation never landed on-chain; roll back.
      await this.budgetMutex.runExclusive(async () => {
        const before = this.escrowDepositsSessionMicro;
        if (before > 0 && before < amountMicro) {
          process.stderr.write(
            `[solvela-mcp] WARN: escrow refund clamped (before=$${(before / 1_000_000).toFixed(6)}, refund=$${amount.toFixed(6)}). ` +
            `Possible race condition.\n`,
          );
        }
        this.escrowDepositsSessionMicro = Math.max(0, this.escrowDepositsSessionMicro - amountMicro);
        // critical=true: a disk failure here means the cap may be overstated after restart.
        await this.persistState(true);
      });
      throw err;
    }

    return this.escrowDepositsSessionMicro / 1_000_000;
  }

  /**
   * Reset in-memory session counters to zero and delete the persisted session file.
   * Both operations happen inside the mutex so a concurrent Phase-3 commit cannot
   * persist stale data between the zero and the file deletion.
   * Used by the `spending --reset` tool flag.
   */
  async resetSession(): Promise<void> {
    await this.budgetMutex.runExclusive(async () => {
      this.sessionSpentMicro = 0;
      this.escrowDepositsSessionMicro = 0;
      this.requestCount = 0;
      this.sessionStateLoaded = true; // prevent applyPersistedStateOnce from re-loading
      if (this.sessionStore) {
        await this.sessionStore.reset();
      }
    });
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

}
