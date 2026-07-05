/**
 * v0 off-chain spend-down channel — client-side voucher signer.
 *
 * The client half of the channel whose gateway half is
 * `crates/x402/src/channel.rs` (the pure voucher core + close authorization)
 * and `crates/protocol/src/payment.rs` (`ChannelVoucherPayload` header wire).
 * "Fund once, draw down": after a one-time on-chain deposit, the agent draws
 * the channel down off-chain with a sequence of monotonic cumulative vouchers,
 * each an ed25519 signature over a byte-exact canonical message.
 *
 * This module ports the frozen byte layouts EXACTLY — the golden vectors in
 * channel.rs are the cross-language contract:
 *   - `buildVoucherMessage`  ↔ channel.rs `build_voucher_message` / GOLDEN_VECTOR_B64
 *   - `buildCloseMessage`    ↔ channel.rs `build_close_message`   / CLOSE_GOLDEN_VECTOR_B64
 *   - the header payload      ↔ payment.rs `ChannelVoucherPayload` / CHANNEL_VOUCHER_PAYLOAD_GOLDEN_JSON
 * A parity-test mismatch means THIS port is wrong, never the vector.
 *
 * Scope (plan Decision D — minimal additive): the four builders + a cumulative
 * tracker with a single-flight gate and structured resync. It does NOT speak
 * HTTP or open/close channels — the sidecar owns those and injects a `send`
 * transport into `ChannelTracker.draw`.
 *
 * Crypto primitives: `node:crypto` for ed25519 (OpenSSL, RFC 8032 — the same
 * standard the gateway's `ed25519-dalek` verifies) and SHA-256 — no new
 * dependency, consistent with `sign.ts`'s hashing. See §10 in the plan doc:
 * @solana/web3.js 1.98.4, @noble/curves 1.9.7, @noble/hashes 1.8.0 observed.
 */

import { createHash, createPrivateKey, sign as ed25519Sign } from 'node:crypto';

import bs58 from 'bs58';

import type { PaymentAccept } from './types.js';
import { X402_VERSION_CLIENT } from './schema.js';
import { SigningError } from './sign.js';

// ---------------------------------------------------------------------------
// Frozen byte-layout constants (byte-for-byte with channel.rs)
// ---------------------------------------------------------------------------

/** channel.rs `DOMAIN_SEP` = b"solvela-channel-voucher-v1" (26 bytes). */
const VOUCHER_DOMAIN_SEP = new TextEncoder().encode('solvela-channel-voucher-v1');
/** channel.rs `CLOSE_DOMAIN_SEP` = b"solvela-channel-close-v1" (24 bytes). */
const CLOSE_DOMAIN_SEP = new TextEncoder().encode('solvela-channel-close-v1');

/** `DOMAIN_SEP(26) || channel_id(32) || cumulative(8) || expiry(8) || nonce(8) || digest(32)`. */
const VOUCHER_MESSAGE_LEN = 26 + 32 + 8 + 8 + 8 + 32; // = 114
/** `CLOSE_DOMAIN_SEP(24) || channel_id(32)`. */
const CLOSE_MESSAGE_LEN = 24 + 32; // = 56

const U64_MAX = 0xffffffffffffffffn;

/**
 * Byte-exact base64 of `buildVoucherMessage` for the fixed golden input
 * (channel_id=[0x11;32], cumulative=2625, expiry=1000750, nonce=42,
 * digest=[0x22;32]). Copied verbatim from channel.rs `GOLDEN_VECTOR_B64`.
 * DO NOT hand-edit — the Rust/on-chain value is ground truth.
 */
export const CHANNEL_VOUCHER_GOLDEN_VECTOR_B64 =
  'c29sdmVsYS1jaGFubmVsLXZvdWNoZXItdjEREREREREREREREREREREREREREREREREREREREREREUEKAAAAAAAALkUPAAAAAAAqAAAAAAAAACIiIiIiIiIiIiIiIiIiIiIiIiIiIiIiIiIiIiIiIiIi';

/**
 * Byte-exact base64 of `buildCloseMessage` for channel_id=[0x11;32]. Copied
 * verbatim from channel.rs `CLOSE_GOLDEN_VECTOR_B64`. DO NOT hand-edit.
 */
export const CHANNEL_CLOSE_GOLDEN_VECTOR_B64 =
  'c29sdmVsYS1jaGFubmVsLWNsb3NlLXYxERERERERERERERERERERERERERERERERERERERERERE=';

/**
 * Byte-exact JSON of a `ChannelVoucherPayload` for the fixed input — the
 * cross-repo HEADER wire contract, copied verbatim from payment.rs
 * `CHANNEL_VOUCHER_PAYLOAD_GOLDEN_JSON`. Pins field ORDER and the fact that
 * `cumulative_atomic`/`expiry_slot`/`nonce` are JSON numbers (not strings).
 * DO NOT hand-edit.
 */
export const CHANNEL_VOUCHER_PAYLOAD_GOLDEN_JSON =
  '{"channel_id":"cid","cumulative_atomic":12600,"expiry_slot":1000750,"nonce":42,"request_digest":"ZA==","signature":"c2ln"}';

// ---------------------------------------------------------------------------
// Validation helpers (fail-closed, money-path)
// ---------------------------------------------------------------------------

function assertLen(bytes: Uint8Array, len: number, field: string): void {
  if (bytes.length !== len) {
    throw new Error(`${field} must be exactly ${len} bytes, got ${bytes.length}`);
  }
}

/**
 * Guard a u64 field BEFORE `DataView.setBigUint64` / JSON emission.
 *
 * `setBigUint64` takes its value modulo 2^64 (ToBigUint64) — a negative or
 * over-range value would silently WRAP into a different amount. On a money
 * path that is a fund-misdirection bug, so we refuse instead.
 */
function assertU64(v: bigint, field: string): bigint {
  if (typeof (v as unknown) !== 'bigint') {
    throw new TypeError(`${field} must be a bigint (atomic u64), got ${typeof v}`);
  }
  if (v < 0n || v > U64_MAX) {
    throw new RangeError(`${field} out of u64 range [0, 2^64-1]: ${v}`);
  }
  return v;
}

/**
 * Guard a `ChannelTracker` backoff/retry knob at construction. A negative or
 * non-finite value would degenerate the capped-backoff loop silently (e.g. a
 * negative `maxRetries` gives up immediately; a negative delay skips waiting),
 * so we refuse it up front. Throws `RangeError` — consistent with `assertU64`'s
 * construction-time validation of `lastCumulative`/`startNonce`.
 */
function assertNonNegative(v: number, field: string): number {
  if (typeof v !== 'number' || !Number.isFinite(v) || v < 0) {
    throw new RangeError(`${field} must be a non-negative finite number, got ${v}`);
  }
  return v;
}

// ---------------------------------------------------------------------------
// 1. buildVoucherMessage — the 114 canonical bytes
// ---------------------------------------------------------------------------

/**
 * Assemble the byte-exact canonical voucher message the agent signs and the
 * gateway re-derives:
 *
 * ```text
 * DOMAIN_SEP(26) || channel_id[32] || cumulative(u64 LE) || expiry_slot(u64 LE)
 *   || nonce(u64 LE) || request_digest[32]
 * ```
 *
 * u64 fields go through `DataView.setBigUint64(offset, value, true)` — LE,
 * full 64-bit precision, NEVER `Number` (which would corrupt atomic values
 * above 2^53). Byte-for-byte identical to channel.rs's Rust layout.
 */
export function buildVoucherMessage(
  channelId32: Uint8Array,
  cumulative: bigint,
  expirySlot: bigint,
  nonce: bigint,
  requestDigest32: Uint8Array,
): Uint8Array {
  assertLen(channelId32, 32, 'channel_id');
  assertLen(requestDigest32, 32, 'request_digest');
  assertU64(cumulative, 'cumulative');
  assertU64(expirySlot, 'expiry_slot');
  assertU64(nonce, 'nonce');

  const msg = new Uint8Array(VOUCHER_MESSAGE_LEN);
  const dv = new DataView(msg.buffer, msg.byteOffset, msg.byteLength);
  let o = 0;
  msg.set(VOUCHER_DOMAIN_SEP, o);
  o += VOUCHER_DOMAIN_SEP.length; // 26
  msg.set(channelId32, o);
  o += 32;
  dv.setBigUint64(o, cumulative, true);
  o += 8;
  dv.setBigUint64(o, expirySlot, true);
  o += 8;
  dv.setBigUint64(o, nonce, true);
  o += 8;
  msg.set(requestDigest32, o);
  return msg;
}

// ---------------------------------------------------------------------------
// 2. signVoucher — detached ed25519 over the message
// ---------------------------------------------------------------------------

/** PKCS8 DER prefix wrapping a raw 32-byte ed25519 seed (16 bytes, RFC 8410). */
const ED25519_PKCS8_SEED_PREFIX = Buffer.from('302e020100300506032b657004220420', 'hex');

/**
 * Extract the 32-byte ed25519 SEED from a secret-key input.
 *
 * The classic tweetnacl/@solana interop bug: a 64-byte "secretKey" is
 * seed(32)||pubkey(32); the ed25519 seed is the FIRST 32 bytes. Feeding all 64
 * to a seed-expecting API (or slicing the wrong half) yields a different — or
 * invalid — signature the gateway rejects. We accept both forms and always
 * derive from the seed.
 */
function ed25519Seed(secretKey: Uint8Array): Uint8Array {
  if (secretKey.length === 32) return secretKey;
  if (secretKey.length === 64) return secretKey.subarray(0, 32);
  // Typed like every other signing failure in this module (mirrors sign.ts).
  // The message carries only the observed byte LENGTH — never key bytes.
  throw new SigningError(
    `ed25519 secret key must be 32 (seed) or 64 (seed||pubkey) bytes, got ${secretKey.length}`,
  );
}

/**
 * Detached ed25519 signature (64 bytes) over `message`, using `node:crypto`
 * (OpenSSL, RFC 8032). ed25519 is deterministic, so this is the same canonical
 * signature any RFC-8032 verifier — including the gateway's `ed25519-dalek` —
 * accepts. Accepts a 32-byte seed or a 64-byte expanded key.
 *
 * This is the highest-frequency signing path (one call per draw). Mirrors
 * sign.ts's posture: any underlying `node:crypto` error is re-wrapped in a
 * message-only `SigningError` (never chaining `.stack`/`.cause`, which could let
 * a downstream logger traverse to transient key material), and the local seed
 * copies allocated here are zeroed in a `finally` so the 32-byte seed does not
 * linger on the heap for the channel's whole lifetime. Best-effort — the
 * CALLER's `secretKey` is untouched (they own it); only our own copies are wiped.
 */
export function signVoucher(message: Uint8Array, secretKey: Uint8Array): Uint8Array {
  // `seed` is the caller's array (32-byte input) or a VIEW into it (64-byte
  // input's first 32 bytes) — never zero it; only the copies below are ours.
  const seed = ed25519Seed(secretKey);
  let seedCopy: Buffer | null = null;
  let der: Buffer | null = null;
  try {
    seedCopy = Buffer.from(seed); // fresh heap copy of the 32-byte seed
    der = Buffer.concat([ED25519_PKCS8_SEED_PREFIX, seedCopy]);
    const keyObject = createPrivateKey({ key: der, format: 'der', type: 'pkcs8' });
    // ed25519 is a pure EdDSA (no pre-hash) → algorithm MUST be null.
    const sig = ed25519Sign(null, Buffer.from(message), keyObject);
    // `new Uint8Array(sig)` copies the signature out BEFORE the `finally` runs,
    // so wiping the key buffers below cannot corrupt the returned value.
    return new Uint8Array(sig);
  } catch (err) {
    if (err instanceof SigningError) throw err;
    throw new SigningError(
      `Failed to sign channel voucher: ${err instanceof Error ? err.message : String(err)}`,
    );
  } finally {
    // Best-effort wipe of the local seed copies (der embeds the seed). Runs
    // after the signature is captured. Not full erasure — V8 may have copied
    // the bytes internally — but keeps the seed from sitting in these buffers.
    if (der) der.fill(0);
    if (seedCopy) seedCopy.fill(0);
  }
}

// ---------------------------------------------------------------------------
// 3. buildCloseMessage — the 56 close-authorization bytes
// ---------------------------------------------------------------------------

/**
 * Assemble the byte-exact close-authorization message:
 * `CLOSE_DOMAIN_SEP(24) || channel_id[32]`. Distinct domain-sep from the
 * voucher message, so a voucher signature can never be replayed as a close
 * (or vice versa). Byte-for-byte with channel.rs `build_close_message`.
 */
export function buildCloseMessage(channelId32: Uint8Array): Uint8Array {
  assertLen(channelId32, 32, 'channel_id');
  const msg = new Uint8Array(CLOSE_MESSAGE_LEN);
  msg.set(CLOSE_DOMAIN_SEP, 0);
  msg.set(channelId32, CLOSE_DOMAIN_SEP.length);
  return msg;
}

// ---------------------------------------------------------------------------
// 4. buildChannelPaymentHeader — the PAYMENT-SIGNATURE header the gateway parses
// ---------------------------------------------------------------------------

interface ChannelVoucherPayloadFields {
  channelIdB58: string;
  cumulativeAtomic: bigint;
  expirySlot: bigint;
  nonce: bigint;
  requestDigestB64: string;
  signatureB64: string;
}

/**
 * Serialize the inner `ChannelVoucherPayload` byte-exactly, in the field order
 * pinned by `CHANNEL_VOUCHER_PAYLOAD_GOLDEN_JSON`.
 *
 * The u64 fields are emitted as bare JSON number tokens via `.toString()` —
 * full precision, NEVER `Number()` (which would f64-round a large atomic
 * value). `serde_json` on the gateway parses these integer tokens straight
 * into `u64`, so the wire carries full u64. The base58/base64 string fields
 * are JSON-encoded (safe escaping); their alphabets contain no JSON-special
 * characters, but `JSON.stringify` is used defensively.
 */
export function channelVoucherPayloadJson(f: ChannelVoucherPayloadFields): string {
  return (
    '{"channel_id":' +
    JSON.stringify(f.channelIdB58) +
    ',"cumulative_atomic":' +
    assertU64(f.cumulativeAtomic, 'cumulative_atomic').toString() +
    ',"expiry_slot":' +
    assertU64(f.expirySlot, 'expiry_slot').toString() +
    ',"nonce":' +
    assertU64(f.nonce, 'nonce').toString() +
    ',"request_digest":' +
    JSON.stringify(f.requestDigestB64) +
    ',"signature":' +
    JSON.stringify(f.signatureB64) +
    '}'
  );
}

export interface ChannelPaymentHeaderParams {
  /**
   * The chosen `PaymentAccept` from the 402. The channel is header-invoked —
   * the 402 advertises `exact` (never `channel`), so this header FORCES
   * `scheme: "channel"` while reusing the advertised network/amount/asset/
   * pay_to/max_timeout_seconds. `amount` is the PER-CALL quote (the gateway
   * hard-rejects a mismatch; billing uses its own quote).
   */
  accept: PaymentAccept;
  resourceUrl: string;
  channelId32: Uint8Array;
  cumulative: bigint;
  expirySlot: bigint;
  nonce: bigint;
  requestDigest32: Uint8Array;
  signature64: Uint8Array;
}

/**
 * Build the base64 `PAYMENT-SIGNATURE` header value the gateway's channel fork
 * consumes: `base64(JSON(PaymentPayload{ x402_version, resource, accepted,
 * payload: ChannelVoucherPayload }))`. Matches the shape the gateway's
 * `decode_payment_header` + channel fork require and the prod smoke used.
 */
export function buildChannelPaymentHeader(params: ChannelPaymentHeaderParams): string {
  assertLen(params.channelId32, 32, 'channel_id');
  assertLen(params.requestDigest32, 32, 'request_digest');
  assertLen(params.signature64, 64, 'signature');

  const payloadJson = channelVoucherPayloadJson({
    channelIdB58: bs58.encode(params.channelId32),
    cumulativeAtomic: params.cumulative,
    expirySlot: params.expirySlot,
    nonce: params.nonce,
    requestDigestB64: Buffer.from(params.requestDigest32).toString('base64'),
    signatureB64: Buffer.from(params.signature64).toString('base64'),
  });

  // `accepted`: scheme FORCED to "channel"; escrow_program_id omitted (the
  // gateway's Option field defaults to None when absent). Field ORDER is
  // irrelevant here — the gateway parses PaymentPayload/PaymentAccept by name.
  const accepted = {
    scheme: 'channel',
    network: params.accept.network,
    amount: params.accept.amount,
    asset: params.accept.asset,
    pay_to: params.accept.pay_to,
    max_timeout_seconds: params.accept.max_timeout_seconds,
  };

  const wrapper =
    '{"x402_version":' +
    String(X402_VERSION_CLIENT) +
    ',"resource":' +
    JSON.stringify({ url: params.resourceUrl, method: 'POST' }) +
    ',"accepted":' +
    JSON.stringify(accepted) +
    ',"payload":' +
    payloadJson +
    '}';

  return Buffer.from(wrapper, 'utf-8').toString('base64');
}

// ---------------------------------------------------------------------------
// 5. Cumulative tracker — chaining, structured resync, single-flight, retry
// ---------------------------------------------------------------------------

/**
 * Parse the STRUCTURED resync figure from a gateway rejection body.
 *
 * The authoritative last cumulative lives ONLY in `error.last_cumulative` as
 * an atomic-amount STRING (gateway `InvalidPaymentWithResync`, plan §4b;
 * fixture `channel_resync_error.json`). This NEVER scrapes the prose
 * `error.message` — pre-auth rejections deliberately carry no ledger figure,
 * and trusting prose would let a crafted message move the counter. Returns
 * `null` for a missing / non-string / non-integer / out-of-u64 value
 * (fail-closed: no resync on garbage).
 */
export function parseResyncCumulative(body: unknown): bigint | null {
  if (typeof body !== 'object' || body === null) return null;
  const error = (body as Record<string, unknown>).error;
  if (typeof error !== 'object' || error === null) return null;
  const raw = (error as Record<string, unknown>).last_cumulative;
  if (typeof raw !== 'string' || !/^\d+$/.test(raw)) return null; // string wire type, non-negative int only
  const v = BigInt(raw);
  if (v > U64_MAX) return null;
  return v;
}

/** A gateway draw rejection. `status` is the HTTP status (0 = local validation). */
export class ChannelDrawError extends Error {
  readonly status: number;
  constructor(status: number, message: string) {
    super(message);
    this.name = 'ChannelDrawError';
    this.status = status;
  }
}

/** The normalized response the caller's `send` transport returns. */
export interface DrawResponse {
  /** HTTP status of the gateway reply. */
  status: number;
  /** Parsed JSON body (or `undefined`). Used to extract a structured resync. */
  body: unknown;
}

export interface ChannelTrackerOptions {
  /** Resume from a known authoritative last cumulative (default 0). */
  lastCumulative?: bigint;
  /** First per-draw audit nonce (default 0), monotonically increasing. */
  startNonce?: bigint;
  /** Max retries per draw before giving up (default 5). */
  maxRetries?: number;
  /** Base backoff delay in ms (default 200). */
  baseDelayMs?: number;
  /** Backoff ceiling in ms (default 5000). */
  maxDelayMs?: number;
}

export interface ChannelDrawParams {
  /** The 402's chosen accept; `amount` is the per-call quote (drives the delta). */
  accept: PaymentAccept;
  resourceUrl: string;
  /** The EXACT request-body bytes that will be sent (digest preimage). */
  rawBody: Uint8Array;
  /** 32-byte seed or 64-byte expanded ed25519 key for the channel session key. */
  signSeed: Uint8Array;
  /** Voucher expiry slot (caller computes it — the tracker is RPC-free). */
  expirySlot: bigint;
  /** Transport the sidecar owns: POST the header, return `{status, body}`. */
  send: (header: string) => Promise<DrawResponse>;
}

/**
 * Per-channel draw state for the sidecar (plan §5.G). The channel is strictly
 * SERIAL — each voucher signs `last + quote`, so vouchers cannot be signed in
 * parallel — so this exposes a single-flight promise-chain gate: at most one
 * voucher is in flight per channel. `last` advances on a 2xx OR re-bases down on
 * a structured resync figure. NB: a 2xx does NOT strictly guarantee the gateway
 * durably recorded the draw — its lock-lost (`Ok(false)`) and
 * persist-failed-after-serve arms also return 2xx while the ledger did NOT
 * advance server-side, so `#last` can briefly get ahead of the gateway's true
 * state. This self-heals on the next draw (the gateway returns a
 * DeltaMismatch/402 with the authoritative `last_cumulative` and the tracker
 * re-bases down) — one wasted round-trip, never an overcharge or under-sign.
 * A 503 lock-reject retries the same cumulative; a `DeltaMismatch` resync
 * (structured `last_cumulative`) re-bases and retries; both behind capped
 * backoff, both behind the gate. Any other non-2xx is a hard, fail-closed
 * reject (never resync from prose, never silently advance).
 */
export class ChannelTracker {
  readonly channelId: Uint8Array;
  #last: bigint;
  #nonce: bigint;
  readonly #maxRetries: number;
  readonly #baseDelayMs: number;
  readonly #maxDelayMs: number;
  /** Single-flight tail — never rejects, so one failed draw can't poison the queue. */
  #tail: Promise<unknown> = Promise.resolve();

  constructor(channelId32: Uint8Array, opts: ChannelTrackerOptions = {}) {
    assertLen(channelId32, 32, 'channel_id');
    this.channelId = channelId32;
    this.#last = assertU64(opts.lastCumulative ?? 0n, 'lastCumulative');
    this.#nonce = assertU64(opts.startNonce ?? 0n, 'startNonce');
    this.#maxRetries = assertNonNegative(opts.maxRetries ?? 5, 'maxRetries');
    this.#baseDelayMs = assertNonNegative(opts.baseDelayMs ?? 200, 'baseDelayMs');
    this.#maxDelayMs = assertNonNegative(opts.maxDelayMs ?? 5000, 'maxDelayMs');
  }

  /** The authoritative last accepted cumulative (atomic micro-USDC). */
  get lastCumulative(): bigint {
    return this.#last;
  }

  /** Force a resync from an out-of-band structured figure (e.g. channel reload). */
  resyncTo(last: bigint): void {
    this.#last = assertU64(last, 'lastCumulative');
  }

  /**
   * Perform one logical draw behind the single-flight gate: sign a voucher for
   * `last + quote`, send it, and on success advance `last`. Retries 503
   * lock-rejects and structured-resync `DeltaMismatch`es with capped backoff.
   * Resolves with the gateway's success body; rejects with `ChannelDrawError`
   * on a hard reject or retry exhaustion.
   */
  draw(params: ChannelDrawParams): Promise<unknown> {
    return this.#runExclusive(() => this.#drawOnce(params));
  }

  #runExclusive<T>(task: () => Promise<T>): Promise<T> {
    const run = this.#tail.then(task, task);
    this.#tail = run.then(
      () => undefined,
      () => undefined,
    );
    return run;
  }

  async #drawOnce(params: ChannelDrawParams): Promise<unknown> {
    const quote = parseAtomicAmount(params.accept.amount);
    const digest = new Uint8Array(createHash('sha256').update(Buffer.from(params.rawBody)).digest());
    const nonce = this.#nonce++; // one nonce per logical draw; retries reuse it

    for (let attempt = 0; ; attempt += 1) {
      const cumulative = assertU64(this.#last + quote, 'cumulative');
      const message = buildVoucherMessage(this.channelId, cumulative, params.expirySlot, nonce, digest);
      const signature = signVoucher(message, params.signSeed);
      const header = buildChannelPaymentHeader({
        accept: params.accept,
        resourceUrl: params.resourceUrl,
        channelId32: this.channelId,
        cumulative,
        expirySlot: params.expirySlot,
        nonce,
        requestDigest32: digest,
        signature64: signature,
      });

      const resp = await params.send(header);

      if (resp.status >= 200 && resp.status < 300) {
        // Advance on a 2xx. NB: a 2xx does not strictly prove the gateway
        // durably recorded this draw (its lock-lost / persist-fail-after-serve
        // arms also reply 2xx); if it didn't, the next draw's DeltaMismatch
        // resync re-bases `#last` back down — one wasted round-trip, never an
        // overcharge.
        this.#last = cumulative;
        return resp.body;
      }

      // Non-2xx: 503 = lock busy / degraded (retry same cumulative); a
      // structured resync = re-base last and retry; anything else = hard stop.
      if (resp.status !== 503) {
        const resync = parseResyncCumulative(resp.body);
        if (resync === null) {
          throw new ChannelDrawError(resp.status, 'channel draw rejected by the gateway');
        }
        this.#last = resync;
      }

      if (attempt >= this.#maxRetries) {
        throw new ChannelDrawError(
          resp.status,
          `channel draw exhausted ${this.#maxRetries} retries`,
        );
      }
      await this.#backoff(attempt);
    }
  }

  async #backoff(attempt: number): Promise<void> {
    const delay = Math.min(this.#baseDelayMs * 2 ** attempt, this.#maxDelayMs);
    if (delay > 0) await new Promise((resolve) => setTimeout(resolve, delay));
  }
}

/** Parse a per-call quote (atomic-unit integer string) fail-closed. */
function parseAtomicAmount(amount: string): bigint {
  if (!/^\d+$/.test(amount)) {
    throw new ChannelDrawError(
      0,
      `quote amount must be a non-negative integer string (atomic units), got: ${amount}`,
    );
  }
  const v = BigInt(amount);
  if (v <= 0n) {
    throw new ChannelDrawError(0, `quote amount must be positive, got: ${amount}`);
  }
  if (v > U64_MAX) {
    throw new ChannelDrawError(0, 'quote amount exceeds u64 range');
  }
  return v;
}
