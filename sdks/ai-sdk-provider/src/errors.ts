/**
 * Typed error classes for the Solvela AI SDK provider.
 *
 * All payment/HTTP-surface errors extend `APICallError` from `@ai-sdk/provider`
 * so that `APICallError.isInstance(err)` returns `true` for every Solvela error
 * that is API-surface related.
 *
 * `SolvelaInvalidConfigError` extends `AISDKError` because it is a
 * construction-time error that has no associated HTTP request.
 *
 * Each class carries its own `Symbol.for(...)` marker so cross-package
 * `instanceof` checks work regardless of module deduplication.
 */

import { AISDKError, APICallError } from '@ai-sdk/provider';

import {
  redactBase58,
  redactHex,
  sanitizeError,
  stripPaymentSignature,
} from './util/redact.js';

// ---------------------------------------------------------------------------
// Private marker symbols — one per class
// ---------------------------------------------------------------------------

const PAYMENT_ERROR_MARKER = Symbol.for('solvela.ai.error.SolvelaPaymentError');
const BUDGET_ERROR_MARKER = Symbol.for('solvela.ai.error.SolvelaBudgetExceededError');
const SIGNING_ERROR_MARKER = Symbol.for('solvela.ai.error.SolvelaSigningError');
const UPSTREAM_ERROR_MARKER = Symbol.for('solvela.ai.error.SolvelaUpstreamError');
const INVALID_CONFIG_MARKER = Symbol.for('solvela.ai.error.SolvelaInvalidConfigError');

// ---------------------------------------------------------------------------
// Shared parameter types
// ---------------------------------------------------------------------------

/** Parameters shared by all APICallError-based Solvela errors. */
interface SolvelaAPIErrorParams {
  message: string;
  url: string;
  requestBodyValues: unknown;
  statusCode?: number;
  responseHeaders?: Record<string, string>;
  responseBody?: string;
  cause?: unknown;
}

// ---------------------------------------------------------------------------
// SolvelaPaymentError
// ---------------------------------------------------------------------------

/**
 * Raised when a payment verification or processing step fails (HTTP 402 /
 * on-chain rejection). Never retryable — the caller must build a new payment.
 */
export class SolvelaPaymentError extends APICallError {
  static override isInstance(
    error: unknown,
  ): error is SolvelaPaymentError {
    return (
      error != null &&
      typeof error === 'object' &&
      (error as Record<symbol, unknown>)[PAYMENT_ERROR_MARKER] === true
    );
  }

  constructor(params: SolvelaAPIErrorParams) {
    const clean = sanitizeError({
      ...params,
      responseHeaders: params.responseHeaders
        ? stripPaymentSignature(params.responseHeaders)
        : undefined,
    });
    super({ ...clean, isRetryable: false });
    (this as Record<symbol, unknown>)[PAYMENT_ERROR_MARKER] = true;
    this.name = 'SolvelaPaymentError';
  }
}

// ---------------------------------------------------------------------------
// SolvelaBudgetExceededError
// ---------------------------------------------------------------------------

/**
 * Raised when the request exceeds the agent's configured spending budget.
 * Never retryable — the caller must increase the budget or reduce request size.
 */
export class SolvelaBudgetExceededError extends APICallError {
  static override isInstance(
    error: unknown,
  ): error is SolvelaBudgetExceededError {
    return (
      error != null &&
      typeof error === 'object' &&
      (error as Record<symbol, unknown>)[BUDGET_ERROR_MARKER] === true
    );
  }

  constructor(params: SolvelaAPIErrorParams) {
    const clean = sanitizeError({
      ...params,
      responseHeaders: params.responseHeaders
        ? stripPaymentSignature(params.responseHeaders)
        : undefined,
    });
    super({ ...clean, isRetryable: false });
    (this as Record<symbol, unknown>)[BUDGET_ERROR_MARKER] = true;
    this.name = 'SolvelaBudgetExceededError';
  }
}

// ---------------------------------------------------------------------------
// SolvelaSigningError
// ---------------------------------------------------------------------------

/**
 * Raised when the wallet adapter fails to sign a transaction.
 * Never retryable — a signing failure indicates a configuration or key problem.
 *
 * Per Sec-15/M3: the cause message is explicitly double-redacted
 * (redactHex then redactBase58) before being passed to the base class.
 */
export class SolvelaSigningError extends APICallError {
  static override isInstance(
    error: unknown,
  ): error is SolvelaSigningError {
    return (
      error != null &&
      typeof error === 'object' &&
      (error as Record<symbol, unknown>)[SIGNING_ERROR_MARKER] === true
    );
  }

  constructor(params: SolvelaAPIErrorParams) {
    // Sec-15/M3: explicitly redact the cause message before sanitizing.
    const causeWithRedactedMessage =
      params.cause != null &&
      typeof params.cause === 'object' &&
      typeof (params.cause as Record<string, unknown>)['message'] === 'string'
        ? {
            ...(params.cause as Record<string, unknown>),
            message: redactBase58(
              redactHex(
                (params.cause as Record<string, unknown>)['message'] as string,
              ),
            ),
          }
        : params.cause;

    const clean = sanitizeError({
      ...params,
      cause: causeWithRedactedMessage,
      responseHeaders: params.responseHeaders
        ? stripPaymentSignature(params.responseHeaders)
        : undefined,
    });
    super({ ...clean, isRetryable: false });
    (this as Record<symbol, unknown>)[SIGNING_ERROR_MARKER] = true;
    this.name = 'SolvelaSigningError';
  }
}

// ---------------------------------------------------------------------------
// SolvelaUpstreamError
// ---------------------------------------------------------------------------

/**
 * Raised when the upstream LLM provider returns an error response.
 *
 * Retryability:
 * - `statusCode == null` (network / transport error) — retryable
 * - 5xx status codes — retryable
 * - 408, 409, 429 — retryable
 * - all other status codes — not retryable
 */
export class SolvelaUpstreamError extends APICallError {
  static override isInstance(
    error: unknown,
  ): error is SolvelaUpstreamError {
    return (
      error != null &&
      typeof error === 'object' &&
      (error as Record<symbol, unknown>)[UPSTREAM_ERROR_MARKER] === true
    );
  }

  constructor(params: SolvelaAPIErrorParams & { isRetryable?: boolean }) {
    const retryable = params.isRetryable ?? isUpstreamRetryable(params.statusCode);
    const clean = sanitizeError({
      ...params,
      responseHeaders: params.responseHeaders
        ? stripPaymentSignature(params.responseHeaders)
        : undefined,
    });
    super({ ...clean, isRetryable: retryable });
    (this as Record<symbol, unknown>)[UPSTREAM_ERROR_MARKER] = true;
    this.name = 'SolvelaUpstreamError';
  }
}

function isUpstreamRetryable(statusCode: number | undefined): boolean {
  if (statusCode == null) return true; // network error
  if (statusCode >= 500) return true;  // 5xx server error
  if (statusCode === 408 || statusCode === 409 || statusCode === 429) return true;
  return false;
}

// ---------------------------------------------------------------------------
// SolvelaInvalidConfigError
// ---------------------------------------------------------------------------

/**
 * Raised at construction time when the provider configuration is invalid
 * (e.g. missing wallet adapter, bad base URL, unsupported network).
 *
 * Extends `AISDKError` rather than `APICallError` because there is no
 * associated HTTP request.
 */
export class SolvelaInvalidConfigError extends AISDKError {
  static override isInstance(
    error: unknown,
  ): error is SolvelaInvalidConfigError {
    return (
      error != null &&
      typeof error === 'object' &&
      (error as Record<symbol, unknown>)[INVALID_CONFIG_MARKER] === true
    );
  }

  constructor(params: { message: string; cause?: unknown }) {
    const clean = sanitizeError(params);
    super({
      name: 'SolvelaInvalidConfigError',
      message: clean.message,
      cause: clean.cause,
    });
    (this as Record<symbol, unknown>)[INVALID_CONFIG_MARKER] = true;
    this.name = 'SolvelaInvalidConfigError';
  }
}
