/**
 * Custom fetch wrapper — 402 → sign → retry loop (§4.3 fetch-wrapper).
 *
 * Passed into `createOpenAICompatible` as its `fetch` option. Runs inside
 * `postJsonToApi` but intercepts the response before `postJsonToApi` performs
 * its non-2xx → `APICallError` conversion:
 *
 *   - On the 200 path the wrapper inspects only `resp.status`; the body is
 *     never read (preserves SSE streams — T2-D).
 *   - On 402 the wrapper parses the gateway envelope, reserves budget, calls
 *     the wallet adapter, retries once with `PAYMENT-SIGNATURE`, finalizes on
 *     success, releases on any failure, throws sanitized Solvela errors
 *     directly so the signed header never reaches `APICallError` (T1-C).
 *   - On retry non-2xx that is NOT 402, the wrapper constructs
 *     `SolvelaUpstreamError` with `statusCode`, redacted `responseBody`, and
 *     `responseHeaders` with `PAYMENT-SIGNATURE` stripped (Option A seam).
 *
 * Per-invocation counter: exactly 2 base-fetch calls on the 402 path,
 * 1 on the 200 path, 0 extra retries (Sec-21). On the post-payment path the
 * thrown `SolvelaUpstreamError` carries `isRetryable: false` explicitly, so
 * the AI SDK's outer retry loop never re-invokes `solvelaFetch` after payment
 * has been submitted (double-spend guard, Sec-21).
 */

import type { FetchFunction } from '@ai-sdk/provider-utils';

import { BudgetState } from './budget.js';
import {
  SolvelaPaymentError,
  SolvelaSigningError,
  SolvelaUpstreamError,
} from './errors.js';
import type { SolvelaWalletAdapter } from './wallet-adapter.js';
import { parseGateway402, selectAccept } from './util/parse-402.js';
import { stripPaymentSignature } from './util/redact.js';
import { warnOnce } from './util/warn-once.js';

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/** Canonical casing emitted by the provider; gateway reads case-insensitively. */
const PAYMENT_SIGNATURE_HEADER = 'PAYMENT-SIGNATURE';
const PAYMENT_SIGNATURE_LOWER = 'payment-signature';

const WARN_ABORT_MID_RETRY =
  '[solvela] aborted mid-retry — signed transaction built but not submitted; ' +
  'wallet may be in an uncertain state until blockhash expires (~60-90s)';

// ---------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------

/**
 * Event payload emitted to the optional logger — exposes the per-invocation
 * fetch counter so Unit-3 can assert exactly-2-on-402 / exactly-1-on-200.
 */
export interface SolvelaFetchLogEvent {
  event: 'fetch-start' | 'fetch-end' | 'sign-start' | 'sign-end' | 'release' | 'finalize';
  attempt: number;
  requestId: string;
  status?: number;
}

export interface CreateSolvelaFetchOptions {
  /** Required wallet adapter. */
  wallet: SolvelaWalletAdapter;
  /** Shared per-provider budget state. */
  budget: BudgetState;
  /**
   * Cap on `init.body.length` before signing (T2-C). The wrapper refuses to
   * sign bodies larger than this.
   */
  maxSignedBodyBytes: number;
  /**
   * Optional debug hook. Invoked synchronously with per-invocation events so
   * tests can spy on the fetch counter. Never invoked with signature bytes.
   */
  logger?: (event: SolvelaFetchLogEvent) => void;
  /**
   * Optional override of the underlying fetch (test seam). Defaults to
   * `globalThis.fetch`. AI SDK convention.
   */
  baseFetch?: typeof globalThis.fetch;
}

// ---------------------------------------------------------------------------
// Local type aliases
// ---------------------------------------------------------------------------

/**
 * Fetch input shape: URL | string | Request.
 * `@types/node` exposes `Request`/`Response`/`RequestInit` but not
 * `HeadersInit` / `RequestInfo`, so we re-derive locally.
 */
type FetchInput = Parameters<typeof globalThis.fetch>[0];

/** Shape accepted by `RequestInit.headers`. */
type HeadersInput = RequestInit['headers'];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/**
 * Coerce `url` (URL | string | Request) to a string. `Request` exposes `.url`;
 * `URL` exposes `toString()`.
 */
function urlToString(url: FetchInput): string {
  if (typeof url === 'string') return url;
  if (url instanceof URL) return url.toString();
  // Request: has a `.url` string field.
  return (url as { url: string }).url;
}

/**
 * Return the value of a header (case-insensitive) from any `HeadersInit`
 * shape: `Headers`, `string[][]`, or `Record<string, string>`.
 * Returns `undefined` when absent.
 */
function readHeader(
  headers: HeadersInput | undefined,
  name: string,
): string | undefined {
  if (!headers) return undefined;
  const lower = name.toLowerCase();

  if (typeof Headers !== 'undefined' && headers instanceof Headers) {
    const v = headers.get(name);
    return v === null ? undefined : v;
  }
  if (Array.isArray(headers)) {
    for (const entry of headers) {
      if (
        Array.isArray(entry) &&
        entry.length === 2 &&
        typeof entry[0] === 'string' &&
        typeof entry[1] === 'string' &&
        entry[0].toLowerCase() === lower
      ) {
        return entry[1];
      }
    }
    return undefined;
  }
  // Plain record
  for (const [k, v] of Object.entries(headers as Record<string, string>)) {
    if (k.toLowerCase() === lower && typeof v === 'string') return v;
  }
  return undefined;
}

/**
 * Produce a plain `Record<string, string>` from any `HeadersInit`. Used only
 * on the retry path where we need to spread + override
 * `PAYMENT-SIGNATURE`.
 */
function normalizeHeaders(
  headers: HeadersInput | undefined,
): Record<string, string> {
  if (!headers) return {};
  const out: Record<string, string> = {};
  if (typeof Headers !== 'undefined' && headers instanceof Headers) {
    headers.forEach((value, key) => {
      out[key] = value;
    });
    return out;
  }
  if (Array.isArray(headers)) {
    for (const entry of headers) {
      if (
        Array.isArray(entry) &&
        entry.length === 2 &&
        typeof entry[0] === 'string' &&
        typeof entry[1] === 'string'
      ) {
        out[entry[0]] = entry[1];
      }
    }
    return out;
  }
  return { ...(headers as Record<string, string>) };
}

/**
 * True when `err` is a DOMException / native AbortError surfaced by `fetch`
 * or a wallet's abort handling. Rethrown unchanged so the AI SDK's abort
 * handling recognises it.
 */
function isAbortError(err: unknown): boolean {
  if (err == null || typeof err !== 'object') return false;
  const name = (err as { name?: unknown }).name;
  return typeof name === 'string' && name === 'AbortError';
}

/**
 * Build a fresh AbortError. Used when `init.signal.aborted` is already true
 * between sign and retry.
 */
function makeAbortError(): Error {
  // Prefer DOMException where available for API compatibility.
  if (typeof DOMException === 'function') {
    return new DOMException('The operation was aborted', 'AbortError');
  }
  const err = new Error('The operation was aborted');
  err.name = 'AbortError';
  return err;
}

/**
 * Read a response body as text safely. Callers invoke this only for
 * non-streaming error envelopes (safe to buffer). Never called on the 200
 * path — streams must not be tee'd.
 */
async function safeReadText(resp: Response): Promise<string | undefined> {
  try {
    return await resp.text();
  } catch (err) {
    if (isAbortError(err)) throw err;
    return undefined;
  }
}

/**
 * Convert `Response.headers` into a plain `Record<string, string>` for error
 * surfaces. Note the caller MUST still run `stripPaymentSignature` — this
 * helper only projects the shape.
 */
function headersToRecord(headers: Headers): Record<string, string> {
  const out: Record<string, string> = {};
  headers.forEach((value, key) => {
    out[key] = value;
  });
  return out;
}

// ---------------------------------------------------------------------------
// createSolvelaFetch
// ---------------------------------------------------------------------------

/**
 * Build the custom fetch function wired into `createOpenAICompatible`.
 *
 * Exactly one base-fetch on 200 path, exactly two on the 402 path — asserted
 * via the counter in `SolvelaFetchLogEvent`.
 */
export function createSolvelaFetch(
  options: CreateSolvelaFetchOptions,
): FetchFunction {
  const {
    wallet,
    budget,
    maxSignedBodyBytes,
    logger,
    baseFetch = globalThis.fetch,
  } = options;

  return async function solvelaFetch(
    url: FetchInput,
    init?: RequestInit,
  ): Promise<Response> {
    // (a) Per-invocation requestId. Used as the BudgetState reservation key.
    const requestId = crypto.randomUUID();

    // Per-invocation counter — local to this closure, never shared across
    // logical requests (concurrency safe).
    let fetchCount = 0;
    const emitFetchStart = (): number => {
      fetchCount += 1;
      logger?.({ event: 'fetch-start', attempt: fetchCount, requestId });
      return fetchCount;
    };
    const emitFetchEnd = (attempt: number, status: number): void => {
      logger?.({ event: 'fetch-end', attempt, requestId, status });
    };

    const resolvedUrl = urlToString(url);

    // (b) First call. Body is NOT read on the 200 path (T2-D — preserves SSE).
    const attempt1 = emitFetchStart();
    const resp = await baseFetch(url, init);
    emitFetchEnd(attempt1, resp.status);

    // (c) Non-402 first-response: return as-is.
    if (resp.status !== 402) {
      return resp;
    }

    // (d) Caller-supplied PAYMENT-SIGNATURE refuses re-sign (T2-F).
    const callerSig = readHeader(init?.headers, PAYMENT_SIGNATURE_LOWER);
    if (callerSig !== undefined) {
      throw new SolvelaPaymentError({
        message: 'caller supplied PAYMENT-SIGNATURE; refusing to re-sign',
        url: resolvedUrl,
        requestBodyValues: undefined,
        statusCode: resp.status,
      });
    }

    // (e) Parse the gateway 402 envelope. Must read the body — it is a
    // non-streaming error envelope (safe to buffer).
    const rawText = await safeReadText(resp);
    let parsedBody: unknown;
    try {
      parsedBody = rawText === undefined ? undefined : JSON.parse(rawText);
    } catch {
      throw new SolvelaPaymentError({
        message: '[solvela] 402 envelope: body is not valid JSON',
        url: resolvedUrl,
        requestBodyValues: undefined,
        statusCode: resp.status,
      });
    }
    const parsed = parseGateway402(parsedBody);
    const selected = selectAccept(parsed);

    // (f) Body type + size check (T2-C). `createOpenAICompatible` always
    // serialises to a JSON string; anything else is out-of-scope in v1.
    const body = init?.body;
    if (typeof body !== 'string') {
      throw new SolvelaPaymentError({
        message: 'unsupported body type for payment signing in v1',
        url: resolvedUrl,
        requestBodyValues: undefined,
        statusCode: resp.status,
      });
    }
    if (new TextEncoder().encode(body).byteLength > maxSignedBodyBytes) {
      throw new SolvelaPaymentError({
        message: 'request body exceeds payment signing size cap',
        url: resolvedUrl,
        requestBodyValues: undefined,
        statusCode: resp.status,
      });
    }

    // (g) Atomic reserve. SolvelaBudgetExceededError propagates as-is;
    // wallet.signPayment is NOT reached on reserve failure (fail-closed:
    // a zero-cost wallet call would race the budget otherwise).
    budget.reserve(requestId, selected.cost);

    // (h) Call the wallet adapter. Any throw => release reservation and either
    // rethrow AbortError unchanged or wrap in SolvelaSigningError.
    let signature: string;
    try {
      logger?.({ event: 'sign-start', attempt: fetchCount, requestId });
      signature = await wallet.signPayment({
        paymentRequired: parsed,
        resourceUrl: resolvedUrl,
        requestBody: body,
        signal: init?.signal ?? undefined,
      });
      logger?.({ event: 'sign-end', attempt: fetchCount, requestId });
    } catch (err) {
      budget.release(requestId);
      logger?.({ event: 'release', attempt: fetchCount, requestId });
      if (isAbortError(err)) {
        // Preserve AbortError identity so AI SDK abort handling works.
        throw err;
      }
      throw new SolvelaSigningError({
        message: '[solvela] wallet adapter failed to sign payment',
        url: resolvedUrl,
        requestBodyValues: undefined,
        cause: err,
      });
    }

    // Guard: if abort fired during signing, don't submit a signed tx. T2-E.
    if (init?.signal?.aborted) {
      budget.release(requestId);
      logger?.({ event: 'release', attempt: fetchCount, requestId });
      warnOnce(WARN_ABORT_MID_RETRY);
      throw makeAbortError();
    }

    // (i) Retry with PAYMENT-SIGNATURE.
    const retryHeaders: Record<string, string> = {
      ...normalizeHeaders(init?.headers),
      [PAYMENT_SIGNATURE_HEADER]: signature,
    };

    const attempt2 = emitFetchStart();
    let retryResp: Response;
    try {
      retryResp = await baseFetch(url, {
        ...init,
        headers: retryHeaders,
        signal: init?.signal ?? null,
      });
    } catch (err) {
      budget.release(requestId);
      logger?.({ event: 'release', attempt: fetchCount, requestId });
      if (isAbortError(err)) {
        // Abort fired between sign and retry (T2-E). warn-once with NO
        // signature bytes, rethrow the AbortError.
        warnOnce(WARN_ABORT_MID_RETRY);
        throw err;
      }
      // Network / transport error — no signed header leaked.
      throw err;
    }
    emitFetchEnd(attempt2, retryResp.status);

    // (j) Retry 2xx — finalize, return response unmodified. 200 path never
    // reads the body; `postJsonToApi` handles streaming/non-streaming from
    // here.
    if (retryResp.status >= 200 && retryResp.status < 300) {
      budget.finalize(requestId);
      logger?.({ event: 'finalize', attempt: fetchCount, requestId });
      return retryResp;
    }

    // (l) Retry-bomb guard — retry also returned 402 (T1-B + Sec-4).
    if (retryResp.status === 402) {
      budget.release(requestId);
      logger?.({ event: 'release', attempt: fetchCount, requestId });
      // Do not loop. Surface a single payment error.
      throw new SolvelaPaymentError({
        message: 'Payment rejected after retry',
        url: resolvedUrl,
        requestBodyValues: undefined,
        statusCode: retryResp.status,
      });
    }

    // (k) Any other non-2xx on retry — Option A seam. Read the error envelope
    // (non-streaming — safe to buffer), strip PAYMENT-SIGNATURE from response
    // headers, throw a sanitized SolvelaUpstreamError directly so
    // `postJsonToApi` never sees the retry response.
    budget.release(requestId);
    logger?.({ event: 'release', attempt: fetchCount, requestId });

    const retryBody = await safeReadText(retryResp);
    const retryHeadersRecord = stripPaymentSignature(
      headersToRecord(retryResp.headers),
    );

    throw new SolvelaUpstreamError({
      message: `[solvela] upstream request failed after payment (status ${retryResp.status})`,
      url: resolvedUrl,
      requestBodyValues: undefined,
      statusCode: retryResp.status,
      responseHeaders: retryHeadersRecord,
      responseBody: retryBody,
      isRetryable: false,
    });
  };
}
