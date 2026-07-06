/**
 * Channel voucher signer — golden-vector parity + money-path tests.
 *
 * signer-core is the shared signer behind the MCP server / sidecar. The
 * channel module is the client half of the off-chain spend-down channel whose
 * gateway half lives in `crates/x402/src/channel.rs` (voucher core) and
 * `crates/protocol/src/payment.rs` (header wire).
 *
 * Golden-vector parity IS the contract: `buildVoucherMessage` /
 * `buildCloseMessage` MUST reproduce channel.rs's `GOLDEN_VECTOR_B64` /
 * `CLOSE_GOLDEN_VECTOR_B64` byte-for-byte for the fixed input, and the header
 * payload MUST match `CHANNEL_VOUCHER_PAYLOAD_GOLDEN_JSON`. If any diverges,
 * the on-chain / gateway disagreement is a fund-misdirection bug — fix the
 * port, NEVER edit the expected golden value (the Rust value is ground truth).
 */

import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { createPublicKey, verify as nodeVerify } from 'node:crypto';

// Independent RFC-8032 ed25519 implementation (transitive via @solana/web3.js;
// present in the lockfile at @noble/curves 1.9.7). Used ONLY in tests as a
// second, distinct verifier — if both node:crypto (OpenSSL) and @noble accept
// the signature, the gateway's ed25519-dalek (a third RFC-8032 impl) will too.
import { ed25519 } from '@noble/curves/ed25519';
import bs58 from 'bs58';

import {
  buildVoucherMessage,
  signVoucher,
  buildCloseMessage,
  buildChannelPaymentHeader,
  channelVoucherPayloadJson,
  parseResyncCumulative,
  ChannelTracker,
  ChannelDrawError,
  CHANNEL_VOUCHER_GOLDEN_VECTOR_B64,
  CHANNEL_CLOSE_GOLDEN_VECTOR_B64,
} from '../src/channel.ts';
import { CHANNEL_VOUCHER_PAYLOAD_GOLDEN_JSON } from '../src/channel.ts';
import type { PaymentAccept } from '../src/types.ts';

// ---------------------------------------------------------------------------
// Golden fixture — IDENTICAL to the Rust channel.rs golden-vector input.
//   channel_id = [0x11; 32], cumulative = 2_625, expiry = 1_000_750,
//   nonce = 42, request_digest = [0x22; 32].
// (crates/x402/src/channel.rs GOLDEN_VECTOR_B64 doc comment.)
// ---------------------------------------------------------------------------
const GOLDEN_CHANNEL_ID = new Uint8Array(32).fill(0x11);
const GOLDEN_CUMULATIVE = 2_625n;
const GOLDEN_EXPIRY = 1_000_750n;
const GOLDEN_NONCE = 42n;
const GOLDEN_DIGEST = new Uint8Array(32).fill(0x22);

// channel.rs test signing key: SigningKey::from_bytes(&[7u8; 32]).
const GOLDEN_SEED = new Uint8Array(32).fill(7);

function b64ToBytes(b64: string): Uint8Array {
  return new Uint8Array(Buffer.from(b64, 'base64'));
}

// ---------------------------------------------------------------------------
// buildVoucherMessage — byte-exact golden vector (THE central proof)
// ---------------------------------------------------------------------------

describe('buildVoucherMessage — golden-vector parity with the gateway', () => {
  it('reproduces channel.rs GOLDEN_VECTOR_B64 byte-for-byte', () => {
    const msg = buildVoucherMessage(
      GOLDEN_CHANNEL_ID,
      GOLDEN_CUMULATIVE,
      GOLDEN_EXPIRY,
      GOLDEN_NONCE,
      GOLDEN_DIGEST,
    );
    assert.equal(msg.length, 114, 'voucher message must be 114 bytes');
    assert.deepEqual(
      Buffer.from(msg),
      Buffer.from(b64ToBytes(CHANNEL_VOUCHER_GOLDEN_VECTOR_B64)),
      'voucher message bytes drifted from the pinned golden vector — fix the port, not the vector',
    );
    // Message must start with the domain-sep tag.
    assert.equal(
      Buffer.from(msg.subarray(0, 26)).toString('utf-8'),
      'solvela-channel-voucher-v1',
    );
  });

  it('encodes u64 fields little-endian at values > 2^53 (bigint path, no Number precision loss)', () => {
    // cumulative near u64::MAX region — a Number() round-trip would corrupt this.
    const cumulative = 0xFEDCBA9876543210n; // 18364758544493064720
    const expiry = 0x0011223344556677n;
    const nonce = 0xFFFFFFFFFFFFFFFFn; // u64::MAX
    const msg = buildVoucherMessage(GOLDEN_CHANNEL_ID, cumulative, expiry, nonce, GOLDEN_DIGEST);
    const dv = new DataView(msg.buffer, msg.byteOffset, msg.byteLength);
    // Fields sit after DOMAIN_SEP(26) + channel_id(32) = 58.
    assert.equal(dv.getBigUint64(58, true), cumulative, 'cumulative LE u64 round-trip');
    assert.equal(dv.getBigUint64(66, true), expiry, 'expiry LE u64 round-trip');
    assert.equal(dv.getBigUint64(74, true), nonce, 'nonce LE u64 round-trip (u64::MAX)');
  });

  it('throws on a channel_id / request_digest that is not exactly 32 bytes', () => {
    assert.throws(
      () => buildVoucherMessage(new Uint8Array(31), 1n, 1n, 1n, GOLDEN_DIGEST),
      /channel_id.*32/i,
    );
    assert.throws(
      () => buildVoucherMessage(GOLDEN_CHANNEL_ID, 1n, 1n, 1n, new Uint8Array(33)),
      /request_digest.*32/i,
    );
  });

  it('fails closed on a u64 field outside [0, 2^64-1] (never silently wraps via setBigUint64)', () => {
    assert.throws(
      () => buildVoucherMessage(GOLDEN_CHANNEL_ID, -1n, 1n, 1n, GOLDEN_DIGEST),
      /u64|range/i,
    );
    assert.throws(
      () => buildVoucherMessage(GOLDEN_CHANNEL_ID, 1n << 64n, 1n, 1n, GOLDEN_DIGEST),
      /u64|range/i,
    );
  });
});

// ---------------------------------------------------------------------------
// buildCloseMessage — byte-exact golden vector
// ---------------------------------------------------------------------------

describe('buildCloseMessage — golden-vector parity', () => {
  it('reproduces channel.rs CLOSE_GOLDEN_VECTOR_B64 byte-for-byte', () => {
    const msg = buildCloseMessage(GOLDEN_CHANNEL_ID);
    assert.equal(msg.length, 56, 'close message must be 56 bytes');
    assert.deepEqual(
      Buffer.from(msg),
      Buffer.from(b64ToBytes(CHANNEL_CLOSE_GOLDEN_VECTOR_B64)),
      'close message bytes drifted from the pinned golden vector',
    );
    assert.equal(
      Buffer.from(msg.subarray(0, 24)).toString('utf-8'),
      'solvela-channel-close-v1',
    );
  });

  it('throws on a non-32-byte channel_id', () => {
    assert.throws(() => buildCloseMessage(new Uint8Array(16)), /channel_id.*32/i);
  });
});

// ---------------------------------------------------------------------------
// signVoucher — tweetnacl 64-vs-32 seed guard + gateway-verifiable signature
// ---------------------------------------------------------------------------

describe('signVoucher — ed25519 detached signature', () => {
  const msg = buildVoucherMessage(
    GOLDEN_CHANNEL_ID,
    GOLDEN_CUMULATIVE,
    GOLDEN_EXPIRY,
    GOLDEN_NONCE,
    GOLDEN_DIGEST,
  );

  it('produces a valid signature from a 32-byte seed, accepted by TWO independent verifiers', () => {
    const sig = signVoucher(msg, GOLDEN_SEED);
    assert.equal(sig.length, 64, 'ed25519 signature is 64 bytes');

    // Independent verifier #1: @noble/curves (pure JS RFC-8032).
    const pub = ed25519.getPublicKey(GOLDEN_SEED);
    assert.equal(ed25519.verify(sig, msg, pub), true, '@noble must verify');

    // Independent verifier #2: node:crypto (OpenSSL). Derive the SPKI pubkey
    // and verify — the same operation the gateway's ed25519-dalek performs.
    const spki = Buffer.concat([
      Buffer.from('302a300506032b6570032100', 'hex'),
      Buffer.from(pub),
    ]);
    const pubKeyObj = createPublicKey({ key: spki, format: 'der', type: 'spki' });
    assert.equal(nodeVerify(null, Buffer.from(msg), pubKeyObj, Buffer.from(sig)), true);
  });

  it('handles a 64-byte expanded key (seed||pubkey) — SAME signature as the 32-byte seed', () => {
    // The classic tweetnacl interop bug: a 64-byte "secretKey" is
    // seed(32)||pubkey(32); the SEED is the first 32 bytes. Slicing wrong
    // (or feeding all 64 to a seed-expecting API) yields a different — or
    // invalid — signature the gateway would reject.
    const pub = ed25519.getPublicKey(GOLDEN_SEED);
    const expanded64 = new Uint8Array(64);
    expanded64.set(GOLDEN_SEED, 0);
    expanded64.set(pub, 32);

    const sigFrom32 = signVoucher(msg, GOLDEN_SEED);
    const sigFrom64 = signVoucher(msg, expanded64);
    assert.deepEqual(
      Buffer.from(sigFrom64),
      Buffer.from(sigFrom32),
      '64-byte expanded key must derive the same seed → identical deterministic signature',
    );
    assert.equal(ed25519.verify(sigFrom64, msg, pub), true, '64-byte-derived signature must verify');
  });

  it('is deterministic and cross-impl-identical (node:crypto == @noble for the same seed+msg)', () => {
    // ed25519 is deterministic (RFC 8032): two independent impls MUST agree
    // byte-for-byte. This proves signVoucher emits the canonical signature
    // ed25519-dalek would accept.
    const sig = signVoucher(msg, GOLDEN_SEED);
    const nobleSig = ed25519.sign(msg, GOLDEN_SEED);
    assert.deepEqual(Buffer.from(sig), Buffer.from(nobleSig), 'node:crypto and @noble must agree');
  });

  it('rejects a secret key that is neither 32 nor 64 bytes', () => {
    assert.throws(() => signVoucher(msg, new Uint8Array(48)), /32.*64|seed/i);
  });
});

// ---------------------------------------------------------------------------
// channelVoucherPayloadJson + buildChannelPaymentHeader — header wire contract
// ---------------------------------------------------------------------------

describe('channel payment header — cross-repo wire contract', () => {
  it('inner payload JSON matches CHANNEL_VOUCHER_PAYLOAD_GOLDEN_JSON (field order + number types)', () => {
    const json = channelVoucherPayloadJson({
      channelIdB58: 'cid',
      cumulativeAtomic: 12_600n,
      expirySlot: 1_000_750n,
      nonce: 42n,
      requestDigestB64: 'ZA==',
      signatureB64: 'c2ln',
    });
    assert.equal(
      json,
      CHANNEL_VOUCHER_PAYLOAD_GOLDEN_JSON,
      'ChannelVoucherPayload JSON drifted from the pinned cross-repo header contract',
    );
  });

  it('emits cumulative_atomic as a bare JSON number at full u64 precision (> 2^53)', () => {
    const big = 0xFFFFFFFFFFFFFF00n; // > Number.MAX_SAFE_INTEGER, exact-representable only as bigint
    const json = channelVoucherPayloadJson({
      channelIdB58: 'cid',
      cumulativeAtomic: big,
      expirySlot: 1n,
      nonce: 0n,
      requestDigestB64: 'ZA==',
      signatureB64: 'c2ln',
    });
    assert.ok(
      json.includes(`"cumulative_atomic":${big.toString()}`),
      'cumulative must serialize as a full-precision bare number, never an f64-rounded value',
    );
  });

  it('buildChannelPaymentHeader produces a base64 PaymentPayload the gateway fork parses', () => {
    const channelId = new Uint8Array(32).fill(0x11);
    const digest = new Uint8Array(32).fill(0x22);
    const sig = signVoucher(
      buildVoucherMessage(channelId, 12_600n, 1_000_750n, 42n, digest),
      GOLDEN_SEED,
    );
    const accept: PaymentAccept = {
      scheme: 'exact', // from the 402 — the header must FORCE it to "channel"
      network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
      amount: '2100',
      asset: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
      pay_to: '9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM',
      max_timeout_seconds: 300,
    };
    const header = buildChannelPaymentHeader({
      accept,
      resourceUrl: '/v1/chat/completions',
      channelId32: channelId,
      cumulative: 12_600n,
      expirySlot: 1_000_750n,
      nonce: 42n,
      requestDigest32: digest,
      signature64: sig,
    });

    const decoded = JSON.parse(Buffer.from(header, 'base64').toString('utf-8'));
    // Envelope shape the gateway's decode_payment_header + fork require.
    assert.equal(decoded.x402_version, 2);
    assert.deepEqual(decoded.resource, { url: '/v1/chat/completions', method: 'POST' });
    assert.equal(decoded.accepted.scheme, 'channel', 'scheme MUST be forced to channel');
    assert.equal(decoded.accepted.amount, '2100', 'accepted.amount is the per-call quote, not the cumulative');
    assert.equal(decoded.accepted.pay_to, accept.pay_to);
    assert.ok(!('escrow_program_id' in decoded.accepted), 'escrow_program_id omitted for channel');

    // Inner payload = ChannelVoucherPayload, matching the golden shape.
    const p = decoded.payload;
    assert.equal(p.channel_id, bs58.encode(channelId));
    assert.equal(p.cumulative_atomic, 12_600); // JSON number
    assert.equal(p.expiry_slot, 1_000_750);
    assert.equal(p.nonce, 42);
    assert.equal(p.request_digest, Buffer.from(digest).toString('base64'));
    assert.equal(p.signature, Buffer.from(sig).toString('base64'));
    assert.deepEqual(
      Object.keys(p),
      ['channel_id', 'cumulative_atomic', 'expiry_slot', 'nonce', 'request_digest', 'signature'],
      'payload field order must match the golden pin',
    );
  });

  it('throws on wrong-length channel_id / digest / signature', () => {
    const accept: PaymentAccept = {
      scheme: 'exact',
      network: 'solana:x',
      amount: '1',
      asset: 'm',
      pay_to: 'p',
      max_timeout_seconds: 300,
    };
    const base = {
      accept,
      resourceUrl: '/v1/chat/completions',
      cumulative: 1n,
      expirySlot: 1n,
      nonce: 0n,
    };
    assert.throws(
      () =>
        buildChannelPaymentHeader({
          ...base,
          channelId32: new Uint8Array(31),
          requestDigest32: new Uint8Array(32),
          signature64: new Uint8Array(64),
        }),
      /channel_id.*32/i,
    );
    assert.throws(
      () =>
        buildChannelPaymentHeader({
          ...base,
          channelId32: new Uint8Array(32),
          requestDigest32: new Uint8Array(32),
          signature64: new Uint8Array(63),
        }),
      /signature.*64/i,
    );
  });
});

// ---------------------------------------------------------------------------
// parseResyncCumulative — structured field ONLY, never prose
// ---------------------------------------------------------------------------

describe('parseResyncCumulative — resync from the STRUCTURED field only', () => {
  it('reads error.last_cumulative (atomic string) from the real gateway fixture', () => {
    // Byte-identical to crates/gateway/tests/fixtures/channel_resync_error.json.
    const fixture = {
      error: {
        type: 'invalid_payment',
        message: 'channel voucher rejected; resync from authoritative last_cumulative=12600',
        last_cumulative: '12600',
      },
    };
    assert.equal(parseResyncCumulative(fixture), 12_600n);
  });

  it('returns full u64 precision (> 2^53) from the string field', () => {
    const big = '18446744073709551000';
    assert.equal(parseResyncCumulative({ error: { last_cumulative: big } }), BigInt(big));
  });

  it('returns null for a prose-only body — a prose message must NOT move the counter', () => {
    // Pre-auth rejections (InvalidSignature / ChannelMismatch) deliberately
    // carry NO ledger figure. Scraping "last_cumulative=12600" from the prose
    // is exactly the anti-pattern §4b forbids.
    const proseOnly = {
      error: {
        type: 'invalid_payment',
        message: 'channel voucher rejected; resync from authoritative last_cumulative=12600',
      },
    };
    assert.equal(parseResyncCumulative(proseOnly), null);
  });

  it('returns null for missing / malformed / non-integer structured values (fail closed)', () => {
    assert.equal(parseResyncCumulative(null), null);
    assert.equal(parseResyncCumulative({}), null);
    assert.equal(parseResyncCumulative({ error: {} }), null);
    assert.equal(parseResyncCumulative({ error: { last_cumulative: 'not-a-number' } }), null);
    assert.equal(parseResyncCumulative({ error: { last_cumulative: '-5' } }), null);
    assert.equal(parseResyncCumulative({ error: { last_cumulative: '1.5' } }), null);
    assert.equal(parseResyncCumulative({ error: { last_cumulative: 12600 } }), null); // number, not the string wire type
  });
});

// ---------------------------------------------------------------------------
// ChannelTracker — chaining, single-flight, capped-backoff retry
// ---------------------------------------------------------------------------

function testAccept(amount: string): PaymentAccept {
  return {
    scheme: 'exact',
    network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
    amount,
    asset: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
    pay_to: '9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM',
    max_timeout_seconds: 300,
  };
}

// `tracker` is intentionally NOT a parameter: a ChannelDrawParams carries no
// channelId (the tracker owns it internally), so the helper needs only the
// per-call quote + transport.
function drawParams(quote: string, send: (h: string) => Promise<{ status: number; body: unknown }>) {
  return {
    accept: testAccept(quote),
    resourceUrl: '/v1/chat/completions',
    rawBody: new TextEncoder().encode('{"model":"sonnet","messages":[]}'),
    signSeed: GOLDEN_SEED,
    expirySlot: 1_000_000n,
    send,
  };
}

/** Decode a header a mock `send` received back to its inner voucher cumulative. */
function cumulativeOf(header: string): bigint {
  const decoded = JSON.parse(Buffer.from(header, 'base64').toString('utf-8'));
  return BigInt(decoded.payload.cumulative_atomic);
}

describe('ChannelTracker — cumulative chaining', () => {
  it('chains cumulative = last + quote across sequential draws; advances ONLY on 2xx', async () => {
    const tracker = new ChannelTracker(GOLDEN_CHANNEL_ID);
    const seen: bigint[] = [];
    const send = async (h: string) => {
      seen.push(cumulativeOf(h));
      return { status: 200, body: { ok: true } };
    };
    await tracker.draw(drawParams('1000', send));
    await tracker.draw(drawParams('2000', send));
    await tracker.draw(drawParams('500', send));
    assert.deepEqual(seen, [1000n, 3000n, 3500n], 'each voucher cumulative = running total');
    assert.equal(tracker.lastCumulative, 3500n);
  });

  it('does NOT advance last on a hard-rejected draw', async () => {
    const tracker = new ChannelTracker(GOLDEN_CHANNEL_ID);
    const send = async () => ({ status: 400, body: { error: { message: 'nope' } } });
    await assert.rejects(() => tracker.draw(drawParams('1000', send)), ChannelDrawError);
    assert.equal(tracker.lastCumulative, 0n, 'a rejected draw must not move the counter');
  });
});

describe('ChannelTracker — single-flight serialization', () => {
  it('serializes concurrent draws (never parallel) and keeps cumulative chained', async () => {
    const tracker = new ChannelTracker(GOLDEN_CHANNEL_ID);
    let inFlight = 0;
    let maxConcurrent = 0;
    const order: bigint[] = [];
    const send = async (h: string) => {
      inFlight += 1;
      maxConcurrent = Math.max(maxConcurrent, inFlight);
      await new Promise((r) => setTimeout(r, 20)); // hold the "in flight" window open
      order.push(cumulativeOf(h));
      inFlight -= 1;
      return { status: 200, body: { ok: true } };
    };
    // Fire two draws WITHOUT awaiting the first — they must still serialize.
    const a = tracker.draw(drawParams('1000', send));
    const b = tracker.draw(drawParams('1000', send));
    await Promise.all([a, b]);
    assert.equal(maxConcurrent, 1, 'only ONE voucher may be in flight per channel');
    assert.deepEqual(order, [1000n, 2000n], 'the second draw chains off the first');
    assert.equal(tracker.lastCumulative, 2000n);
  });

  it('a HARD-rejected first draw does not poison the queue; the second still runs off the UNCHANGED last', async () => {
    // Guards the non-obvious poison-protection in `#runExclusive`
    // (`this.#tail = run.then(() => undefined, () => undefined)` +
    // `this.#tail.then(task, task)`). If a refactor drops the rejection
    // handling — e.g. collapses to `const run = this.#tail.then(task);
    // this.#tail = run;` — the second draw chains off a rejected tail with no
    // onRejected and NEVER runs, turning `await b` / the `seen` assertion red.
    const tracker = new ChannelTracker(GOLDEN_CHANNEL_ID);
    let inFlight = 0;
    let maxConcurrent = 0;
    let call = 0;
    const seen: bigint[] = [];
    const send = async (h: string) => {
      inFlight += 1;
      maxConcurrent = Math.max(maxConcurrent, inFlight);
      await new Promise((r) => setTimeout(r, 20)); // hold the in-flight window open
      seen.push(cumulativeOf(h));
      call += 1;
      inFlight -= 1;
      // First draw HARD-rejects (400, InvalidSignature-class — NOT 503, NOT a
      // structured resync). Second draw succeeds.
      if (call === 1) return { status: 400, body: { error: { message: 'invalid signature' } } };
      return { status: 200, body: { ok: true } };
    };

    // Fire BOTH without awaiting — they must serialize behind the gate even
    // though the first one rejects.
    const a = tracker.draw(drawParams('1000', send));
    const b = tracker.draw(drawParams('2000', send));

    // (b) queue tail NOT poisoned: the first rejects, the second still resolves.
    await assert.rejects(a, ChannelDrawError, 'the first draw hard-rejects');
    await b; // must resolve — a dropped onRejected handler would hang/reject this

    // (a) single-flight held even across the rejection.
    assert.equal(maxConcurrent, 1, 'the gate must serialize even when the first draw rejects');
    // (c) the failed first draw did NOT advance last: the second signs off last=0
    // (cumulative 2000), not last=1000 (which would give 3000).
    assert.deepEqual(seen, [1000n, 2000n], 'second draw signs unchanged last (0) + its own quote');
    assert.equal(tracker.lastCumulative, 2000n, 'only the successful second draw advanced last');
  });
});

describe('ChannelTracker — capped-backoff retry', () => {
  it('retries a 503 lock-reject (same cumulative) then succeeds, behind the gate', async () => {
    const tracker = new ChannelTracker(GOLDEN_CHANNEL_ID, { baseDelayMs: 1, maxDelayMs: 4 });
    const seen: bigint[] = [];
    let calls = 0;
    const send = async (h: string) => {
      seen.push(cumulativeOf(h));
      calls += 1;
      if (calls < 3) return { status: 503, body: { error: { message: 'draw in progress' } } };
      return { status: 200, body: { ok: true } };
    };
    await tracker.draw(drawParams('1000', send));
    assert.equal(calls, 3, 'two 503s then success');
    assert.deepEqual(seen, [1000n, 1000n, 1000n], '503 retries reuse the SAME cumulative');
    assert.equal(tracker.lastCumulative, 1000n);
  });

  it('resyncs from a DeltaMismatch structured last_cumulative, then chains off it', async () => {
    const tracker = new ChannelTracker(GOLDEN_CHANNEL_ID, { baseDelayMs: 1, maxDelayMs: 4 });
    const seen: bigint[] = [];
    let calls = 0;
    const send = async (h: string) => {
      seen.push(cumulativeOf(h));
      calls += 1;
      if (calls === 1) {
        // Tracker thought last=0 → cumulative 1000; gateway's authoritative
        // last is 5000 (a prior persisted draw the tracker missed).
        return {
          status: 402,
          body: {
            error: {
              type: 'invalid_payment',
              message: 'channel voucher rejected; resync from authoritative last_cumulative=5000',
              last_cumulative: '5000',
            },
          },
        };
      }
      return { status: 200, body: { ok: true } };
    };
    await tracker.draw(drawParams('1000', send));
    assert.deepEqual(seen, [1000n, 6000n], 'after resync last=5000, retry cumulative = 5000 + quote');
    assert.equal(tracker.lastCumulative, 6000n);
  });

  it('throws after exhausting the retry cap (never loops unbounded)', async () => {
    const tracker = new ChannelTracker(GOLDEN_CHANNEL_ID, { maxRetries: 2, baseDelayMs: 1, maxDelayMs: 2 });
    let calls = 0;
    const send = async () => {
      calls += 1;
      return { status: 503, body: {} };
    };
    await assert.rejects(() => tracker.draw(drawParams('1000', send)), ChannelDrawError);
    assert.equal(calls, 3, 'initial attempt + 2 retries, then give up');
  });

  it('rejects a non-integer / non-positive quote (fail closed, never signs $0)', async () => {
    const tracker = new ChannelTracker(GOLDEN_CHANNEL_ID);
    const send = async () => ({ status: 200, body: {} });
    await assert.rejects(() => tracker.draw(drawParams('0', send)), /amount|positive/i);
    await assert.rejects(() => tracker.draw(drawParams('1.5', send)), /amount|integer/i);
  });
});

describe('ChannelTracker — construction-time option validation', () => {
  it('rejects a negative / non-finite backoff knob (guards a degenerate backoff loop)', () => {
    assert.throws(() => new ChannelTracker(GOLDEN_CHANNEL_ID, { maxRetries: -1 }), /maxRetries.*non-negative/i);
    assert.throws(() => new ChannelTracker(GOLDEN_CHANNEL_ID, { baseDelayMs: -1 }), /baseDelayMs.*non-negative/i);
    assert.throws(() => new ChannelTracker(GOLDEN_CHANNEL_ID, { maxDelayMs: NaN }), /maxDelayMs.*non-negative/i);
  });
});
