/**
 * Golden-vector + expiry-clamp tests for signer-core's escrow deposit builder.
 *
 * signer-core is the shared signer behind the MCP server, OpenClaw provider,
 * and AI SDK provider. Before this test existed it built a v0
 * VersionedTransaction with raw instruction-order accounts — which does NOT
 * match the byte-exact, writability-sorted legacy-message layout the gateway
 * verifier expects (and that the canonical Rust/Go/Python/TS SDKs pin).
 *
 * Golden-vector parity IS the contract: `buildEscrowDepositTx` MUST reproduce
 * `crates/escrow-tx/src/deposit.rs`'s GOLDEN_VECTOR_B64 byte-for-byte for the
 * fixed input. If it diverges, the on-chain layout disagreement is a
 * fund-misdirection bug — fix the builder, NEVER edit the expected golden
 * value (the Rust/on-chain value is ground truth). Mirrors
 * sdks/typescript/tests/unit/escrow.test.ts, sdks/go/escrow_test.go, and
 * sdks/python/tests/unit/test_escrow.py.
 */

import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';

import { Keypair } from '@solana/web3.js';

import {
  buildEscrowDepositTx,
  escrowExpirySlot,
  anchorDiscriminator,
  GOLDEN_VECTOR_B64,
  MIN_ESCROW_EXPIRY_SLOTS_AHEAD,
  MAX_ESCROW_EXPIRY_SLOTS_AHEAD,
  MIN_PLAUSIBLE_SLOT,
  assertPlausibleSlot,
  type EscrowDepositParams,
} from '../src/escrow.ts';
import { SigningError } from '../src/sign.ts';

// ---------------------------------------------------------------------------
// Golden fixture — identical to the Rust/Go/Python/TS golden vector input.
// Agent keypair from seed [42; 32], provider
// 9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM, USDC mint
// EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v, program
// 9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU, service_id=[7; 32],
// expiry_slot=1_000_750, recent_blockhash=[0xAB; 32], amount=2625.
// ---------------------------------------------------------------------------

const GOLDEN_PROVIDER = '9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM';
const GOLDEN_USDC_MINT = 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v';
const GOLDEN_ESCROW_PROGRAM = '9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU';

function goldenKeypair(): Keypair {
  return Keypair.fromSeed(new Uint8Array(32).fill(42));
}

function goldenParams(): EscrowDepositParams {
  return {
    keypair: goldenKeypair(),
    providerWalletB58: GOLDEN_PROVIDER,
    usdcMintB58: GOLDEN_USDC_MINT,
    escrowProgramIdB58: GOLDEN_ESCROW_PROGRAM,
    amount: 2625,
    serviceId: new Uint8Array(32).fill(7),
    expirySlot: 1_000_750,
    recentBlockhash: new Uint8Array(32).fill(0xab),
  };
}

describe('signer-core escrow deposit golden vector (byte-exact parity)', () => {
  it('reproduces the canonical Rust/Go/Python/TS golden vector byte-for-byte', () => {
    const out = buildEscrowDepositTx(goldenParams());
    // NEVER edit GOLDEN_VECTOR_B64 to match this output. The on-chain layout is
    // ground truth — if this diverges, the fix is in the builder.
    assert.equal(out, GOLDEN_VECTOR_B64);
  });

  it('is deterministic for identical fixed input (no nonce, no clock)', () => {
    assert.equal(buildEscrowDepositTx(goldenParams()), buildEscrowDepositTx(goldenParams()));
  });

  it('decodes to compact-u16(1) || sig(64) || message', () => {
    const raw = Buffer.from(GOLDEN_VECTOR_B64, 'base64');
    assert.equal(raw[0], 0x01);
    assert.ok(raw.length > 1 + 64);
  });
});

describe('signer-core escrow golden vector drift guard', () => {
  // The signer-core escrow golden constant MUST equal the Rust source of truth.
  // If the Rust GOLDEN_VECTOR_B64 ever changes, the on-chain wire layout changed
  // and this test fails loudly, forcing a resync rather than silent divergence.
  it('signer-core escrow golden constant matches the Rust source of truth', () => {
    const here = dirname(fileURLToPath(import.meta.url));
    // tests/escrow.test.ts -> worktree root is ../../../ from here.
    const rustSrc = resolve(here, '../../../crates/escrow-tx/src/deposit.rs');
    const text = readFileSync(rustSrc, 'utf-8');
    const m = text.match(/GOLDEN_VECTOR_B64:\s*&str\s*=\s*"([^"]+)"/);
    assert.ok(m, 'could not locate GOLDEN_VECTOR_B64 in crates/escrow-tx/src/deposit.rs');
    assert.equal(m![1], GOLDEN_VECTOR_B64);
  });
});

describe('signer-core escrow deposit derivation parity', () => {
  it('anchorDiscriminator is deterministic and distinct per instruction', () => {
    assert.deepEqual(anchorDiscriminator('deposit'), anchorDiscriminator('deposit'));
    assert.notDeepEqual(anchorDiscriminator('deposit'), anchorDiscriminator('claim'));
  });

  it('the deposit tx embeds the deposit discriminator', () => {
    const raw = Buffer.from(buildEscrowDepositTx(goldenParams()), 'base64');
    const disc = Buffer.from(anchorDiscriminator('deposit'));
    assert.ok(raw.includes(disc));
  });

  it('the deposit tx embeds the agent pubkey', () => {
    const params = goldenParams();
    const agentPubkey = Buffer.from(params.keypair.publicKey.toBytes());
    const raw = Buffer.from(buildEscrowDepositTx(params), 'base64');
    assert.ok(raw.includes(agentPubkey));
  });
});

describe('signer-core escrow deposit fail-closed amount guards', () => {
  it('rejects zero amount', () => {
    assert.throws(() => buildEscrowDepositTx({ ...goldenParams(), amount: 0 }), SigningError);
  });

  it('rejects a negative amount', () => {
    assert.throws(() => buildEscrowDepositTx({ ...goldenParams(), amount: -1 }), /greater than zero|integer/);
  });

  it('rejects a non-integer (float) amount', () => {
    assert.throws(() => buildEscrowDepositTx({ ...goldenParams(), amount: 2625.5 }), /integer/);
  });

  it('rejects a NaN amount', () => {
    assert.throws(() => buildEscrowDepositTx({ ...goldenParams(), amount: Number.NaN }), /integer/);
  });

  it('rejects an amount above the safe-integer range', () => {
    assert.throws(
      () => buildEscrowDepositTx({ ...goldenParams(), amount: Number.MAX_SAFE_INTEGER + 2 }),
      /safe-integer|integer/,
    );
  });
});

describe('signer-core escrow deposit fail-closed expiry-slot guards', () => {
  it('rejects a zero expiry slot', () => {
    assert.throws(() => buildEscrowDepositTx({ ...goldenParams(), expirySlot: 0 }), SigningError);
  });

  it('rejects a negative expiry slot', () => {
    assert.throws(() => buildEscrowDepositTx({ ...goldenParams(), expirySlot: -1 }), /integer/);
  });

  it('rejects a non-integer expiry slot', () => {
    assert.throws(() => buildEscrowDepositTx({ ...goldenParams(), expirySlot: 1.5 }), /integer/);
  });

  // FINDING B (audit 2026-06-08): a non-finite expiry_slot must fail closed as a
  // SigningError, not leak a raw `RangeError: The number ... cannot be converted
  // to a BigInt` out of `BigInt(Infinity)`/`BigInt(NaN)` in `u64LE`. The guard is
  // in `assertValidExpirySlot` (Number.isSafeInteger rejects both).
  it('rejects an Infinity expiry slot (fail closed, not a raw BigInt RangeError)', () => {
    assert.throws(
      () => buildEscrowDepositTx({ ...goldenParams(), expirySlot: Number.POSITIVE_INFINITY }),
      SigningError,
    );
  });

  it('rejects a NaN expiry slot', () => {
    assert.throws(
      () => buildEscrowDepositTx({ ...goldenParams(), expirySlot: Number.NaN }),
      SigningError,
    );
  });

  // FINDING C (audit 2026-06-08): expiry_slot parity with assertValidAmount —
  // values above MAX_SAFE_INTEGER don't round-trip through an IEEE-754 double and
  // cannot encode faithfully into the on-chain u64, so reject them like the amount
  // guard does (Number.isSafeInteger, not Number.isInteger).
  it('rejects an expiry slot above MAX_SAFE_INTEGER (safe-integer parity with amount)', () => {
    assert.throws(
      () => buildEscrowDepositTx({ ...goldenParams(), expirySlot: Number.MAX_SAFE_INTEGER + 2 }),
      /safe-integer|integer/,
    );
  });
});

// ---------------------------------------------------------------------------
// Expiry clamp parity with the four canonical SDKs (FINDING 2).
// [MIN_ESCROW_EXPIRY_SLOTS_AHEAD=150, MAX_ESCROW_EXPIRY_SLOTS_AHEAD=10000]
// floor/cap, MIN_PLAUSIBLE_SLOT=1_000_000 fail-closed plausibility check.
// Ported verbatim from sdks/typescript/src/signer.ts (and rust/go/python).
// ---------------------------------------------------------------------------

describe('signer-core escrowExpirySlot clamp (parity with canonical SDKs)', () => {
  it('exposes the canonical constants', () => {
    assert.equal(MIN_ESCROW_EXPIRY_SLOTS_AHEAD, 150);
    assert.equal(MAX_ESCROW_EXPIRY_SLOTS_AHEAD, 10_000);
    assert.equal(MIN_PLAUSIBLE_SLOT, 1_000_000);
  });

  it('clamps a below-floor timeout up to the 150-slot floor', () => {
    // maxTimeoutSeconds = 10 -> 10*1000/400 = 25 slots -> floored to 150.
    const slot = escrowExpirySlot(5_000_000, 10);
    assert.equal(slot, 5_000_000 + MIN_ESCROW_EXPIRY_SLOTS_AHEAD);
  });

  it('floors a tiny timeout (< floor) — the dead-on-arrival case the old code shipped', () => {
    // maxTimeoutSeconds = 1 -> 2 slots in the old signer-core code (Math.max(.., 10)
    // would even give 10) — both below the gateway's 50-slot reject threshold.
    // The canonical clamp lifts it to 150.
    assert.equal(escrowExpirySlot(5_000_000, 1), 5_000_000 + MIN_ESCROW_EXPIRY_SLOTS_AHEAD);
  });

  it('caps an above-ceiling timeout down to the 10000-slot ceiling', () => {
    // maxTimeoutSeconds = 100000 -> 250000 slots -> capped to 10000.
    const slot = escrowExpirySlot(5_000_000, 100_000);
    assert.equal(slot, 5_000_000 + MAX_ESCROW_EXPIRY_SLOTS_AHEAD);
  });

  it('uses the raw offset when it falls inside the band', () => {
    // maxTimeoutSeconds = 1000 -> 2500 slots, within [150, 10000].
    const slot = escrowExpirySlot(5_000_000, 1000);
    assert.equal(slot, 5_000_000 + 2500);
  });

  it('floors a negative timeout to 0 then clamps to the 150-slot floor', () => {
    assert.equal(escrowExpirySlot(5_000_000, -42), 5_000_000 + MIN_ESCROW_EXPIRY_SLOTS_AHEAD);
  });

  // FINDING B (audit 2026-06-08): escrowExpirySlot is a public export. Before the
  // guard it silently returned NaN for a NaN timeout (Math.max(NaN, 0) = NaN) and
  // silently clamped Infinity to the 10000-slot ceiling — both hiding a real input
  // error on a value path. Fail closed on a non-finite timeout instead. The
  // canonical signers never feed it a non-integer (Rust u64 / Go int reject at
  // deserialization, the canonical TS signer rejects with !Number.isInteger), so
  // this guard only ever fires on a genuinely malformed input.
  it('throws on a NaN timeout (was: silently returned NaN)', () => {
    assert.throws(() => escrowExpirySlot(5_000_000, Number.NaN), SigningError);
  });

  it('throws on an Infinity timeout (was: silently clamped to the ceiling)', () => {
    assert.throws(() => escrowExpirySlot(5_000_000, Number.POSITIVE_INFINITY), SigningError);
  });

  it('throws on a non-finite currentSlot', () => {
    assert.throws(() => escrowExpirySlot(Number.NaN, 300), SigningError);
    assert.throws(() => escrowExpirySlot(Number.POSITIVE_INFINITY, 300), SigningError);
  });
});

describe('signer-core escrow slot plausibility (fail-closed)', () => {
  it('accepts a plausible mainnet slot', () => {
    assert.doesNotThrow(() => assertPlausibleSlot(5_000_000));
  });

  it('rejects a slot below MIN_PLAUSIBLE_SLOT (stub/genesis node)', () => {
    assert.throws(() => assertPlausibleSlot(42), /implausibly low slot|MIN_PLAUSIBLE_SLOT|refusing/);
  });

  it('rejects a slot exactly one below the floor', () => {
    assert.throws(() => assertPlausibleSlot(MIN_PLAUSIBLE_SLOT - 1), SigningError);
  });

  it('accepts a slot exactly at the floor', () => {
    assert.doesNotThrow(() => assertPlausibleSlot(MIN_PLAUSIBLE_SLOT));
  });

  it('rejects a NaN / non-finite slot', () => {
    assert.throws(() => assertPlausibleSlot(Number.NaN), SigningError);
  });
});
