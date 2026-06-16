// THE FUND-SAFETY GATE: "verify-what-you-sign" for escrow deposits.
//
// The gateway returns an UNSIGNED message and a `decoded_intent`. Both are
// UNTRUSTED. Before the wallet signs, this module independently:
//   1. decodes the message bytes the wallet will actually sign,
//   2. parses the in-band instruction (discriminator, amount, service_id,
//      expiry) and the static account list,
//   3. RE-DERIVES the escrow PDA + ATAs from the *connected user's* pubkey, and
//   4. asserts the message, the intent, AND the re-derivation all agree, and
//      that the user-requested amount/service_id match too.
//
// The point of the re-derivation is fund safety: re-deriving the escrow PDA from
// the connected wallet and matching it to the message proves the deposit funds
// an escrow the user controls — not an attacker's. Any mismatch → reject.
//
// Browser-native ONLY: @solana/kit + Web APIs. No node:crypto, no Buffer.
// On a tamper we return `{ ok:false, reason }`; we throw ONLY on truly malformed
// base64 that cannot be decoded at all.

import {
  address,
  type Address,
  getProgramDerivedAddress,
  getAddressEncoder,
  getCompiledTransactionMessageDecoder,
  getBase64Encoder,
  getU64Decoder,
} from "@solana/kit";

import type { DecodedIntent } from "@/lib/escrow/types";
import { atomicToUsdcDisplay } from "@/lib/escrow/amount";

// Well-known program addresses (from [solvela-x402]).
const TOKEN_PROGRAM = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
const ATA_PROGRAM = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";
const SYSTEM_PROGRAM = "11111111111111111111111111111111";

// Client-owned canonical addresses (build-time). Defaults are mainnet; override
// per deployment (e.g. devnet) via NEXT_PUBLIC_*. The gateway-supplied
// program_id/usdc_mint are UNTRUSTED — pinning here is what makes the PDA/ATA
// re-derivation meaningful: a hostile gateway cannot substitute its own program
// or a fake mint. Fail-closed: any mismatch rejects.
const EXPECTED_ESCROW_PROGRAM =
  process.env.NEXT_PUBLIC_ESCROW_PROGRAM_ID ?? "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU";
const EXPECTED_USDC_MINT =
  process.env.NEXT_PUBLIC_USDC_MINT ?? "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

// Canonical instruction-account index order (writability-sorted positions).
const EXPECTED_ACCOUNT_INDICES = [0, 4, 5, 1, 2, 3, 6, 7, 8] as const;

// Canonical static-account count and instruction-data length.
const STATIC_ACCOUNT_COUNT = 10;
const IX_DATA_LEN = 56;

const addressEncoder = getAddressEncoder();
const base64Encoder = getBase64Encoder();
const u64Decoder = getU64Decoder();

/** Inputs to the verification gate. */
export interface VerifyInput {
  messageB64: string;
  decodedIntent: DecodedIntent;
  /** Connected pubkey + user-requested amount + the service_id we sent. */
  expected: { agentWallet: string; amountAtomic: string; serviceIdB64: string };
}

/** A fully-verified, safe-to-sign deposit summary. */
export interface VerifiedDeposit {
  agentWallet: string;
  amountAtomic: bigint;
  amountUsdc: string;
  escrowPda: string;
  vaultAta: string;
  provider: string;
  usdcMint: string;
  programId: string;
  expirySlot: number;
  recentBlockhash: string;
}

export type VerifyResult =
  | { ok: true; deposit: VerifiedDeposit }
  | { ok: false; reason: string };

function fail(reason: string): VerifyResult {
  return { ok: false, reason };
}

/**
 * Address (base58 string) → its raw 32 bytes. `address()` validates AND brands
 * the input, so a malformed pubkey throws here (callers wrap derivation in
 * try/catch and fail closed).
 */
function addressBytes(addr: string): Uint8Array {
  return new Uint8Array(addressEncoder.encode(address(addr)));
}

/**
 * SHA-256 over `data` using browser WebCrypto. Copies into a fresh
 * `ArrayBuffer`-backed view so the `BufferSource` type is exact (a subarray may
 * be `SharedArrayBuffer`-backed, which `digest` rejects on the type level).
 */
async function sha256(data: Uint8Array): Promise<Uint8Array> {
  const copy = new Uint8Array(data.length);
  copy.set(data);
  const digest = await globalThis.crypto.subtle.digest("SHA-256", copy);
  return new Uint8Array(digest);
}

/** The 8-byte Anchor discriminator for `deposit` = sha256("global:deposit")[..8]. */
async function depositDiscriminator(): Promise<Uint8Array> {
  const full = await sha256(new TextEncoder().encode("global:deposit"));
  return full.subarray(0, 8);
}

/** Escrow PDA = PDA(program, ["escrow", agent(32), service_id(32)]). */
async function deriveEscrowPda(
  programId: string,
  agentWallet: string,
  serviceId: Uint8Array,
): Promise<Address> {
  const [pda] = await getProgramDerivedAddress({
    programAddress: address(programId),
    seeds: [new TextEncoder().encode("escrow"), addressBytes(agentWallet), serviceId],
  });
  return pda;
}

/** ATA(owner, mint) = PDA(ATA_PROGRAM, [owner(32), TOKEN_PROGRAM(32), mint(32)]). */
async function deriveAta(owner: string, mint: string): Promise<Address> {
  const [ata] = await getProgramDerivedAddress({
    programAddress: address(ATA_PROGRAM),
    seeds: [addressBytes(owner), addressBytes(TOKEN_PROGRAM), addressBytes(mint)],
  });
  return ata;
}

/** Constant-time-ish byte compare (length-checked, full scan). */
function bytesEqual(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false;
  let diff = 0;
  for (let i = 0; i < a.length; i++) diff |= a[i] ^ b[i];
  return diff === 0;
}

function arraysEqual(a: readonly number[], b: readonly number[]): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}

/**
 * Verify what the wallet is about to sign. Returns `{ ok:true, deposit }` only
 * when EVERY check below holds; otherwise `{ ok:false, reason }`. Throws only on
 * base64 that cannot be decoded to bytes at all.
 */
export async function verifyDepositMessage(input: VerifyInput): Promise<VerifyResult> {
  const { messageB64, decodedIntent, expected } = input;

  // ── 1. Decode the message bytes. ──────────────────────────────────────────
  // Base64 that decodes is structural data; a decode failure is the only
  // "throw" case the spec permits, but we still wrap it as ok:false so callers
  // get a uniform result type for any untrusted-input failure.
  let messageBytes: Uint8Array;
  try {
    messageBytes = new Uint8Array(base64Encoder.encode(messageB64));
  } catch {
    return fail("message is not valid base64");
  }
  if (messageBytes.length === 0) {
    return fail("message is empty");
  }

  let decoded;
  try {
    decoded = getCompiledTransactionMessageDecoder().decode(messageBytes);
  } catch {
    return fail("message could not be decoded as a Solana transaction message");
  }

  // ── 2. Structural shape. ──────────────────────────────────────────────────
  if (decoded.version !== "legacy") {
    return fail("message is not a legacy transaction message");
  }
  if (decoded.header.numSignerAccounts !== 1) {
    return fail("message must have exactly one signer");
  }
  // Pin the full canonical legacy header [1, 0, 6] (1 signer, 0 readonly
  // signers, 6 readonly non-signers). The single-signer check above is not
  // enough: a tampered header that re-partitions writable/readonly accounts
  // could flip a readonly program/mint into a writable position. Assert the
  // remaining two header fields explicitly. (kit field names per
  // @solana/transaction-messages compile/legacy/header.d.ts.)
  if (decoded.header.numReadonlySignerAccounts !== 0) {
    return fail("message must have zero read-only signers");
  }
  if (decoded.header.numReadonlyNonSignerAccounts !== 6) {
    return fail("message must have exactly six read-only non-signer accounts");
  }
  const staticAccounts = decoded.staticAccounts;
  if (staticAccounts.length !== STATIC_ACCOUNT_COUNT) {
    return fail(
      `message must reference exactly ${STATIC_ACCOUNT_COUNT} accounts, found ${staticAccounts.length}`,
    );
  }

  // ── 3. Exactly one instruction, the escrow program at slot 9. ─────────────
  if (decoded.instructions.length !== 1) {
    return fail(
      `message must contain exactly one instruction, found ${decoded.instructions.length}`,
    );
  }
  const ix = decoded.instructions[0];
  if (ix.programAddressIndex !== 9) {
    return fail("instruction does not target the escrow program slot");
  }
  // Pin the program to the CLIENT-OWNED constant, not the gateway's intent. A
  // hostile gateway could name its own program here (whose `deposit` drains the
  // user's ATA); without this pin every downstream consistency check would still
  // pass. Assert both the on-chain message slot AND the intent equal the pin.
  if (staticAccounts[9] !== EXPECTED_ESCROW_PROGRAM) {
    return fail("program id in the message is not the expected escrow program");
  }
  if (decodedIntent.program_id !== EXPECTED_ESCROW_PROGRAM) {
    return fail("program id in the intent is not the expected escrow program");
  }

  // ── 4. Canonical account-index order. ─────────────────────────────────────
  const accountIndices = ix.accountIndices ?? [];
  if (!arraysEqual([...accountIndices], [...EXPECTED_ACCOUNT_INDICES])) {
    return fail("instruction account order is not the canonical deposit order");
  }

  // ── 5. Instruction data: length + deposit discriminator. ──────────────────
  const data = ix.data ?? new Uint8Array(0);
  if (data.length !== IX_DATA_LEN) {
    return fail(`instruction data must be ${IX_DATA_LEN} bytes, found ${data.length}`);
  }
  const expectedDisc = await depositDiscriminator();
  if (!bytesEqual(data.subarray(0, 8), expectedDisc)) {
    return fail("not a deposit instruction");
  }

  // ── 6. Parse in-band fields and cross-check intent + user-expected. ───────
  const amount = u64Decoder.decode(data.subarray(8, 16));
  const serviceIdBytes = data.subarray(16, 48);
  const expiry = u64Decoder.decode(data.subarray(48, 56));
  // The gateway echoes service_id base64-encoded; compare in that form.
  const serviceIdB64 = bytesToBase64(serviceIdBytes);

  // amount: message ↔ intent ↔ user-expected (all integer/BigInt).
  let intentAmount: bigint;
  let expectedAmount: bigint;
  try {
    intentAmount = BigInt(decodedIntent.amount);
    expectedAmount = BigInt(expected.amountAtomic);
  } catch {
    return fail("amount is not a valid integer");
  }
  if (amount !== intentAmount) {
    return fail("amount in the message does not match the intent");
  }
  if (amount !== expectedAmount) {
    return fail("amount in the message does not match the requested amount");
  }

  // service_id: message ↔ intent ↔ user-expected (base64 compare).
  if (serviceIdB64 !== decodedIntent.service_id) {
    return fail("service_id in the message does not match the intent");
  }
  if (serviceIdB64 !== expected.serviceIdB64) {
    return fail("service_id in the message does not match the requested service_id");
  }

  // expiry: message ↔ intent.
  if (expiry !== BigInt(decodedIntent.expiry_slot)) {
    return fail("expiry slot in the message does not match the intent");
  }

  // ── 7. Re-derive from the CONNECTED wallet and match message + intent. ────
  // This is the fund-safety core: the escrow PDA must derive from the user's
  // own pubkey, or the deposit would fund an escrow they do not control.
  let escrowPda: string;
  let agentAta: string;
  let vaultAta: string;
  try {
    // Derive from the PINNED program + mint (proven equal to the intent above),
    // NOT from gateway-supplied fields. This is what binds the re-derivation to a
    // program the client trusts rather than one the gateway named.
    escrowPda = await deriveEscrowPda(EXPECTED_ESCROW_PROGRAM, expected.agentWallet, serviceIdBytes);
    agentAta = await deriveAta(expected.agentWallet, EXPECTED_USDC_MINT);
    vaultAta = await deriveAta(escrowPda, EXPECTED_USDC_MINT);
  } catch {
    return fail("failed to re-derive escrow accounts from the connected wallet");
  }

  // fee payer / signer is the connected wallet.
  if (staticAccounts[0] !== expected.agentWallet) {
    return fail("the transaction is not signed by the connected wallet");
  }

  // escrow PDA: re-derived === intent === message slot 1.
  if (escrowPda !== decodedIntent.escrow_pda) {
    return fail("re-derived escrow account does not match the intent");
  }
  if (escrowPda !== staticAccounts[1]) {
    return fail("escrow account in the message does not match the re-derived value");
  }

  // agent ATA: re-derived === message slot 2.
  if (agentAta !== staticAccounts[2]) {
    return fail("agent token account in the message does not match the re-derived value");
  }

  // vault ATA: re-derived === intent === message slot 3.
  if (vaultAta !== decodedIntent.vault_ata) {
    return fail("re-derived vault account does not match the intent");
  }
  if (vaultAta !== staticAccounts[3]) {
    return fail("vault account in the message does not match the re-derived value");
  }

  // provider / mint / programs at their canonical slots.
  if (staticAccounts[4] !== decodedIntent.provider) {
    return fail("provider in the message does not match the intent");
  }
  // Pin the mint to the CLIENT-OWNED constant, not the gateway's intent. A fake
  // mint substituted by a hostile gateway would re-derive a self-consistent (but
  // wrong) ATA pair and pass every internal cross-check. Assert both the on-chain
  // message slot AND the intent equal the pin.
  if (staticAccounts[5] !== EXPECTED_USDC_MINT) {
    return fail("USDC mint in the message is not the expected mint");
  }
  if (decodedIntent.usdc_mint !== EXPECTED_USDC_MINT) {
    return fail("USDC mint in the intent is not the expected mint");
  }
  if (staticAccounts[6] !== TOKEN_PROGRAM) {
    return fail("token program in the message is not canonical");
  }
  if (staticAccounts[7] !== ATA_PROGRAM) {
    return fail("associated-token program in the message is not canonical");
  }
  if (staticAccounts[8] !== SYSTEM_PROGRAM) {
    return fail("system program in the message is not canonical");
  }

  // ── 8. Blockhash (lifetimeToken) === intent.recent_blockhash. ─────────────
  // V-1 DECLINED (anchor blockhash to an independent `expected` value): the
  // client has no INDEPENDENT blockhash source — the UI only sees the gateway's
  // response, so comparing `expected.recentBlockhash` would compare the gateway's
  // value to itself (vacuous). On-chain liveness already rejects a stale
  // blockhash at broadcast. This internal-consistency check (message ↔ intent) is
  // all that is meaningful here.
  const lifetimeToken = decoded.lifetimeToken;
  if (lifetimeToken !== decodedIntent.recent_blockhash) {
    return fail("recent blockhash in the message does not match the intent");
  }

  // All checks passed — safe to sign.
  return {
    ok: true,
    deposit: {
      agentWallet: expected.agentWallet,
      amountAtomic: amount,
      amountUsdc: atomicToUsdcDisplay(amount),
      escrowPda,
      vaultAta,
      provider: decodedIntent.provider,
      // Report the PINNED program/mint (proven equal to the intent above), so the
      // summary the user signs against reflects the client-trusted addresses.
      usdcMint: EXPECTED_USDC_MINT,
      programId: EXPECTED_ESCROW_PROGRAM,
      expirySlot: Number(expiry),
      recentBlockhash: lifetimeToken,
    },
  };
}

// Encode raw bytes to standard-alphabet base64 (browser-native, no Buffer).
// Used to compare the in-band 32-byte service_id against the gateway's
// base64-echoed `decoded_intent.service_id` and the user-supplied `serviceIdB64`.
// V-6 DECLINED (URL-safe base64 normalization): the gateway emits STANDARD-
// alphabet base64 (golden-vector-pinned; the Rust handler uses `STANDARD.encode`),
// so this standard-alphabet contract is exact. A variant mismatch could only
// cause a false-reject, never a verification bypass — so no normalization.
function bytesToBase64(bytes: Uint8Array): string {
  let bin = "";
  for (let i = 0; i < bytes.length; i++) bin += String.fromCharCode(bytes[i]);
  return btoa(bin);
}
