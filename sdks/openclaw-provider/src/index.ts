/**
 * Solvela OpenClaw Provider Plugin
 *
 * Registers Solvela as a first-class LLM provider inside OpenClaw so users
 * can pick "solvela" in the model picker and the plugin signs every call
 * transparently via the x402 protocol.
 *
 * Environment variables required (set via OpenClaw's providerAuthEnvVars):
 *   SOLANA_WALLET_KEY   — base58-encoded 64-byte Solana keypair secret key
 *   SOLANA_RPC_URL      — Solana RPC endpoint (e.g. https://api.mainnet-beta.solana.com)
 *
 * Optional:
 *   SOLVELA_API_URL          — Gateway URL (default: https://api.solvela.ai)
 *   SOLVELA_SIGNING_MODE     — auto | escrow | direct (default: direct)
 *   SOLVELA_SESSION_BUDGET   — Max USDC per session (default: unlimited)
 *   SOLVELA_ALLOW_DEV_BYPASS — Set to "1" to permit probe-200 passthrough (dev only)
 *   SOLVELA_PROBE_TIMEOUT_MS — Probe fetch timeout in ms (default: 5000)
 *
 * Signing mode note (HF-P3-H6):
 *   Default signing mode is 'direct' in Phase 3. Escrow mode works but relies
 *   on gateway auto-claim after max_timeout_seconds. Set SOLVELA_SIGNING_MODE=escrow
 *   or SOLVELA_SIGNING_MODE=auto explicitly if you want escrow, accepting that
 *   deposits reconcile after gateway timeout until F4 (escrow-claim hook) ships.
 *
 * Design:
 *   The wrapStreamFn hook is used instead of prepareRuntimeAuth — per plan r1.3
 *   amendment 1. wrapStreamFn injects the payment-signature header directly into
 *   every outbound stream request. The gateway middleware/x402.rs:38 reads exactly
 *   this header. No Bearer token, no gateway changes required.
 *
 *   Unlike the MCP server's 402-retry-loop (fire → 402 → sign → retry), the Provider
 *   Plugin signs pro-actively before each call. This is required because the OpenClaw
 *   stream pipeline fires next() exactly once per call — there is no built-in retry
 *   mechanism. The plugin fetches a cost estimate from the gateway before each stream
 *   to obtain a PaymentRequired payload, then signs and injects the header.
 *
 * wrapStreamFn signature note (HF-P3-H1):
 *   The real OpenClaw SDK docs (https://docs.openclaw.ai/plugins/sdk-provider-plugins.md)
 *   confirm the factory shape: `(ctx) => async (params) => inner(params)`. Plan §10.5
 *   sample used a flat `(request, next)` shape — that section is stale.
 *   The types in openclaw-types.ts match the real SDK exactly.
 *
 * DO NOT: modify this to use prepareRuntimeAuth returning a Bearer token.
 *         That pattern was rejected in plan r1.3 amendment 1.
 */

import { SOLVELA_MODELS } from './models.generated.js';
import { SolvelaSigner } from './signer.js';
import { ROUTING_PROFILES, profileToCatalogEntry, resolveDynamicModel } from './registry.js';
import type {
  OpenClawApi,
  StreamFnContext,
  StreamFn,
  CatalogContext,
  DynamicModelContext,
} from './openclaw-types.js';
import type { PaymentRequired } from '@solvela/sdk/types';

export { SOLVELA_MODELS } from './models.generated.js';
export { ROUTING_PROFILES } from './registry.js';
export type { OpenClawApi } from './openclaw-types.js';

// ---------------------------------------------------------------------------
// Config from environment
// ---------------------------------------------------------------------------

/**
 * Validate and return the Solvela gateway URL (HF-P3-M4).
 *
 * Called per-use (not frozen at register() time) so config changes after
 * startup are respected and so a bad URL throws at call time rather than
 * killing the plugin load.
 */
function getApiUrl(): string {
  const raw = process.env['SOLVELA_API_URL'] ?? 'https://api.solvela.ai';
  let parsed: URL;
  try {
    parsed = new URL(raw);
  } catch {
    throw new Error(`SOLVELA_API_URL is not a valid URL: ${raw}`);
  }
  if (parsed.protocol !== 'https:' && parsed.protocol !== 'http:') {
    throw new Error(`SOLVELA_API_URL must use http or https, got ${parsed.protocol}`);
  }
  if (parsed.protocol === 'http:' && !/^(localhost|127\.|::1)/.test(parsed.hostname)) {
    process.stderr.write(
      '[solvela-openclaw] WARN: SOLVELA_API_URL uses plain http — HTTPS required for production\n',
    );
  }
  return (parsed.origin + parsed.pathname).replace(/\/$/, '');
}

/**
 * Return the signing mode from env (HF-P3-M8).
 *
 * Default is 'direct' (HF-P3-H6) — escrow mode relies on gateway auto-claim.
 * Unknown values throw hard — typos must not silently select the wrong mode.
 *
 * 'off' skips signing entirely — the request is forwarded without a
 * payment-signature header (the gateway will 402 unless it's in dev_bypass mode).
 * Parity with Phase 1 MCP server which also accepts 'off'.
 */
function getSigningMode(): 'auto' | 'escrow' | 'direct' | 'off' {
  const raw = process.env['SOLVELA_SIGNING_MODE'] ?? 'direct';
  if (raw === 'auto' || raw === 'escrow' || raw === 'direct' || raw === 'off') return raw;
  throw new Error(
    `SOLVELA_SIGNING_MODE='${raw}' is not recognized. Use auto|escrow|direct|off.`,
  );
}

function getSessionBudget(): number | undefined {
  const raw = process.env['SOLVELA_SESSION_BUDGET'];
  if (!raw) return undefined;
  const parsed = parseFloat(raw);
  if (!Number.isFinite(parsed) || parsed <= 0) {
    process.stderr.write(
      `[solvela-openclaw] WARN: invalid SOLVELA_SESSION_BUDGET='${raw}', ignoring\n`,
    );
    return undefined;
  }
  return parsed;
}

// ---------------------------------------------------------------------------
// Payment info fetcher
// ---------------------------------------------------------------------------

/**
 * Fetch a PaymentRequired payload from the gateway by making a probe request
 * to /v1/chat/completions without a payment header.
 *
 * The gateway returns 402 with the cost breakdown and accepted payment schemes.
 * This is the same first-leg of the MCP server's 402-retry-loop; here we do
 * it explicitly before calling next() because wrapStreamFn fires exactly once.
 *
 * Uses a configurable timeout (SOLVELA_PROBE_TIMEOUT_MS, default 5000ms) and
 * redirect: 'error' to prevent SSRF via 302 redirect (HF-P3-M1, HF-P3-M5).
 */
async function fetchPaymentRequired(
  apiUrl: string,
  body: string,
): Promise<PaymentRequired> {
  const url = `${apiUrl}/v1/chat/completions`;

  const timeoutMs = (() => {
    const raw = process.env['SOLVELA_PROBE_TIMEOUT_MS'];
    if (!raw) return 5000;
    const parsed = parseInt(raw, 10);
    return Number.isFinite(parsed) && parsed > 0 ? parsed : 5000;
  })();

  const controller = new AbortController();
  const timer = setTimeout(() => controller.abort(), timeoutMs);

  let resp: Response;
  try {
    resp = await fetch(url, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body,
      redirect: 'error',
      signal: controller.signal,
    });
  } catch (err) {
    clearTimeout(timer);
    if (err instanceof Error && err.name === 'AbortError') {
      throw new Error(`Solvela gateway probe timed out after ${timeoutMs}ms (${url})`);
    }
    throw err;
  }
  clearTimeout(timer);

  if (resp.status !== 402) {
    // Gateway may return 200 in dev_bypass_payment mode — no signing needed
    if (resp.ok) {
      throw new GatewayAcceptedWithoutPayment();
    }
    // Sanitize error body — redact any payment-signature fragments (HF-P3-M2)
    const text = (await resp.text().catch(() => '')).slice(0, 400);
    const sanitized = text.replace(/payment-signature[^\s,}"]+/gi, '[redacted]');
    throw new Error(`Unexpected gateway response ${resp.status}: ${sanitized}`);
  }

  return parsePaymentRequired(await resp.text());
}

/** Thrown when the gateway accepted the probe request without requiring payment. */
export class GatewayAcceptedWithoutPayment extends Error {
  constructor() {
    super('Gateway accepted request without payment (dev_bypass_payment mode?)');
    this.name = 'GatewayAcceptedWithoutPayment';
  }
}

function parsePaymentRequired(raw: string): PaymentRequired {
  let body: unknown;
  try {
    body = JSON.parse(raw);
  } catch {
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
    } catch {
      throw new Error(
        `Gateway 402 error.message is not valid JSON (first 200 chars): ${msg.slice(0, 200)}`,
      );
    }
  }

  if (typed?.x402_version && Array.isArray(typed?.accepts) && typed.accepts.length > 0) {
    // Validate cost is finite before returning (HF-P3-M9)
    const cost = parseFloat(typed.cost_breakdown?.total);
    if (!Number.isFinite(cost) || cost < 0) {
      throw new Error(`Gateway 402 has invalid cost: ${typed.cost_breakdown?.total}`);
    }
    return typed as PaymentRequired;
  }

  throw new Error(
    `Gateway 402 body missing x402_version/accepts (received keys: ${Object.keys(
      (body as object) ?? {},
    ).join(',')})`,
  );
}

// ---------------------------------------------------------------------------
// Build the full model catalog (real models + routing profiles)
// ---------------------------------------------------------------------------

function buildModelCatalog() {
  const profiles = ROUTING_PROFILES.map(profileToCatalogEntry);
  return [...profiles, ...SOLVELA_MODELS];
}

// ---------------------------------------------------------------------------
// Plugin registration — default export
// ---------------------------------------------------------------------------

/** Options accepted by register() — used for DI in tests. */
export interface RegisterOptions {
  /** Override the signer instance — for testing only. */
  _signer?: SolvelaSigner;
}

/**
 * Register the Solvela provider with the OpenClaw host.
 *
 * Called by OpenClaw when loading this plugin. The `api` object is injected
 * by the host at runtime.
 *
 * Throws if `api.registerProvider` is not a function — indicates an
 * incompatible OpenClaw SDK version (HF-P3-C3).
 *
 * The optional second argument accepts `_signer` for test DI — never set this
 * in production.
 */
export default function register(api: OpenClawApi, opts: RegisterOptions = {}): void {
  // Validate OpenClaw API surface at register() time (HF-P3-C3)
  if (!api || typeof api.registerProvider !== 'function') {
    throw new Error(
      'Solvela: OpenClaw API mismatch — api.registerProvider is not a function. ' +
        'Update @solvela/openclaw-provider to match your OpenClaw version.',
    );
  }

  const signingMode = getSigningMode();
  const sessionBudget = getSessionBudget();

  // 'off' mode: signer is unused (wrapStreamFn skips signing entirely).
  // Create one anyway so the type is consistent; it will never be called.
  const signer =
    opts._signer ??
    (signingMode !== 'off'
      ? new SolvelaSigner({ signingMode, sessionBudget })
      : new SolvelaSigner({ signingMode: 'direct', sessionBudget }));

  // Log at register time — but don't call getApiUrl() here to avoid killing
  // plugin load on a bad URL; per-call getApiUrl() gives a better error context.
  process.stderr.write(
    `[solvela-openclaw] Registering provider: signingMode=${signingMode}\n`,
  );

  api.registerProvider({
    id: 'solvela',
    label: 'Solvela — USDC-gated multi-provider gateway',
    docsPath: 'https://docs.solvela.ai/openclaw',
    envVars: ['SOLANA_WALLET_KEY', 'SOLANA_RPC_URL'],
    auth: [{ method: 'api-key', envVar: 'SOLANA_WALLET_KEY' }],

    catalog: {
      order: 'late',
      run: async (_ctx: CatalogContext): Promise<{
        provider: {
          baseUrl: string;
          apiKey: string;
          api: 'openai-completions';
          models: ReturnType<typeof buildModelCatalog>;
        };
      }> => {
        const apiUrl = getApiUrl();

        // Validate that signing credentials are present (HF-P3-H5)
        const walletKey = process.env['SOLANA_WALLET_KEY'];
        if (!walletKey) {
          process.stderr.write(
            '[solvela-openclaw] WARN: SOLANA_WALLET_KEY not set — provider unavailable\n',
          );
          // Return a diagnostic shadow model so the user sees a clear message
          // in the model picker instead of Solvela simply disappearing (HF-P3-H5).
          return {
            provider: {
              baseUrl: `${apiUrl}/v1`,
              apiKey: '',
              api: 'openai-completions',
              models: [
                {
                  id: 'solvela/not-configured',
                  name: 'Solvela — not configured (set SOLANA_WALLET_KEY)',
                  provider: 'solvela',
                  contextWindow: 0,
                  maxTokens: 0,
                  inputCostPerMillion: 0,
                  outputCostPerMillion: 0,
                  supportsStreaming: false,
                },
              ],
            },
          };
        }

        return {
          provider: {
            baseUrl: `${apiUrl}/v1`,
            // apiKey is empty — real signing happens per-call via wrapStreamFn (HF-P3-L5)
            apiKey: '',
            api: 'openai-completions',
            models: buildModelCatalog(),
          },
        };
      },
    },

    resolveDynamicModel: (ctx: DynamicModelContext) => {
      // Map routing profiles and model IDs to their gateway form.
      // For unknown solvela/* IDs, throw with a helpful suggestion (HF-P3-H4).
      // Non-solvela/ IDs (real model IDs like gpt-4o) pass through as-is.
      const modelId = ctx.modelId;

      if (modelId.startsWith('solvela/')) {
        const resolved = resolveDynamicModel(modelId);
        if (resolved === modelId) {
          // resolveDynamicModel returned the input unchanged — unknown solvela/ ID
          const knownProfiles = ROUTING_PROFILES.map((p) => p.id).join(', ');
          throw new Error(
            `Unknown Solvela profile '${modelId}'. Known profiles: ${knownProfiles}. ` +
              'For direct model IDs, omit the solvela/ prefix (e.g. "gpt-4o", "claude-sonnet-4-20250514").',
          );
        }
        return { id: resolved, name: ctx.modelId };
      }

      // Direct model ID — pass through (gateway resolves directly)
      return {
        id: modelId,
        name: modelId,
      };
    },

    /**
     * wrapStreamFn — per-call payment-signature injection.
     *
     * The hook receives a context containing the existing stream function and
     * returns a wrapped version that injects the PAYMENT-SIGNATURE header
     * before calling the original stream function.
     *
     * Signature note (HF-P3-H1): The real OpenClaw SDK docs confirm the factory
     * shape `(ctx) => async (params) => inner(params)`. Plan §10.5 used a flat
     * `(request, next)` shape — that section is stale. This implementation is correct.
     *
     * Flow:
     *   1. Validate ctx.streamFn — throw if absent (HF-P3-C3)
     *   2. Serialize request body once as canonical JSON string (HF-P3-C1)
     *   3. Write canonical body back to params (HF-P3-C1 — ensures probe and inner() match)
     *   4. Probe the gateway with the canonical body to get a 402 PaymentRequired
     *   5. Call signer.buildHeader() to produce the base64 payment-signature
     *   6. Inject `params.headers['payment-signature'] = header`
     *   7. Call inner(params); on failure → refund budget (HF-P3-C2)
     *
     * solvela/not-configured guard: if the resolved model is the diagnostic shadow,
     * throw a clear config error rather than attempting to sign.
     *
     * DO NOT change this to use Authorization: Bearer — the gateway middleware
     * reads 'payment-signature' (lowercase), not an Authorization header.
     */
    wrapStreamFn: (ctx: StreamFnContext): StreamFn => {
      // Fail loud — silently returning undefined allows OpenClaw to call the
      // inner stream without a payment header (HF-P3-C3).
      if (!ctx?.streamFn || typeof ctx.streamFn !== 'function') {
        throw new Error(
          'Solvela: wrapStreamFn invoked without a valid streamFn. ' +
            'This likely means the OpenClaw SDK version is incompatible. ' +
            'Verify @solvela/openclaw-provider is up to date.',
        );
      }

      const inner = ctx.streamFn;

      return async (params) => {
        // 'off' mode: skip signing entirely — forward to inner() without a payment header.
        // Gateway will 402 unless it's in dev_bypass_payment mode. Parity with Phase 1 MCP.
        if (signingMode === 'off') {
          process.stderr.write(
            '[solvela-openclaw] WARN: signingMode=off — forwarding request without payment header.\n',
          );
          return inner(params);
        }

        // solvela/not-configured guard (HF-P3-H5)
        const resolvedModelId =
          typeof ctx.model?.id === 'string' ? ctx.model.id : undefined;
        if (resolvedModelId === 'solvela/not-configured') {
          throw new Error(
            'Solvela: SOLANA_WALLET_KEY is not set. ' +
              'Set SOLANA_WALLET_KEY to your base58-encoded Solana wallet key to use Solvela.',
          );
        }

        // Step 2+3: Serialize body once and write back to params as canonical string (HF-P3-C1).
        // Both probe and inner() see byte-identical bodies — prevents PDA derivation mismatch.
        const requestBody =
          typeof params.body === 'string' ? params.body : JSON.stringify(params.body ?? {});
        params.body = requestBody;

        const apiUrl = getApiUrl();
        const resourceUrl =
          typeof params.url === 'string' ? params.url : `${apiUrl}/v1/chat/completions`;

        // Steps 4+5: Probe and sign
        let paymentHeader: string;
        let cost: number;
        try {
          const paymentInfo = await fetchPaymentRequired(apiUrl, requestBody);
          cost = parseFloat(paymentInfo.cost_breakdown?.total ?? 'NaN');
          paymentHeader = await signer.buildHeader(paymentInfo, resourceUrl, requestBody);
        } catch (err) {
          if (err instanceof GatewayAcceptedWithoutPayment) {
            // dev_bypass_payment mode — only allowed with explicit opt-in (HF-P3-C4)
            if (process.env['SOLVELA_ALLOW_DEV_BYPASS'] !== '1') {
              throw new Error(
                'Solvela: gateway probe returned 200 unexpectedly (no 402 envelope). ' +
                  'This may indicate caching or a dev_bypass_payment gateway. ' +
                  'Set SOLVELA_ALLOW_DEV_BYPASS=1 if intentional; otherwise check your SOLVELA_API_URL.',
              );
            }
            process.stderr.write(
              '[solvela-openclaw] WARN: Gateway accepted without payment (SOLVELA_ALLOW_DEV_BYPASS=1). No signing this call.\n',
            );
            return inner(params);
          }
          // Surface signing failures clearly — err.message is safe (SDK strips key bytes)
          throw new Error(
            `Solvela payment signing failed: ${err instanceof Error ? err.message : String(err)}`,
          );
        }

        // Step 6: Inject payment-signature header (lowercase, matching middleware/x402.rs:38)
        params.headers = {
          ...params.headers,
          'payment-signature': paymentHeader,
        };

        // Step 7: Call inner with refund on failure (HF-P3-C2).
        // If inner() throws (network, gateway 500, stream error), the session budget
        // is refunded — the real call never succeeded.
        try {
          const result = await inner(params);
          return result;
        } catch (err) {
          // Refund — the real call never succeeded
          await signer.refundBudget(cost);
          throw err;
        }
      };
    },
  });
}
