import { describe, it, expect, vi, afterEach } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, resolve } from 'node:path';
import { Keypair, Connection } from '@solana/web3.js';
import bs58 from 'bs58';

import { KeypairSigner, USDC_DECIMALS, escrowExpirySlot } from '../../src/signer.js';
import { SignerError } from '../../src/errors.js';
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
  vi.unstubAllGlobals();
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

describe('buildExactTransferTx direct-call guard (fail closed before any RPC)', () => {
  // buildExactTransferTx is public + barrel-exported, so external consumers can
  // call it directly, bypassing signExactPayment. The amount guard MUST hold on
  // this path too: a zero/negative/float amount would otherwise be silently
  // mis-encoded into the on-chain u64 (a negative amount wraps to ~18.4
  // quintillion atomic USDC). These pin the money-path Critical fix.
  function spyNoRpc(): ReturnType<typeof vi.spyOn> {
    return vi
      .spyOn(Connection.prototype, 'getLatestBlockhash')
      .mockRejectedValue(new Error('network must not be called for a rejected amount'));
  }

  it('rejects zero amount on a direct call with no RPC', async () => {
    const spy = spyNoRpc();
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    await expect(
      signer.buildExactTransferTx(0, GOLDEN_PROVIDER, goldenBlockhashBase58()),
    ).rejects.toThrow(SignerError);
    expect(spy).not.toHaveBeenCalled();
  });

  it('rejects a negative amount on a direct call with no RPC', async () => {
    const spy = spyNoRpc();
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    await expect(
      signer.buildExactTransferTx(-1, GOLDEN_PROVIDER, goldenBlockhashBase58()),
    ).rejects.toThrow(SignerError);
    expect(spy).not.toHaveBeenCalled();
  });

  it('rejects a float amount on a direct call with no RPC', async () => {
    const spy = spyNoRpc();
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    await expect(
      signer.buildExactTransferTx(2625.5, GOLDEN_PROVIDER, goldenBlockhashBase58()),
    ).rejects.toThrow(SignerError);
    expect(spy).not.toHaveBeenCalled();
  });
});

describe('signPayment error wrapping (non-Error rejections)', () => {
  // Pins SHOULD-FIX #1: a plain non-Error rejection from the RPC layer must not
  // surface as "Failed to sign payment: undefined".
  it('surfaces a SignerError whose message is NOT "...undefined" when RPC throws a string', async () => {
    vi.spyOn(Connection.prototype, 'getLatestBlockhash').mockRejectedValue('boom-string-rejection');
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    let caught: unknown;
    try {
      await signer.signPayment(2625, GOLDEN_PROVIDER, resource(), accept());
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(SignerError);
    const msg = (caught as Error).message;
    expect(msg).not.toMatch(/undefined/);
    expect(msg).toContain('boom-string-rejection');
  });
});

describe('signPayment invalid recipient', () => {
  // An un-decodable base58 recipient must surface a SignerError, not a raw
  // throw from PublicKey construction.
  it('surfaces a SignerError for an invalid recipient base58 string', async () => {
    vi.spyOn(Connection.prototype, 'getLatestBlockhash').mockResolvedValue({
      blockhash: goldenBlockhashBase58(),
      lastValidBlockHeight: 1_000_000,
    });
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    await expect(
      signer.signPayment(2625, 'not-a-valid-base58-pubkey!!!', resource(), accept()),
    ).rejects.toThrow(SignerError);
  });
});

describe('Scheme branching (no silent fallback)', () => {
  it('an exact-selected payment never produces an escrow deposit', async () => {
    vi.spyOn(Connection.prototype, 'getLatestBlockhash').mockResolvedValue({
      blockhash: goldenBlockhashBase58(),
      lastValidBlockHeight: 1_000_000,
    });
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    const payload = await signer.signPayment(2625, GOLDEN_PROVIDER, resource(), accept('exact'));
    expect(payload.payload).toBeInstanceOf(SolanaPayload);
    expect(payload.payload).not.toBeInstanceOf(EscrowPayload);
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

// ---------------------------------------------------------------------------
// Escrow scheme signing. The escrow path uses raw JSON-RPC (global `fetch`) for
// getSlot + getLatestBlockhash (mirroring the Go/Python signers), so these
// tests mock `fetch` rather than web3.js `Connection`. Mirrors sdks/go's
// signer_test.go escrow coverage.
// ---------------------------------------------------------------------------

const ESCROW_PROGRAM = '9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU';

function escrowAccept(...programId: [] | [string | undefined]): PaymentAccept {
  // Use a rest param so the caller distinguishes "default program id" (no arg)
  // from "explicitly undefined / empty" — a defaulted positional param would
  // turn an explicit `undefined` back into the default and mask the
  // missing-program-id rejection path.
  const pid = programId.length === 0 ? ESCROW_PROGRAM : programId[0];
  return new PaymentAccept('escrow', SOLANA_NETWORK, '2625', USDC_MINT, GOLDEN_PROVIDER, 300, pid);
}

/** A JSON-RPC response body for a method, mirroring the real RPC shape. */
function rpcOk(method: string): unknown {
  if (method === 'getSlot') {
    return { jsonrpc: '2.0', id: 1, result: 1_000_000 };
  }
  // getLatestBlockhash — blockhash is a base58 32-byte string. The system
  // program id (32 zero bytes) base58-encodes to a valid 32-byte hash.
  return {
    jsonrpc: '2.0',
    id: 1,
    result: {
      context: { slot: 1_000_000 },
      value: { blockhash: '11111111111111111111111111111111', lastValidBlockHeight: 1_000_000 },
    },
  };
}

/**
 * Install a `fetch` mock routing by JSON-RPC method. `overrides` lets a test
 * substitute a custom { status, body } per method (body may be a non-JSON
 * string to exercise the malformed-JSON path). Returns the vi mock for
 * call-count assertions.
 */
function mockRpc(
  overrides: Partial<Record<string, { status?: number; body?: unknown; raw?: string }>> = {},
): ReturnType<typeof vi.fn> {
  const fn = vi.fn(async (_url: string, init?: { body?: string }) => {
    const parsed = JSON.parse(init?.body ?? '{}') as { method?: string };
    const method = parsed.method ?? '';
    const o = overrides[method];
    if (o?.status !== undefined && o.status !== 200) {
      return new Response('<html>error</html>', { status: o.status });
    }
    if (o?.raw !== undefined) {
      return new Response(o.raw, { status: 200, headers: { 'Content-Type': 'application/json' } });
    }
    const body = o?.body ?? rpcOk(method);
    return new Response(JSON.stringify(body), {
      status: 200,
      headers: { 'Content-Type': 'application/json' },
    });
  });
  vi.stubGlobal('fetch', fn);
  return fn as unknown as ReturnType<typeof vi.fn>;
}

describe('Escrow scheme signing (no silent fallback)', () => {
  it('escrow scheme produces an EscrowPayload with a valid deposit_tx + base64 service_id', async () => {
    mockRpc();
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    const payload = await signer.signPayment(2625, GOLDEN_PROVIDER, resource(), escrowAccept());
    expect(payload.x402Version).toBe(2);
    expect(payload.accepted.scheme).toBe('escrow');
    expect(payload.payload).toBeInstanceOf(EscrowPayload);
    const ep = payload.payload as EscrowPayload;
    // deposit_tx is a base64 signed legacy tx: compact-u16(1) || sig(64) || msg.
    const raw = Buffer.from(ep.depositTx, 'base64');
    expect(raw[0]).toBe(0x01);
    expect(raw.length).toBeGreaterThan(1 + 64);
    // service_id is base64 of exactly 32 bytes.
    expect(Buffer.from(ep.serviceId, 'base64').length).toBe(32);
    expect(ep.agentPubkey).toBe(goldenAgentWallet().address());
  });

  it('generates a unique service_id per call (#118 CSPRNG invariant)', async () => {
    mockRpc();
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    const p1 = await signer.signPayment(2625, GOLDEN_PROVIDER, resource(), escrowAccept());
    const p2 = await signer.signPayment(2625, GOLDEN_PROVIDER, resource(), escrowAccept());
    const s1 = (p1.payload as EscrowPayload).serviceId;
    const s2 = (p2.payload as EscrowPayload).serviceId;
    expect(s1).not.toBe(s2);
  });

  it('rejects a missing escrow_program_id before any RPC', async () => {
    const fn = mockRpc();
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    await expect(
      signer.signPayment(2625, GOLDEN_PROVIDER, resource(), escrowAccept(undefined)),
    ).rejects.toThrow(/escrow_program_id is missing/);
    expect(fn).not.toHaveBeenCalled();
  });

  it('rejects an empty escrow_program_id before any RPC', async () => {
    const fn = mockRpc();
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    await expect(
      signer.signPayment(2625, GOLDEN_PROVIDER, resource(), escrowAccept('')),
    ).rejects.toThrow(/escrow_program_id is missing/);
    expect(fn).not.toHaveBeenCalled();
  });

  it('rejects a zero amount before any RPC', async () => {
    const fn = mockRpc();
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    await expect(
      signer.signPayment(0, GOLDEN_PROVIDER, resource(), escrowAccept()),
    ).rejects.toThrow(/greater than zero/);
    expect(fn).not.toHaveBeenCalled();
  });

  it('rejects a float amount before any RPC', async () => {
    const fn = mockRpc();
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    await expect(
      signer.signPayment(2625.5, GOLDEN_PROVIDER, resource(), escrowAccept()),
    ).rejects.toThrow(/integer/);
    expect(fn).not.toHaveBeenCalled();
  });

  it('rejects an implausibly low slot (stub/genesis node)', async () => {
    mockRpc({ getSlot: { body: { jsonrpc: '2.0', id: 1, result: 0 } } });
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    await expect(
      signer.signPayment(2625, GOLDEN_PROVIDER, resource(), escrowAccept()),
    ).rejects.toThrow(/implausibly low slot/);
  });

  it('rejects a getSlot HTTP 429', async () => {
    mockRpc({ getSlot: { status: 429 } });
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    await expect(
      signer.signPayment(2625, GOLDEN_PROVIDER, resource(), escrowAccept()),
    ).rejects.toThrow(/getSlot RPC HTTP 429/);
  });

  it('rejects a malformed getSlot JSON body', async () => {
    mockRpc({ getSlot: { raw: 'not valid json' } });
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    await expect(
      signer.signPayment(2625, GOLDEN_PROVIDER, resource(), escrowAccept()),
    ).rejects.toThrow(/getSlot RPC: malformed JSON/);
  });

  it('surfaces a getSlot JSON-RPC error code without leaking the message text', async () => {
    mockRpc({
      getSlot: {
        body: { jsonrpc: '2.0', id: 1, error: { code: -32000, message: 'node behind' } },
      },
    });
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    let caught: unknown;
    try {
      await signer.signPayment(2625, GOLDEN_PROVIDER, resource(), escrowAccept());
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(SignerError);
    const msg = (caught as Error).message;
    expect(msg).toMatch(/getSlot RPC error/);
    expect(msg).toContain('-32000');
    expect(msg).not.toContain('node behind');
  });

  it('does not render a code-less getSlot error as "code: 0"', async () => {
    mockRpc({
      getSlot: { body: { jsonrpc: '2.0', id: 1, error: { message: 'node behind' } } },
    });
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    let caught: unknown;
    try {
      await signer.signPayment(2625, GOLDEN_PROVIDER, resource(), escrowAccept());
    } catch (e) {
      caught = e;
    }
    const msg = (caught as Error).message;
    expect(msg).not.toContain('code: 0');
    expect(msg).toMatch(/no numeric code/);
    expect(msg).not.toContain('node behind');
  });

  it('rejects a missing blockhash', async () => {
    mockRpc({
      getLatestBlockhash: {
        body: { jsonrpc: '2.0', id: 1, error: { code: -32000, message: 'internal' } },
      },
    });
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    await expect(
      signer.signPayment(2625, GOLDEN_PROVIDER, resource(), escrowAccept()),
    ).rejects.toThrow(/did not return a blockhash/);
  });

  it('rejects a base58-valid but wrong-length blockhash', async () => {
    mockRpc({
      getLatestBlockhash: {
        body: {
          jsonrpc: '2.0',
          id: 1,
          result: { context: { slot: 1_000_000 }, value: { blockhash: '1111' } },
        },
      },
    });
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    await expect(
      signer.signPayment(2625, GOLDEN_PROVIDER, resource(), escrowAccept()),
    ).rejects.toThrow(/malformed blockhash/);
  });

  it('produces a payload that is NOT a SolanaPayload (escrow scheme contract)', async () => {
    // Converse of the exact-path scheme-contract assertion: an escrow-selected
    // payment must never produce an exact-scheme `SolanaPayload`.
    mockRpc();
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    const payload = await signer.signPayment(2625, GOLDEN_PROVIDER, resource(), escrowAccept());
    expect(payload.payload).toBeInstanceOf(EscrowPayload);
    expect(payload.payload).not.toBeInstanceOf(SolanaPayload);
  });

  it('rejects a NaN maxTimeoutSeconds before any RPC, naming the field', async () => {
    // maxTimeoutSeconds comes straight from the untrusted gateway 402. A NaN
    // value would flow through escrowExpirySlot (Math.max(NaN,0)=NaN) into the
    // builder and surface as a confusing wrapped error. Reject at the boundary,
    // before any RPC mock is hit.
    const fn = mockRpc();
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    const a = escrowAccept();
    (a as { maxTimeoutSeconds: number }).maxTimeoutSeconds = Number.NaN;
    await expect(
      signer.signPayment(2625, GOLDEN_PROVIDER, resource(), a),
    ).rejects.toThrow(/maxTimeoutSeconds/);
    expect(fn).not.toHaveBeenCalled();
  });

  it('rejects a non-integer (float) maxTimeoutSeconds before any RPC', async () => {
    const fn = mockRpc();
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    const a = escrowAccept();
    (a as { maxTimeoutSeconds: number }).maxTimeoutSeconds = 300.5;
    await expect(
      signer.signPayment(2625, GOLDEN_PROVIDER, resource(), a),
    ).rejects.toThrow(/maxTimeoutSeconds/);
    expect(fn).not.toHaveBeenCalled();
  });

  it('accepts a negative finite maxTimeoutSeconds and clamps (parity with Go)', async () => {
    // A negative finite integer is NOT a hard error: escrowExpirySlot floors it
    // to 0 then clamps to the 150-slot floor (mirrors Go's escrowExpirySlot). It
    // must still produce a valid EscrowPayload.
    mockRpc();
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    const a = escrowAccept();
    (a as { maxTimeoutSeconds: number }).maxTimeoutSeconds = -5;
    const payload = await signer.signPayment(2625, GOLDEN_PROVIDER, resource(), a);
    expect(payload.payload).toBeInstanceOf(EscrowPayload);
  });

  it('rejects an oversized (>64KB) getSlot RPC body', async () => {
    // A misbehaving/hostile RPC must not be able to stream an unbounded 200 body
    // into memory. The body cap (parity with Go's io.LimitReader) rejects it
    // with a SignerError before JSON.parse.
    const huge = `{"jsonrpc":"2.0","id":1,"result":1000000,"pad":"${'x'.repeat(70_000)}"}`;
    mockRpc({ getSlot: { raw: huge } });
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    await expect(
      signer.signPayment(2625, GOLDEN_PROVIDER, resource(), escrowAccept()),
    ).rejects.toThrow(/exceeds 65536 bytes/);
  });

  it('caps the RPC body by UTF-8 byte length, not UTF-16 code units', async () => {
    // Regression for #484: the cap must be byte-exact (parity with Go's
    // io.LimitReader). A body padded with multi-byte UTF-8 characters can be
    // <= 65536 in String.length (UTF-16 code units) while exceeding 65536
    // bytes. The old `text().length` check would let it through; the
    // `arrayBuffer().byteLength` check must reject it.
    const pad = 'é'.repeat(40_000); // 'é' = 1 code unit, 2 UTF-8 bytes
    const body = `{"jsonrpc":"2.0","id":1,"result":1000000,"pad":"${pad}"}`;
    // Sanity: under the cap by code units, over it by bytes.
    expect(body.length).toBeLessThanOrEqual(65_536);
    expect(Buffer.byteLength(body, 'utf-8')).toBeGreaterThan(65_536);
    mockRpc({ getSlot: { raw: body } });
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    await expect(
      signer.signPayment(2625, GOLDEN_PROVIDER, resource(), escrowAccept()),
    ).rejects.toThrow(/exceeds 65536 bytes/);
  });

  it('the agent secret key never appears in an escrow error message', async () => {
    // Drive a builder-level failure after RPC by handing a malformed provider in
    // the accept's pay_to. The golden wallet seed is all 42 (0x2a); assert it
    // never leaks.
    mockRpc();
    const signer = new KeypairSigner(goldenAgentWallet(), RPC_URL);
    const bad = escrowAccept();
    (bad as { payTo: string }).payTo = 'not-base58-0OIl!!!';
    let caught: unknown;
    try {
      await signer.signPayment(2625, 'irrelevant', resource(), bad);
    } catch (e) {
      caught = e;
    }
    const msg = (caught as Error).message;
    expect(msg).not.toMatch(/42, ?42, ?42/);
    expect(msg).not.toMatch(/2a,? ?2a,? ?2a/i);
  });
});

describe('escrowExpirySlot clamping math (mirrors Rust/Go/Python)', () => {
  it('typical 300s timeout -> +750 slots', () => {
    expect(escrowExpirySlot(1_000_000, 300)).toBe(1_000_750);
  });

  it('zero seconds applies the 150-slot floor', () => {
    expect(escrowExpirySlot(1_000_000, 0)).toBe(1_000_150);
  });

  it('a huge timeout is clamped to the 10_000-slot cap', () => {
    expect(escrowExpirySlot(1_000_000, 2 ** 40)).toBe(1_010_000);
  });

  it('a negative timeout clamps to the floor, never pushing expiry backwards', () => {
    expect(escrowExpirySlot(1_000_000, -1)).toBe(1_000_150);
  });
});
