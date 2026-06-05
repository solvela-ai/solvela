import { describe, it, expect } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';
import { Keypair } from '@solana/web3.js';

import {
  buildDepositTx,
  GOLDEN_VECTOR_B64,
  anchorDiscriminator,
  type DepositParams,
} from '../../src/escrow.js';
import { Wallet } from '../../src/wallet.js';
import { SignerError } from '../../src/errors.js';

// ---------------------------------------------------------------------------
// Golden-vector parity is the contract: the TS deposit builder MUST reproduce
// crates/escrow-tx/src/deposit.rs's GOLDEN_VECTOR_B64 byte-for-byte for its
// fixed input. If it diverges, the on-chain layout disagreement is a
// fund-misdirection bug — fix the TS builder, NEVER edit the expected golden
// value (the Rust/on-chain value is ground truth). Mirrors Go escrow_test.go +
// Python tests/unit/test_escrow.py.
// ---------------------------------------------------------------------------

const GOLDEN_PROVIDER = '9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM';
const GOLDEN_USDC_MINT = 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v';
const GOLDEN_ESCROW_PROGRAM = '9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU';

/**
 * Deterministic agent wallet from seed [42; 32] — identical to the Rust/Go/
 * Python golden fixture (`SigningKey::from_bytes([42; 32])`).
 */
function goldenWallet(): Wallet {
  const kp = Keypair.fromSeed(new Uint8Array(32).fill(42));
  return Wallet.fromKeypairBytes(kp.secretKey);
}

function goldenParams(): DepositParams {
  return {
    wallet: goldenWallet(),
    providerWalletB58: GOLDEN_PROVIDER,
    usdcMintB58: GOLDEN_USDC_MINT,
    escrowProgramIdB58: GOLDEN_ESCROW_PROGRAM,
    amount: 2625,
    serviceId: new Uint8Array(32).fill(7),
    expirySlot: 1_000_750,
    recentBlockhash: new Uint8Array(32).fill(0xab),
  };
}

describe('Escrow deposit golden vector (byte-exact parity)', () => {
  it('reproduces the canonical Rust/Go golden vector byte-for-byte', () => {
    const out = buildDepositTx(goldenParams());
    // NEVER edit GOLDEN_VECTOR_B64 to match this output. The on-chain layout is
    // ground truth — if this diverges, the fix is in the builder.
    expect(out).toBe(GOLDEN_VECTOR_B64);
  });

  it('is deterministic for identical fixed input (no nonce, no clock)', () => {
    const a = buildDepositTx(goldenParams());
    const b = buildDepositTx(goldenParams());
    expect(a).toBe(b);
  });

  it('golden vector decodes to 0x01 || sig(64) || message', () => {
    const raw = Buffer.from(GOLDEN_VECTOR_B64, 'base64');
    // compact-u16(1) || sig(64) || message
    expect(raw[0]).toBe(0x01);
    expect(raw.length).toBeGreaterThan(1 + 64);
  });
});

describe('Escrow golden vector drift guard', () => {
  // The TS escrow golden constant MUST equal the Rust source of truth. If the
  // Rust GOLDEN_VECTOR_B64 ever changes, the on-chain wire layout changed and
  // this test fails loudly, forcing a resync rather than silent divergence.
  // Mirrors Go's TestGoldenVectorMatchesRustSource.
  it('TS escrow golden constant matches the Rust source of truth', () => {
    const here = dirname(fileURLToPath(import.meta.url));
    // tests/unit/escrow.test.ts -> worktree root is ../../../../ from here.
    const rustSrc = resolve(here, '../../../../crates/escrow-tx/src/deposit.rs');
    const text = readFileSync(rustSrc, 'utf-8');
    const m = text.match(/GOLDEN_VECTOR_B64:\s*&str\s*=\s*"([^"]+)"/);
    expect(m, 'could not locate GOLDEN_VECTOR_B64 in crates/escrow-tx/src/deposit.rs').not.toBeNull();
    const rustValue = m![1];
    expect(rustValue).toBe(GOLDEN_VECTOR_B64);
  });
});

describe('Escrow deposit derivation parity', () => {
  it('anchorDiscriminator is deterministic and distinct per instruction', () => {
    expect(anchorDiscriminator('deposit')).toEqual(anchorDiscriminator('deposit'));
    expect(anchorDiscriminator('deposit')).not.toEqual(anchorDiscriminator('claim'));
  });

  it('the deposit tx embeds the deposit discriminator', () => {
    const raw = Buffer.from(buildDepositTx(goldenParams()), 'base64');
    const disc = Buffer.from(anchorDiscriminator('deposit'));
    expect(raw.includes(disc)).toBe(true);
  });

  it('the deposit tx embeds the agent pubkey', () => {
    const params = goldenParams();
    const agentPubkey = Buffer.from(params.wallet.publicKey().toBytes());
    const raw = Buffer.from(buildDepositTx(params), 'base64');
    expect(raw.includes(agentPubkey)).toBe(true);
  });
});

describe('Escrow deposit fail-closed amount guards', () => {
  it('rejects zero amount', () => {
    expect(() => buildDepositTx({ ...goldenParams(), amount: 0 })).toThrow(SignerError);
  });

  it('rejects a negative amount', () => {
    expect(() => buildDepositTx({ ...goldenParams(), amount: -1 })).toThrow(/greater than zero|integer/);
  });

  it('rejects a non-integer (float) amount', () => {
    expect(() => buildDepositTx({ ...goldenParams(), amount: 2625.5 })).toThrow(/integer/);
  });

  it('rejects a NaN amount', () => {
    expect(() => buildDepositTx({ ...goldenParams(), amount: Number.NaN })).toThrow(/integer/);
  });

  it('rejects an amount above the safe-integer range', () => {
    expect(() =>
      buildDepositTx({ ...goldenParams(), amount: Number.MAX_SAFE_INTEGER + 2 }),
    ).toThrow(/safe-integer|integer/);
  });
});

describe('Escrow deposit fail-closed expiry-slot guards', () => {
  // expiry_slot is encoded directly into the on-chain u64. A 0/negative value is
  // a dead-on-arrival deposit; NaN/Infinity make u64LE's BigInt(...) throw a raw
  // untyped error. All must fail closed with a typed SignerError, parallel to
  // the amount guards. The production path always passes a valid positive slot,
  // so the golden vector is unaffected.
  it('rejects a zero expiry slot', () => {
    expect(() => buildDepositTx({ ...goldenParams(), expirySlot: 0 })).toThrow(SignerError);
  });

  it('rejects a negative expiry slot', () => {
    expect(() => buildDepositTx({ ...goldenParams(), expirySlot: -1 })).toThrow(
      /expiry_slot must be a positive integer/,
    );
  });

  it('rejects a non-integer (float) expiry slot', () => {
    expect(() => buildDepositTx({ ...goldenParams(), expirySlot: 1_000_750.5 })).toThrow(
      /expiry_slot must be a positive integer/,
    );
  });

  it('rejects a NaN expiry slot', () => {
    expect(() => buildDepositTx({ ...goldenParams(), expirySlot: Number.NaN })).toThrow(
      /expiry_slot must be a positive integer/,
    );
  });

  it('rejects an Infinity expiry slot', () => {
    expect(() =>
      buildDepositTx({ ...goldenParams(), expirySlot: Number.POSITIVE_INFINITY }),
    ).toThrow(/expiry_slot must be a positive integer/);
  });
});

describe('Escrow deposit fail-closed structural guards', () => {
  it('rejects a malformed provider address', () => {
    expect(() =>
      buildDepositTx({ ...goldenParams(), providerWalletB58: 'not-base58-0OIl!!!' }),
    ).toThrow(SignerError);
  });

  it('rejects a malformed mint address', () => {
    expect(() =>
      buildDepositTx({ ...goldenParams(), usdcMintB58: 'not-base58-0OIl!!!' }),
    ).toThrow(SignerError);
  });

  it('rejects a malformed escrow program id', () => {
    expect(() =>
      buildDepositTx({ ...goldenParams(), escrowProgramIdB58: 'not-base58-0OIl!!!' }),
    ).toThrow(SignerError);
  });

  it('rejects a service_id of the wrong length', () => {
    expect(() =>
      buildDepositTx({ ...goldenParams(), serviceId: new Uint8Array(31).fill(7) }),
    ).toThrow(/service_id/);
  });

  it('rejects a recent_blockhash of the wrong length', () => {
    expect(() =>
      buildDepositTx({ ...goldenParams(), recentBlockhash: new Uint8Array(31).fill(0xab) }),
    ).toThrow(/blockhash/);
  });
});

describe('Escrow deposit corrupted-pubkey-half guard', () => {
  // The canonical Rust/Go builders re-derive the pubkey from the seed and reject
  // a keypair whose stored public half (bytes 32..64) does not match — a corrupt
  // half would sign fine but seed the escrow PDA from the wrong identity,
  // producing an un-claimable deposit. In the TS SDK this invariant lives at the
  // layer that actually enforces it: `Wallet.fromKeypairBytes` -> web3.js
  // `Keypair.fromSecretKey` validates the public half at construction and
  // rejects a corrupted keypair before it can ever reach the builder. Because a
  // `DepositParams.wallet` is, by type, an already-constructed `Wallet`, the
  // builder cannot receive a public-half-inconsistent keypair and therefore no
  // longer re-checks it (unlike Go, whose `DepositParams` takes raw
  // `ed25519.PrivateKey` bytes with no construction-time validation, and unlike
  // a builder-level guard that would have to pull raw secret-key material onto
  // the per-call money path for zero marginal safety). We pin the protection at
  // the Wallet construction boundary.
  it('rejects a keypair whose stored pubkey half does not match its seed (at the Wallet boundary)', () => {
    const kp = Keypair.fromSeed(new Uint8Array(32).fill(42));
    const corrupted = Uint8Array.from(kp.secretKey);
    // Flip a bit in the stored public half (bytes 32..64) WITHOUT touching the
    // seed (bytes 0..32).
    corrupted[40] ^= 0xff;
    expect(() => Wallet.fromKeypairBytes(corrupted)).toThrow();
  });
});

describe('Escrow deposit secret-redaction', () => {
  // The agent secret key material must never appear in any error message. We
  // force a builder error (malformed provider address) using the golden wallet
  // whose seed bytes are all 42 (0x2a) and assert the raw seed bytes never leak
  // into the thrown error.
  it('never leaks raw keypair bytes in a builder error message', () => {
    try {
      buildDepositTx({ ...goldenParams(), providerWalletB58: 'not-base58-0OIl!!!' });
      throw new Error('expected build to reject the malformed provider address');
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      // 42 42 42 (decimal) / 2a,2a,2a (hex) are the tell-tale shapes of a dumped
      // 32-byte seed. Neither may appear.
      expect(msg).not.toMatch(/42, ?42, ?42/);
      expect(msg).not.toMatch(/2a,? ?2a,? ?2a/i);
    }
  });
});
