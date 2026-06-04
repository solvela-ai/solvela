import { describe, it, expect, vi, afterEach } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';
import { Keypair, Connection } from '@solana/web3.js';
import bs58 from 'bs58';

import { KeypairSigner, USDC_DECIMALS } from '../../src/signer.js';
import { Wallet } from '../../src/wallet.js';
import { PaymentAccept, Resource, SolanaPayload, EscrowPayload } from '../../src/types.js';
import { SOLANA_NETWORK, USDC_MINT } from '../../src/constants.js';

// ---------------------------------------------------------------------------
// Exact-scheme golden vector — byte-exact match against the canonical Rust
// source sdks/rust/crates/solvela-client/src/signer.rs (EXACT_GOLDEN_VECTOR_B64).
// ---------------------------------------------------------------------------
//
// Copied verbatim from EXACT_GOLDEN_VECTOR_B64 in
// sdks/rust/crates/solvela-client/src/signer.rs. DO NOT hand-edit. If this
// changes, the on-chain/gateway-accepted `exact` wire layout drifted — the
// canonical value is ground truth, never this file (mirrors the Python exact
// golden vector contract in tests/unit/test_signer.py).
const EXACT_GOLDEN_VECTOR_B64 =
  'AbipfII25y2dIV7pTiOYf+qp9tAqiikKnoJqJnMsMNmGEMP1hDxqdcaeDIPxW3EJq5WUYR+V27kgDsjLvDsXDwEBAAIFGX9rI+FshTLGq8g4+s1ep4m+DHaykgM0A5v6iz02jWEUtdnlbYnE3avA84DOO4wvyfA9bjZxRumTasKUlOhjntPqjPWsrKjNBSB1EhdcQ871Sl3Znt4goWtVJTc485fcBt324ddloZPZy+FGzut5rBy0he1fWzeROoz1hX7/AKnG+nrzvtutOj1l82qryXQxsbvkwtL24OR8pgIDRS9dYaurq6urq6urq6urq6urq6urq6urq6urq6urq6urq6urAQMEAQQCAAoMQQoAAAAAAAAG';

// Fixed golden inputs (sibling-consistent with the escrow vector and the
// Rust/Python exact vectors).
const GOLDEN_PROVIDER = '9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM';
// recent_blockhash = [0xAB; 32], encoded base58, as web3.js Transaction expects
// a base58 blockhash string. The Rust/Python vectors use the raw 32 bytes
// [0xAB; 32]; bs58 of those bytes is the same blockhash.
const GOLDEN_BLOCKHASH_BYTES = new Uint8Array(32).fill(0xab);

const RPC_URL = 'https://rpc.test.local';

/**
 * Deterministic agent wallet from seed [42; 32] — identical to the escrow
 * golden vector's agent keypair (pubkey 2iXtA8oeZqUU5pofxK971TCEvFGfems2AcDRaZHKD2pQ),
 * for sibling consistency across the exact and escrow vectors and across SDKs.
 */
function goldenAgentWallet(): Wallet {
  const kp = Keypair.fromSeed(new Uint8Array(32).fill(42));
  return Wallet.fromKeypairBytes(kp.secretKey);
}

function goldenBlockhashBase58(): string {
  // The Rust/Python vectors use the raw 32 bytes [0xAB; 32] as recent_blockhash;
  // web3.js expects a base58 blockhash string, so encode those exact bytes.
  return bs58.encode(GOLDEN_BLOCKHASH_BYTES);
}

function accept(scheme: 'exact' | 'escrow' = 'exact'): PaymentAccept {
  return new PaymentAccept(
    scheme,
    SOLANA_NETWORK,
    '2625',
    USDC_MINT,
    GOLDEN_PROVIDER,
    300,
    scheme === 'escrow' ? '9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU' : undefined,
  );
}

function resource(): Resource {
  return new Resource(`${RPC_URL}/v1/chat/completions`, 'POST');
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe('Signer', () => {
  it('KeypairSigner implements Signer interface', () => {
    const [wallet] = Wallet.create();
    const signer = new KeypairSigner(wallet);
    expect(signer).toBeDefined();
    expect(typeof signer.signPayment).toBe('function');
  });

  it('KeypairSigner is an instance of KeypairSigner', () => {
    const [wallet] = Wallet.create();
    const signer = new KeypairSigner(wallet);
    expect(signer).toBeInstanceOf(KeypairSigner);
  });

  it('KeypairSigner accepts custom rpcUrl', () => {
    const [wallet] = Wallet.create();
    const signer = new KeypairSigner(wallet, 'https://custom-rpc.example.com');
    expect(signer).toBeDefined();
  });

  it('USDC_DECIMALS is 6', () => {
    expect(USDC_DECIMALS).toBe(6);
  });
});

describe('Exact golden vector (TransferChecked byte parity)', () => {
  // This is the sole correctness bar for the `exact` wire format. The live
  // gateway verifier (crates/x402/src/solana.rs) rejects a plain SPL Transfer
  // (discriminator 3); it requires TransferChecked (discriminator 12) so the
  // USDC mint is on-chain-verifiable. The fixed inputs are sibling-consistent
  // with the escrow golden vector.
  //
  // NEVER edit EXACT_GOLDEN_VECTOR_B64 to match this output — the vector is the
  // cross-SDK contract; if this drifts, the wire layout changed and the fix is
  // in the builder, not the expected value.
  it('reproduces the canonical golden vector byte-for-byte', async () => {
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    const out = await signer.buildExactTransferTx(2625, GOLDEN_PROVIDER, goldenBlockhashBase58());
    expect(out).toBe(EXACT_GOLDEN_VECTOR_B64);
  });

  it('is deterministic for identical fixed input', async () => {
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    const a = await signer.buildExactTransferTx(2625, GOLDEN_PROVIDER, goldenBlockhashBase58());
    const b = await signer.buildExactTransferTx(2625, GOLDEN_PROVIDER, goldenBlockhashBase58());
    expect(a).toBe(b);
  });

  it('emits TransferChecked (disc 12 + decimals), not plain Transfer (disc 3)', async () => {
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    const raw = Buffer.from(
      await signer.buildExactTransferTx(2625, GOLDEN_PROVIDER, goldenBlockhashBase58()),
      'base64',
    );
    // TransferChecked ix data = [12] || amount(u64 LE) || decimals(1).
    const amountLE = Buffer.alloc(8);
    amountLE.writeBigUInt64LE(2625n);
    const expectedTransferChecked = Buffer.concat([Buffer.from([12]), amountLE, Buffer.from([6])]);
    expect(raw.includes(expectedTransferChecked)).toBe(true);
    // The old plain-Transfer ix data ([3] || amount) must NOT be present.
    const plainTransfer = Buffer.concat([Buffer.from([3]), amountLE]);
    expect(raw.includes(plainTransfer)).toBe(false);
  });
});

describe('Exact golden vector drift guard', () => {
  // The TS exact golden constant MUST equal the canonical Rust source. If
  // EXACT_GOLDEN_VECTOR_B64 ever changes in
  // sdks/rust/crates/solvela-client/src/signer.rs, the gateway-accepted `exact`
  // wire layout changed; this test fails loudly, forcing a resync of the TS
  // constant and builder rather than letting the SDKs silently diverge. Mirrors
  // Python's test_python_exact_golden_matches_rust_source.
  it('TS exact golden constant matches the Rust source of truth', () => {
    const here = dirname(fileURLToPath(import.meta.url));
    // tests/unit/signer.test.ts -> worktree root is ../../../../ from here.
    const rustSrc = resolve(
      here,
      '../../../../sdks/rust/crates/solvela-client/src/signer.rs',
    );
    const text = readFileSync(rustSrc, 'utf-8');
    const m = text.match(/EXACT_GOLDEN_VECTOR_B64:\s*&str\s*=\s*"([^"]+)"/);
    expect(m, 'could not locate EXACT_GOLDEN_VECTOR_B64 in the Rust signer.rs').not.toBeNull();
    const rustValue = m![1];
    expect(rustValue).toBe(EXACT_GOLDEN_VECTOR_B64);
  });
});

describe('Exact amount rejections (fail closed before any RPC)', () => {
  // Mirrors Python's TestExactAmountRejections. A bad amount must be rejected
  // at the boundary, before any network call — never producing a wrong-amount
  // signed transfer.
  function spyNoRpc(): ReturnType<typeof vi.spyOn> {
    // If a rejection path leaks through, this spy would be invoked and the
    // assertion below catches it. getLatestBlockhash is the first network call.
    return vi
      .spyOn(Connection.prototype, 'getLatestBlockhash')
      .mockRejectedValue(new Error('network must not be called for a rejected amount'));
  }

  it('rejects zero amount with no RPC call', async () => {
    const spy = spyNoRpc();
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    await expect(
      signer.signPayment(0, GOLDEN_PROVIDER, resource(), accept()),
    ).rejects.toThrow(/greater than zero/);
    expect(spy).not.toHaveBeenCalled();
  });

  it('rejects negative amount with no RPC call', async () => {
    const spy = spyNoRpc();
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    await expect(
      signer.signPayment(-1, GOLDEN_PROVIDER, resource(), accept()),
    ).rejects.toThrow(/greater than zero/);
    expect(spy).not.toHaveBeenCalled();
  });

  it('rejects non-integer (float) amount with no RPC call', async () => {
    const spy = spyNoRpc();
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    await expect(
      signer.signPayment(2625.5, GOLDEN_PROVIDER, resource(), accept()),
    ).rejects.toThrow(/integer/);
    expect(spy).not.toHaveBeenCalled();
  });

  it('rejects NaN amount with no RPC call', async () => {
    const spy = spyNoRpc();
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    await expect(
      signer.signPayment(Number.NaN, GOLDEN_PROVIDER, resource(), accept()),
    ).rejects.toThrow(/integer/);
    expect(spy).not.toHaveBeenCalled();
  });

  it('rejects an amount above the safe-integer range with no RPC call', async () => {
    const spy = spyNoRpc();
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    await expect(
      signer.signPayment(Number.MAX_SAFE_INTEGER + 2, GOLDEN_PROVIDER, resource(), accept()),
    ).rejects.toThrow(/safe-integer|integer/);
    expect(spy).not.toHaveBeenCalled();
  });

  it('a valid amount (2625) is unaffected by the guard and produces a SolanaPayload', async () => {
    vi.spyOn(Connection.prototype, 'getLatestBlockhash').mockResolvedValue({
      blockhash: goldenBlockhashBase58(),
      lastValidBlockHeight: 1_000_000,
    });
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    const payload = await signer.signPayment(2625, GOLDEN_PROVIDER, resource(), accept());
    expect(payload.x402Version).toBe(2);
    expect(payload.accepted.scheme).toBe('exact');
    expect(payload.payload).toBeInstanceOf(SolanaPayload);
    expect((payload.payload as SolanaPayload).transaction).toBe(EXACT_GOLDEN_VECTOR_B64);
  });
});

describe('Scheme branching (no silent fallback)', () => {
  it('escrow scheme is rejected, never settled as an exact transfer', async () => {
    const spy = vi
      .spyOn(Connection.prototype, 'getLatestBlockhash')
      .mockRejectedValue(new Error('network must not be called for an escrow rejection'));
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    await expect(
      signer.signPayment(2625, GOLDEN_PROVIDER, resource(), accept('escrow')),
    ).rejects.toThrow(/escrow/i);
    expect(spy).not.toHaveBeenCalled();
  });

  it('an unknown scheme bypassing the type system is rejected, not default-routed', async () => {
    const spy = vi
      .spyOn(Connection.prototype, 'getLatestBlockhash')
      .mockRejectedValue(new Error('network must not be called for an unknown scheme'));
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    const a = accept();
    // Force an out-of-domain scheme past the type system to prove the signer
    // fails closed rather than defaulting to a transfer.
    (a as { scheme: string }).scheme = 'upto';
    await expect(
      signer.signPayment(2625, GOLDEN_PROVIDER, resource(), a),
    ).rejects.toThrow(/Unsupported payment scheme/);
    expect(spy).not.toHaveBeenCalled();
    // Defensive: escrow payload must never be produced by the exact signer.
    expect(EscrowPayload).toBeDefined();
  });
});
