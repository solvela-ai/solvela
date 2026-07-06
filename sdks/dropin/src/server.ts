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
 *      buffering, no reframing — thinking-block `signature`s must survive);
 *      JSON relays status+headers+body.
 *
 * `expiry_slot` is sourced from `GET {gateway}/v1/escrow/config` (cached
 * ~30s) and FAILS CLOSED — no slot, no draw, nothing signed.
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

/** The ONLY inbound headers forwarded upstream. `authorization`/`x-api-key`
 * (the local token) are deliberately absent — they must never leave the box. */
const FORWARD_HEADERS = ['content-type', 'anthropic-version', 'anthropic-beta'] as const;

/** Headers never copied onto the relayed response (framing is Node's job, and
 * undici has already decoded any content-encoding). */
const SKIP_RESPONSE_HEADERS = new Set([
  'transfer-encoding',
  'connection',
  'keep-alive',
  'content-length',
  'content-encoding',
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

/**
 * Source `current_slot` from `GET {gateway}/v1/escrow/config`, cached for
 * `ttlMs`. FAILS CLOSED: an unreachable gateway, a non-200 (escrow
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
      resp = await fetch(`${gatewayUrl}/v1/escrow/config`);
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

/** Anthropic-envelope error, so Claude Code renders sidecar failures cleanly. */
function jsonError(res: ServerResponse, status: number, message: string): void {
  res.writeHead(status, { 'content-type': 'application/json' });
  res.end(JSON.stringify({ type: 'error', error: { type: 'api_error', message } }));
}

async function readBody(req: IncomingMessage) {
  const chunks: Buffer[] = [];
  for await (const chunk of req) chunks.push(chunk as Buffer);
  const buf = Buffer.concat(chunks);
  // Same bytes, zero copy — re-viewed so TS accepts it as a fetch BodyInit
  // (Buffer's ArrayBufferLike generic is not assignable to BufferSource).
  return new Uint8Array(buf.buffer as ArrayBuffer, buf.byteOffset, buf.byteLength);
}

/**
 * Relay a fetch `Response` verbatim: status + headers (minus framing), then
 * pipe body chunks AS THEY ARRIVE — for SSE this is the byte-verbatim,
 * incremental path thinking-block signatures depend on.
 */
async function relay(upstream: Response, res: ServerResponse): Promise<void> {
  const headers: Record<string, string> = {};
  upstream.headers.forEach((value, name) => {
    if (!SKIP_RESPONSE_HEADERS.has(name.toLowerCase())) headers[name] = value;
  });
  res.writeHead(upstream.status, headers);
  if (!upstream.body) {
    res.end();
    return;
  }
  try {
    for await (const chunk of upstream.body as unknown as AsyncIterable<Uint8Array>) {
      res.write(chunk);
    }
    res.end();
  } catch {
    // Client went away mid-stream (or upstream died). Nothing to relay to.
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
    handle(req, res).catch((err: unknown) => {
      // Last-resort guard; `handle` maps its own errors. Never leak internals.
      if (!res.headersSent) {
        jsonError(res, 502, sanitizeGatewayError(err instanceof Error ? err.message : 'sidecar error'));
      } else {
        res.destroy();
      }
    });
  });

  async function handle(req: IncomingMessage, res: ServerResponse): Promise<void> {
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
      });
      await relay(upstream, res);
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

    const first = await fetch(target, { method: 'POST', headers: fwdHeaders, body: rawBody });
    if (first.status !== 402) {
      await relay(first, res);
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
      jsonError(
        res,
        502,
        `gateway 402 challenge was malformed: ${sanitizeGatewayError(err instanceof Error ? err.message : 'parse error')}`,
      );
      return;
    }
    if (!accept) {
      jsonError(res, 502, 'gateway 402 offered no exact quote to derive the channel draw amount from; refusing to sign');
      return;
    }

    // expiry_slot — FAIL CLOSED before anything is signed.
    let expirySlot: bigint;
    try {
      expirySlot = (await getSlot()) + expiryMargin;
    } catch (err) {
      jsonError(res, 503, err instanceof Error ? err.message : 'current Solana slot unavailable; refusing to sign a voucher');
      return;
    }

    // The draw. `send` re-POSTs the IDENTICAL bytes + the voucher header; on a
    // 2xx it hands the raw Response back through the tracker (which advances
    // `last` and resolves with it); on a non-2xx it parses JSON so the tracker
    // can extract the structured `error.last_cumulative` resync.
    const send = async (header: string): Promise<DrawResponse> => {
      const resp = await fetch(target, {
        method: 'POST',
        headers: { ...fwdHeaders, 'payment-signature': header },
        body: rawBody,
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

    let served: Response;
    try {
      served = (await tracker.draw({
        accept,
        resourceUrl,
        rawBody,
        signSeed: opts.signSeed,
        expirySlot,
        send,
      })) as Response;
    } catch (err) {
      if (err instanceof ChannelDrawError) {
        // Tracker messages are static/gateway-status-derived — safe. Propagate
        // the gateway status where it is a real HTTP status; 0 = local
        // validation (e.g. malformed quote) → 502.
        const status = err.status >= 400 && err.status <= 599 ? err.status : 502;
        jsonError(res, status, err.message);
        return;
      }
      jsonError(res, 502, sanitizeGatewayError(err instanceof Error ? err.message : 'channel draw failed'));
      return;
    }

    // Best-effort persist of the advanced cumulative (gateway remains truth).
    try {
      opts.onLastCumulative?.(tracker.lastCumulative);
    } catch {
      // Persistence is an optimization; never fail the served response on it.
    }

    await relay(served, res);
  }
}
