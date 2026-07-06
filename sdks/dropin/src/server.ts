/**
 * The sidecar HTTP server — the localhost process Claude Code points at via
 * `ANTHROPIC_BASE_URL`.
 *
 * Per-request money path (LOCKED by the sidecar design + channel plan):
 *   1. authenticate the LOCAL bearer (`Authorization: Bearer` or `x-api-key`,
 *      constant-time compare) — 401 otherwise;
 *   2. buffer the raw request body bytes;
 *   3. forward to the gateway WITHOUT payment (allowlisted headers only — the
 *      local token NEVER goes upstream);
 *   4. non-402 → relay verbatim; 402 → `parse402`, take the `exact` entry's
 *      `amount` as the quote (the sidecar NEVER computes fees or invents
 *      amounts — signer-core's tracker validates the amount string
 *      fail-closed), and `ChannelTracker.draw` signs a voucher for
 *      `last + quote` and re-POSTs the IDENTICAL bytes with
 *      `PAYMENT-SIGNATURE` (the voucher digest binds to the bytes);
 *   5. relay the 2xx: SSE is piped byte-verbatim as chunks arrive (no
 *      buffering, no reframing — thinking-block `signature`s must survive),
 *      honoring backpressure; JSON relays status+headers+body.
 *
 * `expiry_slot` is sourced from `GET {gateway}/v1/escrow/config` (cached
 * ~30s) and FAILS CLOSED — no slot, no draw, nothing signed.
 *
 * Lifecycle: every request gets an AbortController tied to the client
 * connection — a client that disconnects aborts the in-flight upstream
 * fetch/stream (no orphaned full-rate streams). All outbound fetches carry
 * deadlines: short for metadata/probe legs, a long total ceiling (650s,
 * matching the gateway's draw-serve posture) for the paid serve.
 *
 * The channel is strictly serial: signer-core's `ChannelTracker` single-flight
 * gate queues concurrent Claude Code requests (parallel subagents serialize).
 * One channel per sidecar instance.
 *
 * This module deliberately binds NOTHING itself — the CLI listens on
 * 127.0.0.1 ONLY (a hosted sidecar is forbidden: design doc §4 — signing on
 * users' behalf from a server would reintroduce the custodial problem the
 * sidecar exists to remove).
 */

import { createHash, timingSafeEqual } from 'node:crypto';
import { once } from 'node:events';
import { createServer, type IncomingMessage, type Server, type ServerResponse } from 'node:http';

import {
  ChannelTracker,
  ChannelDrawError,
  parse402,
  sanitizeGatewayError,
  type ChannelTrackerOptions,
  type DrawResponse,
} from '@solvela/signer-core';

/** Default voucher expiry margin: generous vs the 50-slot verifier buffer. */
const DEFAULT_EXPIRY_MARGIN_SLOTS = 1200n;
/** How long a fetched `current_slot` may be reused before a mandatory refresh. */
const DEFAULT_SLOT_TTL_MS = 30_000;

/** Deadline for short outbound legs: slot/config fetch, the unpaid 402 probe,
 * and the count_tokens forward. None of these serves a completion. */
const SHORT_TIMEOUT_MS = 10_000;
/** Total ceiling for the paid serve (headers + streamed body) — matches the
 * gateway's `draw_serve_timeout_secs` posture (650s > the 600s native relay). */
const PAID_SERVE_TIMEOUT_MS = 650_000;

/** Generous request-body cap — defense in depth on a local-auth-gated port. */
const MAX_BODY_BYTES = 32 * 1024 * 1024;

/** The ONLY inbound headers forwarded upstream. `authorization`/`x-api-key`
 * (the local token) are deliberately absent — they must never leave the box. */
const FORWARD_HEADERS = ['content-type', 'anthropic-version', 'anthropic-beta'] as const;

/** Headers never copied onto the relayed response (framing is Node's job, and
 * undici has already decoded any content-encoding). `set-cookie` is handled
 * separately via `getSetCookie()` — `Headers.forEach` would collapse
 * duplicates. */
const SKIP_RESPONSE_HEADERS = new Set([
  'transfer-encoding',
  'connection',
  'keep-alive',
  'content-length',
  'content-encoding',
  'set-cookie',
]);

export interface SidecarOptions {
  /** Gateway base URL, no trailing slash (e.g. `https://api.solvela.ai`). */
  gatewayUrl: string;
  /** The local bearer token Claude Code presents. Never forwarded upstream. */
  localToken: string;
  /** 32-byte channel id. */
  channelId: Uint8Array;
  /** 32-byte ed25519 session seed (or 64-byte expanded key). SECRET. */
  signSeed: Uint8Array;
  /** Resume point for the cumulative tracker (default 0; gateway resyncs). */
  lastCumulative?: bigint;
  /** Tracker knobs (tests use fast backoff). */
  trackerOptions?: Omit<ChannelTrackerOptions, 'lastCumulative'>;
  /** Voucher expiry margin above the sourced slot (default 1200). */
  expiryMarginSlots?: bigint;
  /** Slot cache TTL in ms (default 30s). Past it, a fresh fetch is REQUIRED. */
  slotTtlMs?: number;
  /** Best-effort persist hook, called after each accepted draw. */
  onLastCumulative?: (last: bigint) => void;
}

/** Fail-closed slot sourcing error → 503 (retryable, nothing signed). */
class SlotUnavailableError extends Error {}

/** Request body exceeded [`MAX_BODY_BYTES`] → 413. */
class BodyTooLargeError extends Error {}

/**
 * Source `current_slot` from `GET {gateway}/v1/escrow/config`, cached for
 * `ttlMs`. FAILS CLOSED: an unreachable/slow gateway, a non-200 (escrow
 * unconfigured → 404), or a null/absent `current_slot` throws — there is NO
 * stale fallback, because a stale (lower) slot would inflate the voucher's
 * apparent expiry buffer.
 */
function createGatewaySlotSource(gatewayUrl: string, ttlMs: number): () => Promise<bigint> {
  let cached: { slot: bigint; at: number } | null = null;
  return async () => {
    if (cached && Date.now() - cached.at < ttlMs) return cached.slot;
    let resp: Response;
    try {
      // Own deadline, NOT the caller's client-abort signal: the fetched slot
      // is a shared cache other queued draws also consume.
      resp = await fetch(`${gatewayUrl}/v1/escrow/config`, {
        signal: AbortSignal.timeout(SHORT_TIMEOUT_MS),
      });
    } catch {
      throw new SlotUnavailableError(
        'could not reach the gateway /v1/escrow/config for the current Solana slot; refusing to sign a voucher',
      );
    }
    if (!resp.ok) {
      await resp.text().catch(() => {});
      throw new SlotUnavailableError(
        `gateway /v1/escrow/config returned ${resp.status}; cannot source the current Solana slot for the voucher expiry`,
      );
    }
    let body: unknown;
    try {
      body = await resp.json();
    } catch {
      throw new SlotUnavailableError('gateway /v1/escrow/config returned a non-JSON body');
    }
    const slot = (body as Record<string, unknown> | null)?.['current_slot'];
    if (typeof slot !== 'number' || !Number.isSafeInteger(slot) || slot <= 0) {
      throw new SlotUnavailableError(
        'gateway /v1/escrow/config has no usable current_slot; refusing to sign a voucher',
      );
    }
    cached = { slot: BigInt(slot), at: Date.now() };
    return cached.slot;
  };
}

/** Constant-time token compare (hash both sides to equalize lengths first). */
function tokenMatches(provided: string, expected: string): boolean {
  const a = createHash('sha256').update(provided, 'utf-8').digest();
  const b = createHash('sha256').update(expected, 'utf-8').digest();
  return timingSafeEqual(a, b);
}

/** Claude Code sends whichever env var the user set: bearer or x-api-key. */
function extractLocalToken(req: IncomingMessage): string | undefined {
  const auth = req.headers['authorization'];
  if (typeof auth === 'string' && auth.toLowerCase().startsWith('bearer ')) {
    return auth.slice('bearer '.length).trim();
  }
  const key = req.headers['x-api-key'];
  if (typeof key === 'string' && key !== '') return key;
  return undefined;
}

function allowlistedHeaders(req: IncomingMessage): Record<string, string> {
  const out: Record<string, string> = {};
  for (const name of FORWARD_HEADERS) {
    const v = req.headers[name];
    if (typeof v === 'string' && v !== '') out[name] = v;
  }
  return out;
}

/** Map an HTTP status to the Anthropic wire error type Claude Code expects. */
function anthropicErrorType(status: number): string {
  switch (status) {
    case 401:
      return 'authentication_error';
    case 400:
    case 413:
      return 'invalid_request_error';
    case 404:
      return 'not_found_error';
    case 429:
      return 'rate_limit_error';
    case 503:
    case 529:
      return 'overloaded_error';
    default:
      return 'api_error';
  }
}

/** Anthropic-envelope error, so Claude Code renders sidecar failures cleanly. */
function jsonError(res: ServerResponse, status: number, message: string): void {
  res.writeHead(status, { 'content-type': 'application/json' });
  res.end(JSON.stringify({ type: 'error', error: { type: anthropicErrorType(status), message } }));
}

async function readBody(req: IncomingMessage) {
  const chunks: Buffer[] = [];
  let total = 0;
  for await (const chunk of req) {
    total += (chunk as Buffer).length;
    if (total > MAX_BODY_BYTES) {
      throw new BodyTooLargeError(`request body exceeds ${MAX_BODY_BYTES} bytes`);
    }
    chunks.push(chunk as Buffer);
  }
  const buf = Buffer.concat(chunks);
  // Same bytes, zero copy — re-viewed so TS accepts it as a fetch BodyInit
  // (Buffer's ArrayBufferLike generic is not assignable to BufferSource).
  return new Uint8Array(buf.buffer as ArrayBuffer, buf.byteOffset, buf.byteLength);
}

/**
 * Relay a fetch `Response` verbatim: status + headers (minus framing;
 * `set-cookie` preserved per-value), then pipe body chunks AS THEY ARRIVE —
 * for SSE this is the byte-verbatim, incremental path thinking-block
 * signatures depend on. Honors client backpressure (`drain`). On any
 * mid-stream failure the client connection is DESTROYED (abnormal
 * termination), never cleanly ended — a truncated stream must not look
 * complete.
 */
async function relay(
  upstream: Response,
  res: ServerResponse,
  clientGone?: AbortSignal,
): Promise<void> {
  const headers: Record<string, string | string[]> = {};
  upstream.headers.forEach((value, name) => {
    if (!SKIP_RESPONSE_HEADERS.has(name.toLowerCase())) headers[name] = value;
  });
  const cookies = upstream.headers.getSetCookie();
  if (cookies.length > 0) headers['set-cookie'] = cookies;
  res.writeHead(upstream.status, headers);
  if (!upstream.body) {
    res.end();
    return;
  }
  try {
    for await (const chunk of upstream.body as unknown as AsyncIterable<Uint8Array>) {
      if (!res.write(chunk)) {
        // Backpressure: wait for the client to drain — or leave. Early return
        // cancels the upstream iterator (ReadableStream return()).
        await Promise.race([once(res, 'drain'), once(res, 'close')]);
        if (!res.writable) {
          res.destroy();
          return;
        }
      }
    }
    res.end();
  } catch (err) {
    if (clientGone?.aborted) {
      console.error('[solvela-dropin] client disconnected mid-stream; upstream read aborted');
    } else {
      console.error(
        `[solvela-dropin] upstream stream error mid-relay: ${sanitizeGatewayError(
          err instanceof Error ? err.message : String(err),
        )}`,
      );
    }
    res.destroy();
  }
}

/**
 * Build the sidecar server. The caller MUST listen on 127.0.0.1 only.
 * One `ChannelTracker` per server — one channel per sidecar instance; its
 * single-flight gate serializes concurrent draws.
 */
export function createSidecarServer(opts: SidecarOptions): Server {
  const expiryMargin = opts.expiryMarginSlots ?? DEFAULT_EXPIRY_MARGIN_SLOTS;
  const getSlot = createGatewaySlotSource(opts.gatewayUrl, opts.slotTtlMs ?? DEFAULT_SLOT_TTL_MS);
  const tracker = new ChannelTracker(opts.channelId, {
    lastCumulative: opts.lastCumulative ?? 0n,
    ...opts.trackerOptions,
  });

  return createServer((req, res) => {
    // Tie the whole outbound lifecycle to the client connection: a client
    // that disconnects aborts every in-flight upstream fetch/stream for this
    // request. `close` also fires on normal completion — only abort when the
    // response did not finish.
    const clientGone = new AbortController();
    res.on('close', () => {
      if (!res.writableFinished) clientGone.abort();
    });

    handle(req, res, clientGone.signal).catch((err: unknown) => {
      const msg = err instanceof Error ? err.message : String(err);
      if (err instanceof BodyTooLargeError) {
        if (!res.headersSent) jsonError(res, 413, msg);
        else res.destroy();
        return;
      }
      if (clientGone.signal.aborted) {
        // The client walked away; outbound work was aborted by design.
        res.destroy();
        return;
      }
      console.error(`[solvela-dropin] request failed: ${sanitizeGatewayError(msg)}`);
      if (!res.headersSent) jsonError(res, 502, sanitizeGatewayError(msg));
      else res.destroy();
    });
  });

  async function handle(
    req: IncomingMessage,
    res: ServerResponse,
    clientGone: AbortSignal,
  ): Promise<void> {
    const url = new URL(req.url ?? '/', 'http://127.0.0.1');

    // Local auth on EVERYTHING — the open port must not be driveable by a
    // stray local process, and count_tokens rides the same token.
    const provided = extractLocalToken(req);
    if (provided === undefined || !tokenMatches(provided, opts.localToken)) {
      jsonError(res, 401, 'invalid or missing local sidecar token (ANTHROPIC_AUTH_TOKEN)');
      return;
    }

    if (req.method !== 'POST') {
      jsonError(res, 404, 'not found — the sidecar serves POST /v1/messages and /v1/messages/count_tokens only');
      return;
    }

    if (url.pathname === '/v1/messages/count_tokens') {
      // Free verbatim reverse-proxy — no payment header, ever.
      const body = await readBody(req);
      const upstream = await fetch(`${opts.gatewayUrl}${url.pathname}${url.search}`, {
        method: 'POST',
        headers: allowlistedHeaders(req),
        body,
        signal: AbortSignal.any([clientGone, AbortSignal.timeout(SHORT_TIMEOUT_MS)]),
      });
      await relay(upstream, res, clientGone);
      return;
    }

    if (url.pathname !== '/v1/messages') {
      jsonError(res, 404, 'not found — the sidecar serves POST /v1/messages and /v1/messages/count_tokens only');
      return;
    }

    // Buffer the raw bytes ONCE: the voucher digest binds to these bytes, so
    // the unpaid forward and the paid retry must send the identical buffer.
    const rawBody = await readBody(req);
    const target = `${opts.gatewayUrl}${url.pathname}${url.search}`;
    const fwdHeaders = allowlistedHeaders(req);

    // Unpaid probe — expected to 402 (the quote) or fail fast. A non-402 here
    // is an error body / dev stub, so its relay riding the short deadline too
    // is fine (the paid serve below is the long-lived leg).
    const first = await fetch(target, {
      method: 'POST',
      headers: fwdHeaders,
      body: rawBody,
      signal: AbortSignal.any([clientGone, AbortSignal.timeout(SHORT_TIMEOUT_MS)]),
    });
    if (first.status !== 402) {
      await relay(first, res, clientGone);
      return;
    }

    // 402 → the quote is the exact entry's fee-inclusive amount. The 402 never
    // advertises `channel` — the channel is header-invoked; no exact entry
    // means no quotable draw, so fail closed (never pick another scheme).
    const challengeText = await first.text();
    let accept;
    let resourceUrl: string;
    try {
      const pr = parse402(challengeText);
      accept = pr.accepts.find((a) => a.scheme === 'exact');
      resourceUrl = pr.resource?.url ?? url.pathname;
    } catch (err) {
      const msg = `gateway 402 challenge was malformed: ${sanitizeGatewayError(
        err instanceof Error ? err.message : 'parse error',
      )}`;
      console.error(`[solvela-dropin] ${msg}`);
      jsonError(res, 502, msg);
      return;
    }
    if (!accept) {
      const msg = 'gateway 402 offered no exact quote to derive the channel draw amount from; refusing to sign';
      console.error(`[solvela-dropin] ${msg}`);
      jsonError(res, 502, msg);
      return;
    }

    // expiry_slot — FAIL CLOSED before anything is signed.
    let expirySlot: bigint;
    try {
      expirySlot = (await getSlot()) + expiryMargin;
    } catch (err) {
      const msg =
        err instanceof Error ? err.message : 'current Solana slot unavailable; refusing to sign a voucher';
      console.error(`[solvela-dropin] draw refused (slot source): ${msg}`);
      jsonError(res, 503, msg);
      return;
    }

    // The draw. `send` re-POSTs the IDENTICAL bytes + the voucher header; on a
    // 2xx it hands the raw Response back through the tracker (which advances
    // `last` and resolves with it); on a non-2xx it parses JSON so the tracker
    // can extract the structured `error.last_cumulative` resync. The signal
    // composes the client's lifetime with the long serve ceiling — it stays
    // attached through the RELAYED BODY below, so 650s bounds the whole serve.
    const send = async (header: string): Promise<DrawResponse> => {
      const resp = await fetch(target, {
        method: 'POST',
        headers: { ...fwdHeaders, 'payment-signature': header },
        body: rawBody,
        signal: AbortSignal.any([clientGone, AbortSignal.timeout(PAID_SERVE_TIMEOUT_MS)]),
      });
      if (resp.status >= 200 && resp.status < 300) {
        return { status: resp.status, body: resp };
      }
      let parsed: unknown;
      try {
        parsed = await resp.json();
      } catch {
        parsed = undefined;
      }
      return { status: resp.status, body: parsed };
    };

    let drawResult: unknown;
    try {
      drawResult = await tracker.draw({
        accept,
        resourceUrl,
        rawBody,
        signSeed: opts.signSeed,
        expirySlot,
        send,
      });
    } catch (err) {
      if (err instanceof ChannelDrawError) {
        // Tracker messages are static/gateway-status-derived — safe. Propagate
        // the gateway status where it is a real HTTP status; 0 = local
        // validation (e.g. malformed quote) → 502.
        const status = err.status >= 400 && err.status <= 599 ? err.status : 502;
        console.error(`[solvela-dropin] channel draw rejected (gateway status ${err.status}): ${err.message}`);
        jsonError(res, status, err.message);
        return;
      }
      // Anything else (e.g. the retry fetch itself rejecting) propagates to
      // the outer catch: logged + 502, fail closed. `#last` did NOT advance.
      throw err;
    }

    // The 2xx contract of `send` above puts the raw Response in the tracker's
    // resolution. Narrow it — never a bare cast on the money path; a
    // non-Response here is a refactor bug, so fail closed rather than relay
    // garbage.
    if (!(drawResult instanceof Response)) {
      console.error('[solvela-dropin] draw resolved with a non-Response value — refusing to relay (fail closed)');
      jsonError(res, 502, 'internal relay error after a successful draw');
      return;
    }

    // Best-effort persist of the advanced cumulative (gateway remains truth).
    // Guards SYNC throws from the hook only — an async persist failure is the
    // hook's own job to surface (the CLI logs it rate-limited).
    try {
      opts.onLastCumulative?.(tracker.lastCumulative);
    } catch (err) {
      console.error(
        `[solvela-dropin] last-cumulative persist hook threw: ${err instanceof Error ? err.message : String(err)}`,
      );
    }

    await relay(drawResult, res, clientGone);
  }
}
