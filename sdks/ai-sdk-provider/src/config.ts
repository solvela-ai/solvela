/**
 * Configuration validation for SolvelaProviderSettings.
 *
 * validateSettings() is the single entry point — call it at provider
 * construction time. Throws SolvelaInvalidConfigError on any violation.
 */

import { z } from 'zod';

import { SolvelaInvalidConfigError } from './errors.js';
import type { SolvelaWalletAdapter } from './wallet-adapter.js';
import { warnOnce } from './util/warn-once.js';
import type { FetchFunction } from '@ai-sdk/provider-utils';

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const DEFAULT_BASE_URL = 'https://api.solvela.ai/v1';
const V1_SUFFIX = '/v1';
const PAYMENT_SIGNATURE_LOWER = 'payment-signature';
const WARN_PAYMENT_SIG_FILTERED =
  '[solvela] PAYMENT-SIGNATURE header was supplied in `headers` and has been ' +
  'removed. The provider manages this header internally via the wallet adapter.';
const DEFAULT_MAX_BODY_BYTES = 1_000_000;

// ---------------------------------------------------------------------------
// Input settings (public API type)
// ---------------------------------------------------------------------------

/**
 * Settings accepted by createSolvelaProvider(). All fields optional except
 * `wallet`, which is required — there is no escape hatch for raw key bytes.
 */
export interface SolvelaProviderSettings {
  /**
   * Gateway base URL. Resolved in order:
   *   1. this field
   *   2. process.env.SOLVELA_API_URL
   *   3. https://api.solvela.ai/v1 (default)
   *
   * The resolver ensures the URL ends with /v1 (appends if absent).
   */
  baseURL?: string;

  /**
   * Optional per-wallet API key forwarded as an Authorization header.
   * NOT a signing key — never used to derive payment signatures.
   */
  apiKey?: string;

  /**
   * REQUIRED. Adapter implementing SolvelaWalletAdapter.
   * No escape hatch — every signer is an adapter.
   * For dev/test, import createLocalWalletAdapter from
   * @solvela/ai-sdk-provider/adapters/local.
   */
  wallet: SolvelaWalletAdapter;

  /**
   * Optional static headers merged into every request.
   * PAYMENT-SIGNATURE is filtered out if present (emits a one-time warn).
   */
  headers?: Record<string, string>;

  /**
   * Session budget in USDC atomic units. Throws SolvelaBudgetExceededError
   * when exhausted. Overridable via SOLVELA_SESSION_BUDGET env var.
   */
  sessionBudget?: bigint;

  /**
   * Cap on init.body length (bytes) before signing. Default 1_000_000.
   * Overridable via SOLVELA_MAX_SIGNED_BODY_BYTES env var.
   */
  maxBodyBytes?: number;

  /**
   * If true, allows a non-HTTPS baseURL (useful for localhost tests).
   * Has no effect in production environments or on Vercel Edge.
   * See also: SOLVELA_ALLOW_INSECURE_BASE_URL env var (env var variant
   * is refused in production/Edge regardless of this setting).
   */
  allowInsecureBaseURL?: boolean;

  /**
   * Override the underlying fetch function (for tests / observability).
   * Phase 3 replaces the placeholder wrapper with the real 402-retry logic.
   */
  fetch?: FetchFunction;

  /**
   * Forwarded to createOpenAICompatible. Default false — many upstream models
   * do not support structured outputs.
   */
  supportsStructuredOutputs?: boolean;
}

// ---------------------------------------------------------------------------
// Normalized settings (returned by validateSettings)
// ---------------------------------------------------------------------------

/**
 * Post-validation, normalized settings ready for use in the provider factory.
 */
export interface NormalizedSettings {
  /** Fully-normalized base URL ending in /v1 (no trailing slash). */
  baseURL: string;
  apiKey: string | undefined;
  wallet: SolvelaWalletAdapter;
  /** Filtered headers — PAYMENT-SIGNATURE removed. */
  headers: Record<string, string>;
  sessionBudget: bigint | undefined;
  maxBodyBytes: number;
  allowInsecureBaseURL: boolean;
  fetch: FetchFunction | undefined;
  supportsStructuredOutputs: boolean;
}

// ---------------------------------------------------------------------------
// Zod schema (validates the input shape)
// ---------------------------------------------------------------------------

/**
 * Zod schema for SolvelaProviderSettings. Used in validateSettings().
 * wallet validation uses z.custom — the adapter interface is structural.
 */
export const solvelaProviderSettingsSchema = z.object({
  baseURL: z.string().optional(),
  apiKey: z.string().optional(),
  wallet: z.custom<SolvelaWalletAdapter>(
    (v) =>
      typeof v === 'object' &&
      v !== null &&
      typeof (v as Record<string, unknown>)['signPayment'] === 'function' &&
      typeof (v as Record<string, unknown>)['label'] === 'string',
    { error: 'wallet must implement SolvelaWalletAdapter (label: string, signPayment: function)' },
  ),
  headers: z.record(z.string(), z.string()).optional(),
  sessionBudget: z.custom<bigint>((v) => typeof v === 'bigint').optional(),
  maxBodyBytes: z.number().int().positive().optional(),
  allowInsecureBaseURL: z.boolean().optional(),
  fetch: z.custom<FetchFunction>((v) => typeof v === 'function').optional(),
  supportsStructuredOutputs: z.boolean().optional(),
});

// ---------------------------------------------------------------------------
// URL normalization
// ---------------------------------------------------------------------------

/**
 * Normalize a raw baseURL:
 *  1. Strip trailing slash.
 *  2. If not ending in /v1, append /v1.
 * Result is the exact origin that createOpenAICompatible will append
 * /chat/completions to.
 */
function normalizeBaseURL(raw: string): string {
  let url = raw.endsWith('/') ? raw.slice(0, -1) : raw;
  if (!url.endsWith(V1_SUFFIX)) {
    url = url + V1_SUFFIX;
  }
  return url;
}

// ---------------------------------------------------------------------------
// HTTPS enforcement
// ---------------------------------------------------------------------------

/**
 * Returns true when the current environment is production or Vercel Edge.
 * TLS is always enforced in these environments.
 */
function isProductionOrEdge(): boolean {
  if (
    typeof process !== 'undefined' &&
    (process.env['NODE_ENV'] === 'production' ||
      process.env['VERCEL_ENV'] === 'production')
  ) {
    return true;
  }
  // Vercel Edge Runtime exposes the `EdgeRuntime` global
  if (typeof globalThis !== 'undefined' && 'EdgeRuntime' in globalThis) {
    return true;
  }
  return false;
}

/**
 * Returns true when the env-var insecure override should be applied:
 * SOLVELA_ALLOW_INSECURE_BASE_URL=true AND not in production/Edge.
 *
 * Per §4.3 T2-H: if production/Edge and the env var is set, emit
 * console.error and REFUSE to apply (TLS remains enforced).
 */
function resolveEnvInsecureFlag(): boolean {
  const envSet =
    typeof process !== 'undefined' &&
    process.env['SOLVELA_ALLOW_INSECURE_BASE_URL'] === 'true';
  if (!envSet) return false;

  if (isProductionOrEdge()) {
    console.error(
      '[solvela] SOLVELA_ALLOW_INSECURE_BASE_URL=true is set but the current ' +
        'environment is production or Vercel Edge. The insecure override is ' +
        'REFUSED — TLS remains enforced.',
    );
    return false;
  }
  return true;
}

/**
 * Returns true when the test-mode env allows non-HTTPS:
 * NODE_ENV=test AND SOLVELA_AI_SDK_PROVIDER_TEST_MODE=true AND
 * baseURL hostname is localhost or 127.0.0.1.
 */
function isTestModeAllowed(baseURL: string): boolean {
  if (
    typeof process === 'undefined' ||
    process.env['NODE_ENV'] !== 'test' ||
    process.env['SOLVELA_AI_SDK_PROVIDER_TEST_MODE'] !== 'true'
  ) {
    return false;
  }
  try {
    const { hostname } = new URL(baseURL);
    return hostname === 'localhost' || hostname === '127.0.0.1';
  } catch {
    return false;
  }
}

/**
 * Enforce HTTPS on the normalized baseURL. Throws SolvelaInvalidConfigError
 * if the URL uses a non-HTTPS scheme and none of the allowed exceptions apply.
 *
 * Exceptions (in order of precedence):
 *  1. allowInsecureBaseURL setting === true (explicit opt-in, refused in prod/Edge)
 *  2. SOLVELA_ALLOW_INSECURE_BASE_URL=true env var (refused in prod/Edge)
 *  3. Test-mode: NODE_ENV=test + SOLVELA_AI_SDK_PROVIDER_TEST_MODE=true + localhost
 */
function assertSecureBaseURL(
  baseURL: string,
  allowInsecureBaseURL: boolean,
): void {
  try {
    const { protocol } = new URL(baseURL);
    if (protocol === 'https:') return; // always OK
  } catch {
    throw new SolvelaInvalidConfigError({
      message: `[solvela] Invalid baseURL — could not parse as URL: ${baseURL}`,
    });
  }

  // Non-HTTPS: check exceptions
  if (allowInsecureBaseURL) {
    if (isProductionOrEdge()) {
      console.error(
        '[solvela] allowInsecureBaseURL is set but the current environment is ' +
          'production or Vercel Edge. The insecure override is REFUSED — TLS remains enforced.',
      );
      // Fall through to the HTTPS rejection path below.
    } else {
      return;
    }
  }
  if (resolveEnvInsecureFlag()) return;
  if (isTestModeAllowed(baseURL)) return;

  throw new SolvelaInvalidConfigError({
    message:
      `[solvela] Non-HTTPS baseURL "${baseURL}" is not allowed. ` +
      'Set allowInsecureBaseURL: true in settings (non-production only), ' +
      'or set SOLVELA_ALLOW_INSECURE_BASE_URL=true (refused in production/Edge), ' +
      'or use NODE_ENV=test + SOLVELA_AI_SDK_PROVIDER_TEST_MODE=true with localhost.',
  });
}

// ---------------------------------------------------------------------------
// Header filtering
// ---------------------------------------------------------------------------

/**
 * Strip any PAYMENT-SIGNATURE key (case-insensitive) from `headers`.
 * Emits a one-time console.warn via warnOnce if the header was present.
 */
function filterHeaders(
  headers: Record<string, string> | undefined,
): Record<string, string> {
  if (!headers) return {};
  const result: Record<string, string> = {};
  let found = false;
  for (const [key, value] of Object.entries(headers)) {
    if (key.toLowerCase() === PAYMENT_SIGNATURE_LOWER) {
      found = true;
    } else {
      result[key] = value;
    }
  }
  if (found) {
    warnOnce(WARN_PAYMENT_SIG_FILTERED);
  }
  return result;
}

// ---------------------------------------------------------------------------
// Env var overrides
// ---------------------------------------------------------------------------

function resolveSessionBudget(
  settingValue: bigint | undefined,
): bigint | undefined {
  if (settingValue !== undefined) return settingValue;
  const raw =
    typeof process !== 'undefined'
      ? process.env['SOLVELA_SESSION_BUDGET']
      : undefined;
  if (!raw) return undefined;
  try {
    return BigInt(raw);
  } catch {
    throw new SolvelaInvalidConfigError({
      message: `[solvela] SOLVELA_SESSION_BUDGET env var is not a valid integer: "${raw}"`,
    });
  }
}

function resolveMaxBodyBytes(settingValue: number | undefined): number {
  if (settingValue !== undefined) return settingValue;
  const raw =
    typeof process !== 'undefined'
      ? process.env['SOLVELA_MAX_SIGNED_BODY_BYTES']
      : undefined;
  if (!raw) return DEFAULT_MAX_BODY_BYTES;
  const parsed = parseInt(raw, 10);
  if (isNaN(parsed) || parsed <= 0) {
    throw new SolvelaInvalidConfigError({
      message: `[solvela] SOLVELA_MAX_SIGNED_BODY_BYTES env var is not a valid positive integer: "${raw}"`,
    });
  }
  return parsed;
}

// ---------------------------------------------------------------------------
// Main validation entry point
// ---------------------------------------------------------------------------

/**
 * Validate and normalize raw provider settings.
 *
 * Throws SolvelaInvalidConfigError on any validation failure.
 * Must be called at provider construction time (not lazily on first request).
 *
 * @param input - Raw settings object (possibly untrusted — validated via zod).
 */
export function validateSettings(input: unknown): NormalizedSettings {
  // 1. Zod structural validation
  const parseResult = solvelaProviderSettingsSchema.safeParse(input);
  if (!parseResult.success) {
    const issues = parseResult.error.issues
      .map((i) => `${i.path.join('.') || '(root)'}: ${i.message}`)
      .join('; ');
    throw new SolvelaInvalidConfigError({
      message: `[solvela] Invalid provider configuration: ${issues}`,
    });
  }

  const settings = parseResult.data;

  // 2. Resolve baseURL (explicit > env > default)
  const rawBaseURL =
    settings.baseURL ??
    (typeof process !== 'undefined' ? process.env['SOLVELA_API_URL'] : undefined) ??
    DEFAULT_BASE_URL;

  const baseURL = normalizeBaseURL(rawBaseURL);

  // 3. HTTPS enforcement
  const allowInsecure = settings.allowInsecureBaseURL ?? false;
  assertSecureBaseURL(baseURL, allowInsecure);

  // 4. Filter headers (remove PAYMENT-SIGNATURE)
  const filteredHeaders = filterHeaders(settings.headers);

  // 5. Resolve env-var overrides
  const sessionBudget = resolveSessionBudget(settings.sessionBudget);
  const maxBodyBytes = resolveMaxBodyBytes(settings.maxBodyBytes);

  return {
    baseURL,
    apiKey: settings.apiKey,
    wallet: settings.wallet,
    headers: filteredHeaders,
    sessionBudget,
    maxBodyBytes,
    allowInsecureBaseURL: allowInsecure,
    fetch: settings.fetch,
    supportsStructuredOutputs: settings.supportsStructuredOutputs ?? false,
  };
}
