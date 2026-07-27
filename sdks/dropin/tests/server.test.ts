/**
 * Sidecar relay tests — the money path of `@solvela/dropin`.
 *
 * Every test runs the REAL sidecar (`createSidecarServer`) against an
 * in-process fake gateway (`node:http`), so the flow under test is the exact
 * production path: buffer raw bytes → unpaid forward → 402 → `ChannelTracker`
 * draw (signer-core signs the voucher) → byte-identical paid retry → verbatim
 * relay. Nothing here re-implements or mocks the voucher wire — the header the
 * fake gateway receives is what production would send.
 *
 * The golden constants come from `@solvela/signer-core` (which pins them
 * against `crates/x402/src/channel.rs` / `crates/protocol`); this file NEVER
 * invents new vectors.
 */

import { after, describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import {
  createServer,
  request as httpRequest,
  type IncomingMessage,
  type Server,
  type ServerResponse,
} from 'node:http';
import { once } from 'node:events';
import { connect as netConnect, type AddressInfo } from 'node:net';

import bs58 from 'bs58';
// Independent RFC-8032 verifier (transitive via @solana/web3.js — the same
// test-only precedent as signer-core's channel.test.ts).
import { ed25519 } from '@noble/curves/ed25519';

import { buildVoucherMessage, CHANNEL_VOUCHER_PAYLOAD_GOLDEN_JSON } from '@solvela/signer-core';

import { createSidecarServer, type SidecarOptions } from '../src/server.ts';

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/** Test channel/session — the SAME fixed inputs signer-core's golden tests use. */
const CHANNEL_ID = new Uint8Array(32).fill(0x11);
const SESSION_SEED = new Uint8Array(32).fill(7);
const SESSION_PUB = ed25519.getPublicKey(SESSION_SEED);
const LOCAL_TOKEN = 'test-local-token-abc123';

const USDC_MINT = 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v';
const SOLANA_NETWORK = 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp';
const PAY_TO = bs58.encode(new Uint8Array(32).fill(1));

const QUOTE_ATOMIC = '10500';
const CURRENT_SLOT = 1_000_000;
const EXPIRY_MARGIN = 1200n;

/**
 * A real gateway names an ABSOLUTE `resource.url` in its 402 — which is NOT
 * the local request path. Every fixture below keeps the two DISTINCT on
 * purpose: the sidecar must echo the GATEWAY's value into the voucher wrapper
 * and never re-derive it from the local request, and an identical-by-accident
 * fixture makes that assertion vacuous (deleting the echo would still pass).
 */
const GATEWAY_RESOURCE_ORIGIN = 'https://api.solvela.ai';

/** The gateway's direct-shape x402 402 body (per signer-core's PaymentRequiredSchema). */
function challengeBody(
  amount: string = QUOTE_ATOMIC,
  resourceUrl = `${GATEWAY_RESOURCE_ORIGIN}/v1/messages`,
): string {
  return JSON.stringify({
    x402_version: 2,
    error: 'Payment required',
    resource: { url: resourceUrl, method: 'POST' },
    accepts: [
      {
        scheme: 'exact',
        network: SOLANA_NETWORK,
        amount,
        asset: USDC_MINT,
        pay_to: PAY_TO,
        max_timeout_seconds: 300,
      },
    ],
    cost_breakdown: {
      provider_cost: '0.010000',
      platform_fee: '0.000500',
      total: '0.010500',
      currency: 'USDC',
      fee_percent: 5.0,
    },
  });
}

/** `GET /v1/escrow/config` body. `slot: null` models the RPC-outage arm. */
function escrowConfigBody(slot: number | null = CURRENT_SLOT): string {
  return JSON.stringify({
    escrow_program_id: '9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU',
    current_slot: slot,
    network: SOLANA_NETWORK,
    usdc_mint: USDC_MINT,
    provider_wallet: PAY_TO,
  });
}

interface RecordedRequest {
  method: string;
  url: string;
  headers: IncomingMessage['headers'];
  body: Buffer;
}

interface FakeGateway {
  url: string;
  requests: RecordedRequest[];
  server: Server;
  close(): Promise<void>;
}

type FakeHandler = (
  req: RecordedRequest,
  res: ServerResponse,
  raw: IncomingMessage,
) => void | Promise<void>;

const openServers: Server[] = [];
after(async () => {
  await Promise.all(
    openServers.map(
      (s) =>
        new Promise<void>((resolve) => {
          s.closeAllConnections(); // a hung/red stream must never wedge the run
          s.close(() => resolve());
        }),
    ),
  );
});

async function startFakeGateway(handler: FakeHandler): Promise<FakeGateway> {
  const requests: RecordedRequest[] = [];
  const server = createServer(async (req, res) => {
    const chunks: Buffer[] = [];
    for await (const chunk of req) chunks.push(chunk as Buffer);
    const recorded: RecordedRequest = {
      method: req.method ?? '',
      url: req.url ?? '',
      headers: req.headers,
      body: Buffer.concat(chunks),
    };
    requests.push(recorded);
    await handler(recorded, res, req);
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  openServers.push(server);
  const port = (server.address() as AddressInfo).port;
  return {
    url: `http://127.0.0.1:${port}`,
    requests,
    server,
    close: () =>
      new Promise<void>((resolve) => {
        server.closeAllConnections();
        server.close(() => resolve());
      }),
  };
}

async function startSidecar(
  gatewayUrl: string,
  extra: Partial<SidecarOptions> = {},
): Promise<{ url: string; close(): Promise<void> }> {
  const server = createSidecarServer({
    gatewayUrl,
    localToken: LOCAL_TOKEN,
    channelId: CHANNEL_ID,
    signSeed: SESSION_SEED,
    // Fast retries so the 503/resync tests don't sleep real backoff.
    trackerOptions: { baseDelayMs: 1, maxDelayMs: 2 },
    ...extra,
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  openServers.push(server);
  const port = (server.address() as AddressInfo).port;
  return {
    url: `http://127.0.0.1:${port}`,
    close: () =>
      new Promise<void>((resolve) => {
        server.closeAllConnections();
        server.close(() => resolve());
      }),
  };
}

/** Standard "gateway" behavior: config + 402-then-serve on either paid surface. */
function standardHandler(paidResponse: (req: RecordedRequest, res: ServerResponse) => void): FakeHandler {
  return (req, res) => {
    if (req.method === 'GET' && req.url.startsWith('/v1/escrow/config')) {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(escrowConfigBody());
      return;
    }
    if (
      req.method === 'POST' &&
      (req.url.startsWith('/v1/messages') || req.url.startsWith('/v1/chat/completions'))
    ) {
      if (!req.headers['payment-signature']) {
        // A real gateway names the resource it is quoting as an ABSOLUTE url —
        // deliberately NOT the request path, so the sidecar's echo is pinned.
        res.writeHead(402, { 'content-type': 'application/json' });
        res.end(challengeBody(QUOTE_ATOMIC, `${GATEWAY_RESOURCE_ORIGIN}${req.url.split('?')[0]}`));
        return;
      }
      paidResponse(req, res);
      return;
    }
    res.writeHead(404, { 'content-type': 'application/json' });
    res.end('{"error":"not found"}');
  };
}

function decodeHeader(header: string): {
  x402_version: number;
  resource: { url: string; method: string };
  accepted: Record<string, unknown>;
  payload: Record<string, unknown>;
} {
  return JSON.parse(Buffer.from(header, 'base64').toString('utf-8'));
}

function authedPost(url: string, body: Buffer | string, headers: Record<string, string> = {}) {
  return fetch(url, {
    method: 'POST',
    headers: {
      authorization: `Bearer ${LOCAL_TOKEN}`,
      'content-type': 'application/json',
      'anthropic-version': '2023-06-01',
      ...headers,
    },
    body,
  });
}

const messagesPosts = (gw: FakeGateway) =>
  gw.requests.filter((r) => r.method === 'POST' && r.url.startsWith('/v1/messages'));

// ---------------------------------------------------------------------------
// 1. Happy path: buffered draw, byte-identical retry, verbatim relay
// ---------------------------------------------------------------------------

describe('POST /v1/messages — happy-path channel draw', () => {
  it('402 → signed retry with IDENTICAL bytes → response relayed (query preserved)', async () => {
    const gw = await startFakeGateway(
      standardHandler((_req, res) => {
        res.writeHead(200, {
          'content-type': 'application/json',
          'x-solvela-receipt': '/v1/receipts/abc',
        });
        res.end('{"id":"msg_1","type":"message"}');
      }),
    );
    let observedLast: bigint | null = null;
    const sc = await startSidecar(gw.url, {
      onLastCumulative: (last) => {
        observedLast = last;
      },
    });

    const body = Buffer.from('{"model":"claude-sonnet-4-5","max_tokens":64,"messages":[{"role":"user","content":"hi"}]}');
    const resp = await authedPost(`${sc.url}/v1/messages?beta=true`, body);

    assert.equal(resp.status, 200);
    assert.equal(await resp.text(), '{"id":"msg_1","type":"message"}');
    // Gateway response headers relayed.
    assert.equal(resp.headers.get('x-solvela-receipt'), '/v1/receipts/abc');

    const posts = messagesPosts(gw);
    assert.equal(posts.length, 2, 'exactly one unpaid forward + one paid retry');

    // Query string preserved on BOTH legs.
    assert.equal(posts[0].url, '/v1/messages?beta=true');
    assert.equal(posts[1].url, '/v1/messages?beta=true');

    // The voucher digest binds to the bytes: the retry MUST be byte-identical.
    assert.deepEqual(posts[0].body, body);
    assert.deepEqual(posts[1].body, posts[0].body);

    // First leg unpaid; retry carries the voucher header.
    assert.equal(posts[0].headers['payment-signature'], undefined);
    assert.equal(typeof posts[1].headers['payment-signature'], 'string');

    // Anthropic headers forwarded; the LOCAL token must never go upstream.
    for (const p of posts) {
      assert.equal(p.headers['anthropic-version'], '2023-06-01');
      assert.equal(p.headers['authorization'], undefined, 'local bearer leaked upstream');
      assert.equal(p.headers['x-api-key'], undefined, 'local x-api-key leaked upstream');
    }

    // The tracker advanced by exactly the quote and the persist hook saw it.
    assert.equal(observedLast, BigInt(QUOTE_ATOMIC));

    await sc.close();
    await gw.close();
  });

  it('a non-402 first response is relayed verbatim and NOTHING is signed', async () => {
    const errBody = '{"type":"error","error":{"type":"not_found_error","message":"model not found"}}';
    const gw = await startFakeGateway((req, res) => {
      if (req.method === 'GET' && req.url.startsWith('/v1/escrow/config')) {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(escrowConfigBody());
        return;
      }
      res.writeHead(400, { 'content-type': 'application/json' });
      res.end(errBody);
    });
    const sc = await startSidecar(gw.url);

    const resp = await authedPost(`${sc.url}/v1/messages`, '{"model":"nope","messages":[]}');
    assert.equal(resp.status, 400);
    assert.equal(await resp.text(), errBody);

    const posts = messagesPosts(gw);
    assert.equal(posts.length, 1, 'no retry — nothing was signed');
    assert.equal(posts[0].headers['payment-signature'], undefined);

    await sc.close();
    await gw.close();
  });
});

// ---------------------------------------------------------------------------
// 2. SSE relay — byte-identical AND incremental
// ---------------------------------------------------------------------------

describe('SSE relay', () => {
  it('pipes SSE bytes verbatim as they arrive (no buffering; signature_delta survives)', async () => {
    const chunkA =
      'event: content_block_delta\ndata: {"type":"content_block_delta","delta":{"type":"signature_delta","signature":"EqQBCgIYAhIM"}}\n\n';
    const chunkB = 'event: message_stop\ndata: {"type":"message_stop"}\n\n';

    // The incrementality proof: the fake gateway holds chunk B until the TEST
    // confirms the client received chunk A. A sidecar that buffered the stream
    // would deadlock here (bounded by the timeout below) because the client
    // could never see A before B is sent.
    let releaseChunkB!: () => void;
    const clientGotChunkA = new Promise<void>((resolve) => {
      releaseChunkB = resolve;
    });

    const gw = await startFakeGateway(
      standardHandler(async (_req, res) => {
        res.writeHead(200, { 'content-type': 'text/event-stream' });
        res.write(chunkA);
        await clientGotChunkA;
        res.write(chunkB);
        res.end();
      }),
    );
    const sc = await startSidecar(gw.url);

    const resp = await authedPost(`${sc.url}/v1/messages`, '{"model":"m","stream":true,"messages":[]}');
    assert.equal(resp.status, 200);
    assert.equal(resp.headers.get('content-type'), 'text/event-stream');

    const received: Buffer[] = [];
    const timeout = setTimeout(() => {
      throw new Error('SSE relay appears to BUFFER the stream — chunk A never reached the client');
    }, 5000);
    assert.ok(resp.body, 'relayed SSE response has a body stream');
    let releasedB = false;
    for await (const chunk of resp.body as unknown as AsyncIterable<Uint8Array>) {
      received.push(Buffer.from(chunk));
      const soFar = Buffer.concat(received).toString('utf-8');
      if (!releasedB && soFar.includes('signature_delta')) {
        releasedB = true;
        releaseChunkB();
      }
    }
    clearTimeout(timeout);

    assert.ok(releasedB, 'client received chunk A incrementally before the stream ended');
    assert.equal(
      Buffer.concat(received).toString('utf-8'),
      chunkA + chunkB,
      'relayed SSE bytes must be BYTE-IDENTICAL to what the gateway sent',
    );

    await sc.close();
    await gw.close();
  });
});

// ---------------------------------------------------------------------------
// 3. Local auth
// ---------------------------------------------------------------------------

describe('local auth', () => {
  it('rejects missing and wrong tokens with 401; accepts both header forms', async () => {
    const gw = await startFakeGateway(
      standardHandler((_req, res) => {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end('{"ok":true}');
      }),
    );
    const sc = await startSidecar(gw.url);
    const body = '{"model":"m","messages":[]}';

    // No token.
    let resp = await fetch(`${sc.url}/v1/messages`, { method: 'POST', body });
    assert.equal(resp.status, 401);
    await resp.text();

    // Wrong bearer.
    resp = await fetch(`${sc.url}/v1/messages`, {
      method: 'POST',
      headers: { authorization: 'Bearer wrong-token' },
      body,
    });
    assert.equal(resp.status, 401);
    await resp.text();

    // Wrong x-api-key.
    resp = await fetch(`${sc.url}/v1/messages`, {
      method: 'POST',
      headers: { 'x-api-key': 'nope' },
      body,
    });
    assert.equal(resp.status, 401);
    await resp.text();

    // None of the rejected requests reached the gateway.
    assert.equal(messagesPosts(gw).length, 0);

    // Correct token via Authorization: Bearer.
    resp = await authedPost(`${sc.url}/v1/messages`, body);
    assert.equal(resp.status, 200);
    await resp.text();

    // Correct token via x-api-key (the other env var Claude Code may use).
    resp = await fetch(`${sc.url}/v1/messages`, {
      method: 'POST',
      headers: { 'x-api-key': LOCAL_TOKEN, 'content-type': 'application/json' },
      body,
    });
    assert.equal(resp.status, 200);
    await resp.text();

    await sc.close();
    await gw.close();
  });
});

// ---------------------------------------------------------------------------
// 4. Structured resync + 503 lock-busy retry
// ---------------------------------------------------------------------------

describe('draw retry semantics', () => {
  it('re-bases on a structured error.last_cumulative and succeeds', async () => {
    const paidAttempts: bigint[] = [];
    const gw = await startFakeGateway(
      standardHandler((req, res) => {
        const decoded = decodeHeader(String(req.headers['payment-signature']));
        paidAttempts.push(BigInt(decoded.payload.cumulative_atomic as number));
        if (paidAttempts.length === 1) {
          // Gateway knows better: authoritative last_cumulative = 5000.
          res.writeHead(402, { 'content-type': 'application/json' });
          res.end(
            JSON.stringify({
              error: {
                message: 'channel voucher rejected; resync from authoritative last_cumulative=5000',
                type: 'invalid_payment',
                last_cumulative: '5000',
              },
            }),
          );
          return;
        }
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end('{"id":"msg_2"}');
      }),
    );
    const sc = await startSidecar(gw.url);

    const resp = await authedPost(`${sc.url}/v1/messages`, '{"model":"m","messages":[]}');
    assert.equal(resp.status, 200);
    assert.equal(await resp.text(), '{"id":"msg_2"}');

    // Attempt 1 signed 0 + quote; attempt 2 re-based to 5000 + quote.
    assert.deepEqual(paidAttempts, [BigInt(QUOTE_ATOMIC), 5000n + BigInt(QUOTE_ATOMIC)]);

    await sc.close();
    await gw.close();
  });

  it('retries the SAME cumulative on a 503 lock-busy and succeeds', async () => {
    const paidAttempts: bigint[] = [];
    const gw = await startFakeGateway(
      standardHandler((req, res) => {
        const decoded = decodeHeader(String(req.headers['payment-signature']));
        paidAttempts.push(BigInt(decoded.payload.cumulative_atomic as number));
        if (paidAttempts.length === 1) {
          res.writeHead(503, { 'content-type': 'application/json' });
          res.end('{"error":{"message":"a draw is already in progress on this channel"}}');
          return;
        }
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end('{"id":"msg_3"}');
      }),
    );
    const sc = await startSidecar(gw.url);

    const resp = await authedPost(`${sc.url}/v1/messages`, '{"model":"m","messages":[]}');
    assert.equal(resp.status, 200);

    assert.equal(paidAttempts.length, 2);
    assert.equal(paidAttempts[0], paidAttempts[1], '503 retry must reuse the SAME cumulative');

    await sc.close();
    await gw.close();
  });
});

// ---------------------------------------------------------------------------
// 5. Slot source fail-closed
// ---------------------------------------------------------------------------

describe('expiry-slot sourcing (fail closed)', () => {
  it('refuses the draw when current_slot is null — nothing signed', async () => {
    const gw = await startFakeGateway((req, res) => {
      if (req.method === 'GET' && req.url.startsWith('/v1/escrow/config')) {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(escrowConfigBody(null));
        return;
      }
      if (req.method === 'POST' && req.url.startsWith('/v1/messages')) {
        res.writeHead(402, { 'content-type': 'application/json' });
        res.end(challengeBody());
        return;
      }
      res.writeHead(404).end();
    });
    const sc = await startSidecar(gw.url);

    const resp = await authedPost(`${sc.url}/v1/messages`, '{"model":"m","messages":[]}');
    assert.equal(resp.status, 503);
    const err = (await resp.json()) as { type: string; error: { message: string } };
    assert.equal(err.type, 'error');
    assert.match(err.error.message, /slot/i);

    // The unpaid forward happened, but NO signed retry ever did.
    const posts = messagesPosts(gw);
    assert.equal(posts.length, 1);
    assert.equal(posts[0].headers['payment-signature'], undefined);

    await sc.close();
    await gw.close();
  });

  it('refuses the draw when /v1/escrow/config is 404 (escrow unconfigured)', async () => {
    const gw = await startFakeGateway((req, res) => {
      if (req.method === 'GET' && req.url.startsWith('/v1/escrow/config')) {
        res.writeHead(404, { 'content-type': 'application/json' });
        res.end('{"error":"escrow not configured"}');
        return;
      }
      if (req.method === 'POST' && req.url.startsWith('/v1/messages')) {
        res.writeHead(402, { 'content-type': 'application/json' });
        res.end(challengeBody());
        return;
      }
      res.writeHead(404).end();
    });
    const sc = await startSidecar(gw.url);

    const resp = await authedPost(`${sc.url}/v1/messages`, '{"model":"m","messages":[]}');
    assert.equal(resp.status, 503);
    await resp.text();

    const posts = messagesPosts(gw);
    assert.equal(posts.length, 1, 'no signed retry after a config 404');
    assert.equal(posts[0].headers['payment-signature'], undefined);

    await sc.close();
    await gw.close();
  });
});

// ---------------------------------------------------------------------------
// 6. count_tokens (free, verbatim) + unknown paths
// ---------------------------------------------------------------------------

describe('count_tokens + routing', () => {
  it('forwards count_tokens verbatim with NO payment header and relays the response', async () => {
    const gw = await startFakeGateway((req, res) => {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end('{"input_tokens":42}');
    });
    const sc = await startSidecar(gw.url);

    const body = '{"model":"claude-sonnet-4-5","messages":[{"role":"user","content":"hi"}]}';
    const resp = await authedPost(`${sc.url}/v1/messages/count_tokens`, body);
    assert.equal(resp.status, 200);
    assert.equal(await resp.text(), '{"input_tokens":42}');

    assert.equal(gw.requests.length, 1);
    const fwd = gw.requests[0];
    assert.equal(fwd.url, '/v1/messages/count_tokens');
    assert.equal(fwd.body.toString('utf-8'), body);
    assert.equal(fwd.headers['payment-signature'], undefined, 'count_tokens is free — never signed');
    assert.equal(fwd.headers['authorization'], undefined);
    assert.equal(fwd.headers['x-api-key'], undefined);

    await sc.close();
    await gw.close();
  });

  it('returns 404 for any other path and never contacts the gateway', async () => {
    const gw = await startFakeGateway((_req, res) => {
      res.writeHead(200).end('should never be reached');
    });
    const sc = await startSidecar(gw.url);

    const resp = await authedPost(`${sc.url}/v1/embeddings`, '{}');
    assert.equal(resp.status, 404);
    await resp.text();
    const getResp = await fetch(`${sc.url}/v1/messages`, {
      headers: { authorization: `Bearer ${LOCAL_TOKEN}` },
    });
    assert.equal(getResp.status, 404);
    await getResp.text();

    assert.equal(gw.requests.length, 0);

    await sc.close();
    await gw.close();
  });
});

// ---------------------------------------------------------------------------
// 6b. OpenAI-compatible surface — the SAME money path on /v1/chat/completions
// ---------------------------------------------------------------------------

const chatPosts = (gw: FakeGateway) =>
  gw.requests.filter((r) => r.method === 'POST' && r.url.startsWith('/v1/chat/completions'));

describe('POST /v1/chat/completions — OpenAI-compatible surface', () => {
  it('runs the identical draw: 402 → signed retry with IDENTICAL bytes → verbatim relay', async () => {
    const gw = await startFakeGateway(
      standardHandler((_req, res) => {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end('{"id":"chatcmpl-1","object":"chat.completion"}');
      }),
    );
    const sc = await startSidecar(gw.url);

    const body = Buffer.from('{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}]}');
    const resp = await authedPost(`${sc.url}/v1/chat/completions?stream=false`, body);

    assert.equal(resp.status, 200);
    assert.equal(await resp.text(), '{"id":"chatcmpl-1","object":"chat.completion"}');

    const posts = chatPosts(gw);
    assert.equal(posts.length, 2, 'exactly one unpaid forward + one paid retry');

    // Query string preserved on BOTH legs.
    assert.equal(posts[0].url, '/v1/chat/completions?stream=false');
    assert.equal(posts[1].url, '/v1/chat/completions?stream=false');

    // The voucher digest binds to the bytes: the retry MUST be byte-identical.
    assert.deepEqual(posts[0].body, body);
    assert.deepEqual(posts[1].body, posts[0].body);

    // First leg unpaid; retry carries the voucher. The LOCAL token never leaves.
    assert.equal(posts[0].headers['payment-signature'], undefined);
    assert.equal(typeof posts[1].headers['payment-signature'], 'string');
    for (const p of posts) {
      assert.equal(p.headers['authorization'], undefined);
      assert.equal(p.headers['x-api-key'], undefined);
    }

    await sc.close();
    await gw.close();
  });

  it("binds the voucher to the GATEWAY's 402 resource url, not the local request path", async () => {
    const gw = await startFakeGateway(
      standardHandler((_req, res) => {
        res.writeHead(200, { 'content-type': 'application/json' }).end('{}');
      }),
    );
    const sc = await startSidecar(gw.url);

    await (await authedPost(`${sc.url}/v1/chat/completions`, '{"model":"m","messages":[]}')).text();

    const paid = chatPosts(gw)[1];
    const decoded = decodeHeader(paid.headers['payment-signature'] as string);
    // The gateway's value — which DIFFERS from the request path, so this dies
    // if the echo is dropped and the local pathname is substituted.
    assert.equal(decoded.resource.url, `${GATEWAY_RESOURCE_ORIGIN}/v1/chat/completions`);
    assert.notEqual(decoded.resource.url, '/v1/chat/completions');

    await sc.close();
    await gw.close();
  });

  it('emits an OpenAI-shaped error envelope, not the Anthropic one', async () => {
    const gw = await startFakeGateway((_req, res) => {
      res.writeHead(200).end('should never be reached');
    });
    const sc = await startSidecar(gw.url);

    // Bad local token → 401 before any dispatch.
    const resp = await fetch(`${sc.url}/v1/chat/completions`, {
      method: 'POST',
      headers: { authorization: 'Bearer wrong-token', 'content-type': 'application/json' },
      body: '{}',
    });
    assert.equal(resp.status, 401);
    const parsed = (await resp.json()) as { type?: string; error: { type: string; message: string } };

    // OpenAI envelope: a bare `error` object, NO top-level `type: "error"`.
    assert.ok(!('type' in parsed), 'Anthropic-only top-level `type` must not appear');
    assert.equal(parsed.error.type, 'authentication_error');
    assert.equal(typeof parsed.error.message, 'string');

    assert.equal(gw.requests.length, 0);
    await sc.close();
    await gw.close();
  });

  it('leaves the Anthropic envelope on /v1/messages untouched (regression guard)', async () => {
    const gw = await startFakeGateway((_req, res) => {
      res.writeHead(200).end('should never be reached');
    });
    const sc = await startSidecar(gw.url);

    const resp = await fetch(`${sc.url}/v1/messages`, {
      method: 'POST',
      headers: { authorization: 'Bearer wrong-token', 'content-type': 'application/json' },
      body: '{}',
    });
    assert.equal(resp.status, 401);
    const parsed = (await resp.json()) as { type: string; error: { type: string } };

    assert.equal(parsed.type, 'error', 'Claude Code depends on the top-level `type: "error"`');
    assert.equal(parsed.error.type, 'authentication_error');

    await sc.close();
    await gw.close();
  });
});

// ---------------------------------------------------------------------------
// 7. Voucher header golden check (reuses signer-core's pinned constants)
// ---------------------------------------------------------------------------

describe('PAYMENT-SIGNATURE golden shape', () => {
  it('decodes to the ChannelVoucherPayload shape crates/protocol pins, signed by the session key', async () => {
    const gw = await startFakeGateway(
      standardHandler((_req, res) => {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end('{"id":"msg_g"}');
      }),
    );
    const sc = await startSidecar(gw.url);

    const body = Buffer.from('{"model":"claude-sonnet-4-5","messages":[{"role":"user","content":"golden"}]}');
    const resp = await authedPost(`${sc.url}/v1/messages`, body);
    assert.equal(resp.status, 200);
    await resp.text();

    const paid = messagesPosts(gw).find((r) => r.headers['payment-signature']);
    assert.ok(paid, 'a paid retry with PAYMENT-SIGNATURE reached the gateway');
    const decoded = decodeHeader(String(paid.headers['payment-signature']));

    // Wrapper: PaymentPayload shape, scheme FORCED to channel, accept fields echoed.
    assert.equal(decoded.x402_version, 2);
    // The GATEWAY's resource url, echoed verbatim (it differs from the local
    // request path `/v1/messages` — see GATEWAY_RESOURCE_ORIGIN).
    assert.deepEqual(decoded.resource, {
      url: `${GATEWAY_RESOURCE_ORIGIN}/v1/messages`,
      method: 'POST',
    });
    assert.equal(decoded.accepted.scheme, 'channel');
    assert.equal(decoded.accepted.amount, QUOTE_ATOMIC);
    assert.equal(decoded.accepted.pay_to, PAY_TO);
    assert.equal(decoded.accepted.asset, USDC_MINT);
    assert.equal(decoded.accepted.network, SOLANA_NETWORK);

    // Payload: EXACT field set + order pinned by the protocol golden JSON —
    // reuse signer-core's constant, never a hand-written list.
    const goldenKeys = Object.keys(JSON.parse(CHANNEL_VOUCHER_PAYLOAD_GOLDEN_JSON));
    assert.deepEqual(Object.keys(decoded.payload), goldenKeys);

    const p = decoded.payload as {
      channel_id: string;
      cumulative_atomic: number;
      expiry_slot: number;
      nonce: number;
      request_digest: string;
      signature: string;
    };
    assert.equal(p.channel_id, bs58.encode(CHANNEL_ID));
    assert.equal(p.cumulative_atomic, Number(QUOTE_ATOMIC)); // first draw: 0 + quote
    assert.equal(p.expiry_slot, CURRENT_SLOT + Number(EXPIRY_MARGIN));
    assert.equal(p.nonce, 0);
    assert.equal(
      p.request_digest,
      createHash('sha256').update(body).digest('base64'),
      'request_digest must be SHA-256 of the exact request bytes',
    );

    // Signature verifies over the canonical 114-byte voucher message rebuilt
    // from the wire fields — via signer-core's golden-pinned builder and an
    // independent RFC-8032 verifier.
    const message = buildVoucherMessage(
      bs58.decode(p.channel_id),
      BigInt(p.cumulative_atomic),
      BigInt(p.expiry_slot),
      BigInt(p.nonce),
      new Uint8Array(Buffer.from(p.request_digest, 'base64')),
    );
    const sig = new Uint8Array(Buffer.from(p.signature, 'base64'));
    assert.equal(sig.length, 64);
    assert.ok(
      ed25519.verify(sig, message, SESSION_PUB),
      'voucher signature must verify against the session key over the canonical message',
    );

    await sc.close();
    await gw.close();
  });

  it('fails closed when the 402 offers no exact entry (no silent scheme pick)', async () => {
    const gw = await startFakeGateway((req, res) => {
      if (req.method === 'GET' && req.url.startsWith('/v1/escrow/config')) {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(escrowConfigBody());
        return;
      }
      const noExact = JSON.parse(challengeBody());
      noExact.accepts[0].scheme = 'escrow';
      noExact.accepts[0].escrow_program_id = '9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU';
      res.writeHead(402, { 'content-type': 'application/json' });
      res.end(JSON.stringify(noExact));
    });
    const sc = await startSidecar(gw.url);

    const resp = await authedPost(`${sc.url}/v1/messages`, '{"model":"m","messages":[]}');
    assert.equal(resp.status, 502);
    const err = (await resp.json()) as { error: { message: string } };
    assert.match(err.error.message, /exact/i);

    // Only the unpaid forward — nothing was signed against a non-exact quote.
    assert.equal(messagesPosts(gw).length, 1);

    await sc.close();
    await gw.close();
  });
});

// ---------------------------------------------------------------------------
// 8. Lifecycle + relay fidelity (round-1 review findings)
// ---------------------------------------------------------------------------

describe('lifecycle + relay fidelity', () => {
  it('client abort mid-SSE aborts the upstream read (gateway observes the close)', async () => {
    let upstreamClosed!: () => void;
    const gone = new Promise<void>((resolve) => {
      upstreamClosed = resolve;
    });
    const gw = await startFakeGateway(
      standardHandler((_req, res) => {
        res.writeHead(200, { 'content-type': 'text/event-stream' });
        res.write('event: ping\ndata: {}\n\n');
        res.on('close', upstreamClosed);
        // Never end — a full-rate stream the client walks away from.
      }),
    );
    const sc = await startSidecar(gw.url);

    const ac = new AbortController();
    const resp = await fetch(`${sc.url}/v1/messages`, {
      method: 'POST',
      headers: { authorization: `Bearer ${LOCAL_TOKEN}`, 'content-type': 'application/json' },
      body: '{"model":"m","stream":true,"messages":[]}',
      signal: ac.signal,
    });
    assert.equal(resp.status, 200);
    const reader = (resp.body as ReadableStream<Uint8Array>).getReader();
    await reader.read(); // first chunk arrived — the stream is live
    ac.abort();

    await Promise.race([
      gone,
      new Promise((_, reject) =>
        setTimeout(() => reject(new Error('upstream kept streaming after client abort')), 3000),
      ),
    ]);

    await sc.close();
    await gw.close();
  });

  it('upstream body error mid-SSE terminates the client abnormally, not as a clean end', async () => {
    const gw = await startFakeGateway(
      standardHandler((_req, res) => {
        res.writeHead(200, { 'content-type': 'text/event-stream' });
        res.write('event: ping\ndata: {}\n\n');
        setTimeout(() => res.destroy(), 20); // upstream dies mid-stream
      }),
    );
    const sc = await startSidecar(gw.url);

    const resp = await authedPost(`${sc.url}/v1/messages`, '{"model":"m","stream":true,"messages":[]}');
    assert.equal(resp.status, 200);
    await assert.rejects(
      (async () => {
        for await (const _ of resp.body as unknown as AsyncIterable<Uint8Array>) {
          void _;
        }
      })(),
      undefined,
      'a truncated upstream must NOT read as a clean stream end to the client',
    );

    await sc.close();
    await gw.close();
  });

  it('a paid-retry fetch that rejects outright fails closed with a 502', async () => {
    const gw = await startFakeGateway((req, res, raw) => {
      if (req.method === 'GET' && req.url.startsWith('/v1/escrow/config')) {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(escrowConfigBody());
        return;
      }
      if (req.method === 'POST' && req.url.startsWith('/v1/messages')) {
        if (!req.headers['payment-signature']) {
          res.writeHead(402, { 'content-type': 'application/json' });
          res.end(challengeBody());
          return;
        }
        raw.socket.destroy(); // the retry never gets a response — fetch rejects
        return;
      }
      res.writeHead(404).end();
    });
    const sc = await startSidecar(gw.url);

    const resp = await authedPost(`${sc.url}/v1/messages`, '{"model":"m","messages":[]}');
    assert.equal(resp.status, 502);
    const err = (await resp.json()) as { type: string; error: { type: string } };
    assert.equal(err.type, 'error');

    await sc.close();
    await gw.close();
  });

  it('a paid-retry fetch that rejects outright answers 502 in the OpenAI envelope', async () => {
    // The OUTER catch (the createServer listener's `.catch`) is reachable from
    // an OpenAI client three ways — the paid-retry fetch rejecting, the serve
    // deadline firing, `relay()`'s writeHead throwing — and every one of them
    // was previously pinned ONLY on /v1/messages. Hardcoding `'anthropic'` on
    // that 502 line therefore survived the whole suite.
    const gw = await startFakeGateway((req, res, raw) => {
      if (req.method === 'GET' && req.url.startsWith('/v1/escrow/config')) {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(escrowConfigBody());
        return;
      }
      if (req.method === 'POST' && req.url.startsWith('/v1/chat/completions')) {
        if (!req.headers['payment-signature']) {
          res.writeHead(402, { 'content-type': 'application/json' });
          res.end(challengeBody(QUOTE_ATOMIC, `${GATEWAY_RESOURCE_ORIGIN}/v1/chat/completions`));
          return;
        }
        raw.socket.destroy(); // the retry never gets a response — fetch rejects
        return;
      }
      res.writeHead(404).end();
    });
    const sc = await startSidecar(gw.url);

    const resp = await authedPost(`${sc.url}/v1/chat/completions`, '{"model":"m","messages":[]}');
    assert.equal(resp.status, 502);
    const parsed = (await resp.json()) as { type?: string; error: { type: string } };
    assert.ok(!('type' in parsed), 'the outer catch must answer in the CALLER\'s dialect');
    assert.equal(parsed.error.type, 'server_error');

    await sc.close();
    await gw.close();
  });

  it('maps sidecar error statuses to Anthropic error types (401/503)', async () => {
    // One slot-null fake serves both assertions: unauthenticated -> 401
    // authentication_error; authenticated draw with no slot -> 503
    // overloaded_error.
    const gw = await startFakeGateway((req, res) => {
      if (req.method === 'GET' && req.url.startsWith('/v1/escrow/config')) {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(escrowConfigBody(null));
        return;
      }
      res.writeHead(402, { 'content-type': 'application/json' });
      res.end(challengeBody());
    });
    const sc = await startSidecar(gw.url);

    const unauth = await fetch(`${sc.url}/v1/messages`, { method: 'POST', body: '{}' });
    assert.equal(unauth.status, 401);
    const unauthBody = (await unauth.json()) as { error: { type: string } };
    assert.equal(unauthBody.error.type, 'authentication_error');

    const noSlot = await authedPost(`${sc.url}/v1/messages`, '{"model":"m","messages":[]}');
    assert.equal(noSlot.status, 503);
    const noSlotBody = (await noSlot.json()) as { error: { type: string } };
    assert.equal(noSlotBody.error.type, 'overloaded_error');

    await sc.close();
    await gw.close();
  });

  it('relays duplicate set-cookie headers without collapsing them', async () => {
    const gw = await startFakeGateway(
      standardHandler((_req, res) => {
        res.setHeader('set-cookie', ['a=1; Path=/', 'b=2; Path=/']);
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end('{"id":"msg_c"}');
      }),
    );
    const sc = await startSidecar(gw.url);

    const resp = await authedPost(`${sc.url}/v1/messages`, '{"model":"m","messages":[]}');
    assert.equal(resp.status, 200);
    await resp.text();
    assert.deepEqual(resp.headers.getSetCookie(), ['a=1; Path=/', 'b=2; Path=/']);

    await sc.close();
    await gw.close();
  });
});

// ---------------------------------------------------------------------------
// 9. Malformed request targets — a request LINE must never kill the sidecar
// ---------------------------------------------------------------------------

/**
 * Send a RAW request line. `fetch` cannot express an absolute-form request
 * target (`POST http://a:b:c/x HTTP/1.1`), and Node's HTTP parser accepts
 * several of those that the WHATWG `URL` parser rejects with
 * ERR_INVALID_URL — thrown SYNCHRONOUSLY inside the `createServer` listener,
 * where (with no `uncaughtException` handler in this package) it EXITS the
 * signing sidecar. Pre-auth, so any local process could kill it token-less.
 */
function rawRequest(
  sidecarUrl: string,
  requestLine: string,
  headerLines: string[],
): Promise<string> {
  const port = Number(new URL(sidecarUrl).port);
  return new Promise((resolve, reject) => {
    const socket = netConnect(port, '127.0.0.1', () => {
      socket.end([requestLine, ...headerLines, '', ''].join('\r\n'));
    });
    let out = '';
    socket.setEncoding('utf-8');
    socket.setTimeout(5000, () => {
      socket.destroy();
      reject(new Error(`no response to raw request line: ${requestLine}`));
    });
    socket.on('data', (d) => {
      out += d;
    });
    socket.on('close', () => resolve(out));
    socket.on('error', reject);
  });
}

/** Split a raw HTTP response into its status line and parsed JSON body. Node
 * frames these chunked (no content-length), so the JSON object is lifted out
 * of the raw text rather than de-chunked — enough for an error envelope. */
function parseRawResponse(raw: string): { status: number; body: unknown } {
  const status = Number(/^HTTP\/1\.1 (\d{3}) /.exec(raw)?.[1] ?? 0);
  const start = raw.indexOf('{');
  const end = raw.lastIndexOf('}');
  return {
    status,
    body: start < 0 || end < start ? undefined : JSON.parse(raw.slice(start, end + 1)),
  };
}

const MALFORMED_HEADERS = [
  'Host: 127.0.0.1',
  `Authorization: Bearer ${LOCAL_TOKEN}`,
  'Content-Type: application/json',
  'Content-Length: 0',
  'Connection: close',
];

describe('malformed request targets', () => {
  it('answers 400 (not a misdirecting 404) and STAYS ALIVE on targets the URL parser rejects', async () => {
    const gw = await startFakeGateway(
      standardHandler((_req, res) => {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end('{"id":"msg_alive"}');
      }),
    );
    const sc = await startSidecar(gw.url);

    // Node's HTTP parser accepts every one of these as `req.url`;
    // `new URL(target, base)` throws ERR_INVALID_URL on all of them. An
    // unparseable request TARGET is a client protocol error (400), not a
    // routing miss (404) — and the 404 body's "the sidecar serves
    // POST /v1/messages, …" text actively misdirects.
    for (const target of ['http://a:b:c/v1/messages', 'http://[::1/x', 'http://%/x', 'http://[]/x']) {
      const raw = await rawRequest(sc.url, `POST ${target} HTTP/1.1`, MALFORMED_HEADERS);
      const { status, body } = parseRawResponse(raw);
      assert.equal(status, 400, `expected 400 for ${target}, got: ${raw.slice(0, 160)}`);
      const err = body as { error: { type: string; message: string } };
      assert.equal(err.error.type, 'invalid_request_error');
      assert.doesNotMatch(
        err.error.message,
        /the sidecar serves POST/,
        'the 404 routing text misdirects on a malformed request target',
      );
    }

    // An unparseable target can match no PAID_PATHS/FREE_PATH entry, so the
    // gateway is never contacted and nothing is ever signed.
    assert.equal(gw.requests.length, 0);

    // The process survived: the sidecar still serves the money path.
    const resp = await authedPost(`${sc.url}/v1/messages`, '{"model":"m","messages":[]}');
    assert.equal(resp.status, 200);
    assert.equal(await resp.text(), '{"id":"msg_alive"}');

    await sc.close();
    await gw.close();
  });

  it('answers a malformed /v1/chat/ target in the OpenAI envelope, not the Anthropic one', async () => {
    // Collapsing an unparseable target to `/` forced `dialectFor('/')` →
    // anthropic, so an OpenAI client got `{type:"error",error:{…}}`.
    const gw = await startFakeGateway((_req, res) => {
      res.writeHead(200).end('should never be reached');
    });
    const sc = await startSidecar(gw.url);

    const openai = parseRawResponse(
      await rawRequest(sc.url, 'POST http://a:b:c/v1/chat/completions HTTP/1.1', MALFORMED_HEADERS),
    );
    assert.equal(openai.status, 400);
    assert.ok(!('type' in (openai.body as object)), 'OpenAI envelope has no top-level `type`');
    assert.equal((openai.body as { error: { type: string } }).error.type, 'invalid_request_error');

    const anthropic = parseRawResponse(
      await rawRequest(sc.url, 'POST http://a:b:c/v1/messages HTTP/1.1', MALFORMED_HEADERS),
    );
    assert.equal(anthropic.status, 400);
    assert.equal((anthropic.body as { type: string }).type, 'error');

    assert.equal(gw.requests.length, 0);

    await sc.close();
    await gw.close();
  });

  it('logs the raw target ONCE per burst, truncated — never one stderr write per request', async () => {
    // Reviewer 1: unbounded malformed request lines must not become unbounded
    // stderr writes. Silence is equally wrong — a burst against a signing-key
    // holder has to be visible somewhere.
    const gw = await startFakeGateway((_req, res) => {
      res.writeHead(200).end('should never be reached');
    });
    const sc = await startSidecar(gw.url);

    const logged: string[] = [];
    const realError = console.error;
    console.error = (...args: unknown[]) => {
      logged.push(args.map(String).join(' '));
    };
    try {
      // First one carries a 5000-char target: the log line must be truncated.
      const targets = [
        `http://a:b:c/${'v'.repeat(5000)}`,
        ...Array.from({ length: 19 }, () => 'http://a:b:c/v1/messages'),
      ];
      for (const target of targets) {
        const { status } = parseRawResponse(
          await rawRequest(sc.url, `POST ${target} HTTP/1.1`, MALFORMED_HEADERS),
        );
        assert.equal(status, 400);
      }
    } finally {
      console.error = realError;
    }

    const lines = logged.filter((l) => l.includes('unparseable request target'));
    assert.equal(lines.length, 1, `20 malformed targets produced ${lines.length} stderr writes`);
    assert.ok(lines[0].length < 400, `log line not truncated: ${lines[0].length} chars`);
    assert.ok(lines[0].includes('vvvv'), 'the raw target is what an operator needs to see');

    await sc.close();
    await gw.close();
  });
});

// ---------------------------------------------------------------------------
// 10. Free-tier (no-402) relay must not ride the short probe deadline
// ---------------------------------------------------------------------------

describe('non-402 relay deadline', () => {
  // >10s by construction: the bug is the 10s SHORT_TIMEOUT_MS staying attached
  // to a relayed response, so only a stream that outlives it can prove the fix.
  it('does not guillotine a long free-tier (no-402) stream at the probe deadline', async () => {
    // The gateway's zero-cost bypass (`is_free_estimate`,
    // crates/gateway/src/routes/chat/mod.rs) serves free-tier models — the
    // 0.0/0.0-priced NVIDIA models in config/models.toml — as a full streaming
    // 200 with NO 402 at all. Reproduced: SSE cut at 10.0s, no `[DONE]`.
    let stopped: (() => void) | undefined;
    const gw = await startFakeGateway((req, res) => {
      if (req.method === 'GET' && req.url.startsWith('/v1/escrow/config')) {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(escrowConfigBody());
        return;
      }
      res.writeHead(200, { 'content-type': 'text/event-stream' });
      res.write('data: {"choices":[{"delta":{"content":"tok"}}]}\n\n');
      const timer = setTimeout(() => {
        res.write('data: [DONE]\n\n');
        res.end();
      }, 11_000);
      stopped = () => clearTimeout(timer);
    });
    const sc = await startSidecar(gw.url);

    const resp = await authedPost(
      `${sc.url}/v1/chat/completions`,
      '{"model":"nvidia/free","stream":true,"messages":[]}',
    );
    assert.equal(resp.status, 200);

    let received = '';
    for await (const chunk of resp.body as unknown as AsyncIterable<Uint8Array>) {
      received += Buffer.from(chunk).toString('utf-8');
    }
    assert.match(received, /\[DONE\]/, 'free-tier stream was truncated before [DONE]');

    // A free 200 is relayed as-is — no quote, no voucher, nothing signed.
    assert.equal(chatPosts(gw).length, 1);
    assert.equal(chatPosts(gw)[0].headers['payment-signature'], undefined);

    stopped?.();
    await sc.close();
    await gw.close();
  });

  // The streaming case above CANNOT catch the real bug: a streaming gateway
  // emits response HEADERS on the first token, so `await fetch(...)` settles
  // early and any re-arm after it lands in time. `stream:false` is the OpenAI
  // DEFAULT, and a non-streaming completion sends NO headers until the whole
  // body is finished — one deadline governs the entire request, and a re-arm
  // placed after the fetch promise is never reached. Reproduced on the
  // pre-fix build: `status=502 after 10045ms`.
  it('does not guillotine a slow NON-streaming free-tier (no-402) 200', async () => {
    const completion = '{"id":"chatcmpl-slow","object":"chat.completion","choices":[]}';
    let stopped: (() => void) | undefined;
    const gw = await startFakeGateway((req, res) => {
      if (req.method === 'GET' && req.url.startsWith('/v1/escrow/config')) {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(escrowConfigBody());
        return;
      }
      // Headers AND body together, after the delay — exactly what a
      // non-streaming upstream completion looks like on the wire.
      const timer = setTimeout(() => {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(completion);
      }, 11_000);
      stopped = () => clearTimeout(timer);
    });
    const sc = await startSidecar(gw.url);

    const resp = await authedPost(
      `${sc.url}/v1/chat/completions`,
      '{"model":"nvidia/free","messages":[{"role":"user","content":"hi"}]}',
    );
    assert.equal(resp.status, 200, 'a slow non-streaming free-tier 200 must not become a 502');
    assert.equal(await resp.text(), completion);

    // A free 200 is relayed as-is — no quote, no voucher, nothing signed.
    assert.equal(chatPosts(gw).length, 1);
    assert.equal(chatPosts(gw)[0].headers['payment-signature'], undefined);

    stopped?.();
    await sc.close();
    await gw.close();
  });

  // Same bug, sibling leg: count_tokens attached the short deadline and kept
  // it through `relay()`. Claude Code calls count_tokens before large prompts.
  it('does not guillotine a slow count_tokens forward', async () => {
    const counted = '{"input_tokens":180000}';
    let stopped: (() => void) | undefined;
    const gw = await startFakeGateway((_req, res) => {
      const timer = setTimeout(() => {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(counted);
      }, 11_000);
      stopped = () => clearTimeout(timer);
    });
    const sc = await startSidecar(gw.url);

    const resp = await authedPost(
      `${sc.url}/v1/messages/count_tokens`,
      '{"model":"claude-sonnet-4-5","messages":[{"role":"user","content":"…big…"}]}',
    );
    assert.equal(resp.status, 200, 'a slow count_tokens must not become a 502');
    assert.equal(await resp.text(), counted);

    stopped?.();
    await sc.close();
    await gw.close();
  });
});

// ---------------------------------------------------------------------------
// 10b. A 402 with no `resource` is a malformed quote — fail closed
// ---------------------------------------------------------------------------

describe('402 without a resource url', () => {
  it('refuses to sign a voucher bound to a url the gateway never named', async () => {
    // `resource` is `optional(...)` in signer-core's PaymentRequiredSchema, so
    // this is a LIVE branch. Substituting the local `url.pathname` would sign a
    // draw whose wrapper declares a resource the gateway never quoted — a
    // silent invention on the money path.
    const gw = await startFakeGateway((req, res) => {
      if (req.method === 'GET' && req.url.startsWith('/v1/escrow/config')) {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(escrowConfigBody());
        return;
      }
      const noResource = JSON.parse(challengeBody()) as Record<string, unknown>;
      delete noResource.resource;
      res.writeHead(402, { 'content-type': 'application/json' });
      res.end(JSON.stringify(noResource));
    });
    const sc = await startSidecar(gw.url);

    const resp = await authedPost(`${sc.url}/v1/chat/completions`, '{"model":"m","messages":[]}');
    assert.equal(resp.status, 502);
    const parsed = (await resp.json()) as { type?: string; error: { type: string; message: string } };
    assert.ok(!('type' in parsed), 'OpenAI envelope');
    assert.match(parsed.error.message, /resource/i);

    // Only the unpaid probe — NOTHING was signed against a resource-less quote.
    assert.equal(chatPosts(gw).length, 1, 'a resource-less 402 must not produce a signed retry');
    assert.equal(chatPosts(gw)[0].headers['payment-signature'], undefined);

    await sc.close();
    await gw.close();
  });
});

// ---------------------------------------------------------------------------
// 11. OpenAI error-type mapping (every arm, not just 401)
// ---------------------------------------------------------------------------

/** POST a body larger than MAX_BODY_BYTES (32 MiB) — `fetch` is unhelpful here
 * because the sidecar answers mid-upload. */
function oversizedPost(url: string): Promise<{ status: number; body: string }> {
  return new Promise((resolve, reject) => {
    const req = httpRequest(url, {
      method: 'POST',
      headers: { authorization: `Bearer ${LOCAL_TOKEN}`, 'content-type': 'application/json' },
    });
    req.on('response', (res) => {
      let body = '';
      res.setEncoding('utf-8');
      res.on('data', (c) => {
        body += c;
      });
      res.on('end', () => {
        req.destroy();
        resolve({ status: res.statusCode ?? 0, body });
      });
    });
    req.on('error', reject);
    const chunk = Buffer.alloc(1024 * 1024, 0x20);
    let written = 0;
    const pump = () => {
      while (written < 33) {
        written += 1;
        if (!req.write(chunk)) {
          req.once('drain', pump);
          return;
        }
      }
      req.end();
    };
    pump();
  });
}

describe('OpenAI error-type mapping', () => {
  it('413 from the OUTER catch answers in the OpenAI envelope', async () => {
    // BodyTooLargeError escapes `handle()` and is caught by the createServer
    // listener's `.catch` — the ONLY consumer of the outer dialect derivation.
    const gw = await startFakeGateway((_req, res) => {
      res.writeHead(200).end('should never be reached');
    });
    const sc = await startSidecar(gw.url);

    const { status, body } = await oversizedPost(`${sc.url}/v1/chat/completions`);
    assert.equal(status, 413);
    const parsed = JSON.parse(body) as { type?: string; error: { type: string; message: string } };
    assert.ok(!('type' in parsed), 'OpenAI envelope has no top-level `type`');
    assert.equal(parsed.error.type, 'invalid_request_error');
    assert.equal(gw.requests.length, 0, 'an over-large body never reaches the gateway');

    await sc.close();
    await gw.close();
  });

  it('413 from the OUTER catch answers in the Anthropic envelope on /v1/messages', async () => {
    const gw = await startFakeGateway((_req, res) => {
      res.writeHead(200).end('should never be reached');
    });
    const sc = await startSidecar(gw.url);

    const { status, body } = await oversizedPost(`${sc.url}/v1/messages`);
    assert.equal(status, 413);
    const parsed = JSON.parse(body) as { type: string; error: { type: string } };
    assert.equal(parsed.type, 'error');
    assert.equal(parsed.error.type, 'invalid_request_error');

    await sc.close();
    await gw.close();
  });

  it('404 on an unserved /v1/chat/ path is invalid_request_error', async () => {
    const gw = await startFakeGateway((_req, res) => {
      res.writeHead(200).end('should never be reached');
    });
    const sc = await startSidecar(gw.url);

    const resp = await authedPost(`${sc.url}/v1/chat/embeddings`, '{}');
    assert.equal(resp.status, 404);
    const parsed = (await resp.json()) as { type?: string; error: { type: string } };
    assert.ok(!('type' in parsed));
    assert.equal(parsed.error.type, 'invalid_request_error');
    assert.equal(gw.requests.length, 0);

    await sc.close();
    await gw.close();
  });

  it('503 slot-unavailable is service_unavailable, mirroring the gateway envelope', async () => {
    // The Anthropic arm says `overloaded_error`; the OpenAI arm previously fell
    // through to `server_error`, mis-signalling a retryable, nothing-signed
    // fail-closed as a permanent server fault. `service_unavailable` is the
    // type the gateway itself emits for 503 (GatewayError::ServiceUnavailable,
    // crates/gateway/src/error.rs), so both legs a client sees agree.
    const gw = await startFakeGateway((req, res) => {
      if (req.method === 'GET' && req.url.startsWith('/v1/escrow/config')) {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(escrowConfigBody(null));
        return;
      }
      res.writeHead(402, { 'content-type': 'application/json' });
      res.end(challengeBody(QUOTE_ATOMIC, `${GATEWAY_RESOURCE_ORIGIN}/v1/chat/completions`));
    });
    const sc = await startSidecar(gw.url);

    const resp = await authedPost(`${sc.url}/v1/chat/completions`, '{"model":"m","messages":[]}');
    assert.equal(resp.status, 503);
    const parsed = (await resp.json()) as { type?: string; error: { type: string } };
    assert.ok(!('type' in parsed));
    assert.equal(parsed.error.type, 'service_unavailable');

    // Nothing signed on the fail-closed slot path.
    assert.equal(chatPosts(gw).length, 1);
    assert.equal(chatPosts(gw)[0].headers['payment-signature'], undefined);

    await sc.close();
    await gw.close();
  });

  it('502 (no exact quote to draw from) is server_error', async () => {
    const gw = await startFakeGateway((req, res) => {
      if (req.method === 'GET' && req.url.startsWith('/v1/escrow/config')) {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(escrowConfigBody());
        return;
      }
      const noExact = JSON.parse(challengeBody()) as {
        accepts: Array<Record<string, unknown>>;
      };
      noExact.accepts[0].scheme = 'escrow';
      noExact.accepts[0].escrow_program_id = '9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU';
      res.writeHead(402, { 'content-type': 'application/json' });
      res.end(JSON.stringify(noExact));
    });
    const sc = await startSidecar(gw.url);

    const resp = await authedPost(`${sc.url}/v1/chat/completions`, '{"model":"m","messages":[]}');
    assert.equal(resp.status, 502);
    const parsed = (await resp.json()) as { type?: string; error: { type: string } };
    assert.ok(!('type' in parsed));
    assert.equal(parsed.error.type, 'server_error');
    assert.equal(chatPosts(gw).length, 1, 'nothing signed against a non-exact quote');

    await sc.close();
    await gw.close();
  });

  // A hard gateway rejection of the paid retry surfaces with the GATEWAY's
  // status (ChannelDrawError), so these exercise the remaining OpenAI arms.
  for (const [gatewayStatus, expectedType] of [
    [400, 'invalid_request_error'],
    [429, 'rate_limit_error'],
    [500, 'server_error'],
  ] as const) {
    it(`a ${gatewayStatus} draw rejection maps to ${expectedType}`, async () => {
      const gw = await startFakeGateway(
        standardHandler((_req, res) => {
          res.writeHead(gatewayStatus, { 'content-type': 'application/json' });
          res.end('{"error":{"message":"nope"}}');
        }),
      );
      const sc = await startSidecar(gw.url);

      const resp = await authedPost(`${sc.url}/v1/chat/completions`, '{"model":"m","messages":[]}');
      assert.equal(resp.status, gatewayStatus);
      const parsed = (await resp.json()) as { type?: string; error: { type: string } };
      assert.ok(!('type' in parsed), 'OpenAI envelope has no top-level `type`');
      assert.equal(parsed.error.type, expectedType);

      // Hard reject: one probe + exactly one signed attempt, never a retry loop.
      assert.equal(chatPosts(gw).length, 2);

      await sc.close();
      await gw.close();
    });
  }
});
