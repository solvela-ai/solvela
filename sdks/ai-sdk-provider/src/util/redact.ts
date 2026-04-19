/**
 * Redaction and sanitization utilities for Solvela error handling.
 * These functions are pure (no side effects, no mutation).
 *
 * redactHex and redactBase58 are re-exported from @solvela/signer-core
 * for deduplication. All other functions are AI-SDK-specific and remain here.
 */

import { redactHex, redactBase58 } from '@solvela/signer-core';
export { redactHex, redactBase58 };

/** The PAYMENT-SIGNATURE header name, lowercase for case-insensitive comparison. */
const PAYMENT_SIGNATURE_LOWER = 'payment-signature';

/**
 * Returns a new copy of `headers` with the `PAYMENT-SIGNATURE` header removed
 * (case-insensitive). If the header is absent the original object is returned
 * unchanged (still a new reference via spread so callers can rely on identity).
 */
export function stripPaymentSignature(
  headers: Record<string, string>,
): Record<string, string> {
  const result: Record<string, string> = {};
  for (const [key, value] of Object.entries(headers)) {
    if (key.toLowerCase() !== PAYMENT_SIGNATURE_LOWER) {
      result[key] = value;
    }
  }
  return result;
}

/**
 * Recursively redacts string leaves within an arbitrary `unknown` value.
 * Strings have `redactHex` then `redactBase58` applied. Arrays and objects
 * are walked depth-first. The `seen` WeakMap guards against circular refs AND
 * shared sub-objects: the sanitized result is cached before recursing so a
 * second traversal returns the already-redacted copy rather than the original
 * un-redacted value (T1-C cycle bypass fix, F8).
 * All other primitives are returned unchanged.
 */
function redactUnknown(value: unknown, seen: WeakMap<object, unknown>): unknown {
  if (typeof value === 'string') {
    return redactBase58(redactHex(value));
  }
  if (value == null || typeof value !== 'object') {
    return value;
  }
  const cached = seen.get(value);
  if (cached !== undefined) return cached;
  if (Array.isArray(value)) {
    const arr: unknown[] = [];
    seen.set(value, arr);
    for (const item of value) arr.push(redactUnknown(item, seen));
    return arr;
  }
  const out: Record<string, unknown> = {};
  seen.set(value, out);
  for (const key of Object.keys(value as Record<string, unknown>)) {
    out[key] = redactUnknown((value as Record<string, unknown>)[key], seen);
  }
  return out;
}

/**
 * Walks an error-like object and returns a new copy with sensitive fields
 * redacted. Handles circular references via a `WeakSet` cycle guard.
 *
 * Fields sanitized:
 * - `message` / `stack` — string, redactHex then redactBase58
 * - `cause` — recursive sanitization via `redactUnknown`
 * - `responseHeaders` — stripPaymentSignature
 * - `responseBody` — string, redactHex then redactBase58
 * - `requestBodyValues` — recursive sanitization via `redactUnknown`
 *   (prompts may contain pasted keys; all string leaves are redacted)
 *
 * Returns `null`/`undefined` unchanged. Non-object primitives returned as-is.
 */
export function sanitizeError<T>(value: T, _seen?: WeakMap<object, unknown>): T {
  if (value == null || typeof value !== 'object') {
    return value;
  }

  const seen = _seen ?? new WeakMap<object, unknown>();
  const cached = seen.get(value as object);
  if (cached !== undefined) return cached as T;

  const obj = value as Record<string, unknown>;
  const result: Record<string, unknown> = { ...obj };
  seen.set(value as object, result);

  if (typeof result['message'] === 'string') {
    result['message'] = redactBase58(redactHex(result['message']));
  }

  if (typeof result['stack'] === 'string') {
    result['stack'] = redactBase58(redactHex(result['stack']));
  }

  if (result['cause'] != null && typeof result['cause'] === 'object') {
    result['cause'] = redactUnknown(result['cause'], seen);
  }

  if (
    result['responseHeaders'] != null &&
    typeof result['responseHeaders'] === 'object' &&
    !Array.isArray(result['responseHeaders'])
  ) {
    result['responseHeaders'] = stripPaymentSignature(
      result['responseHeaders'] as Record<string, string>,
    );
  }

  if (typeof result['responseBody'] === 'string') {
    result['responseBody'] = redactBase58(redactHex(result['responseBody']));
  }

  if (result['requestBodyValues'] != null) {
    result['requestBodyValues'] = redactUnknown(
      result['requestBodyValues'],
      seen,
    );
  }

  return result as T;
}
