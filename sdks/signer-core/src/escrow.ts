/**
 * Escrow deposit-transaction builder for @solvela/signer-core.
 *
 * Faithful port of the canonical Rust builder `crates/escrow-tx/src/deposit.rs`
 * (+ `pda.rs`), kept byte-for-byte identical to the canonical TypeScript SDK
 * builder `sdks/typescript/src/escrow.ts`. The output MUST reproduce
 * {@link GOLDEN_VECTOR_B64} for the fixed golden-vector input; the gateway
 * verifier (`crates/x402/src/escrow/`) independently re-derives the same layout,
 * so any divergence here is a fund-misdirection bug, not a style nit.
 *
 * Why this exists as a local port (not an import of `@solvela/sdk`):
 * signer-core is deliberately self-contained — see the history note in
 * `sign.ts`. The published `@solvela/sdk` was wire-incompatible with the
 * production gateway, and signer-core was created specifically to avoid a dead
 * workspace dependency on it. So we port the canonical builder verbatim and pin
 * it against the same Rust golden vector + a drift-guard test, rather than
 * depending on the canonical SDK package.
 *
 * Wire layout (single source of truth — see the Rust file and the `solvela-x402`
 * skill):
 *
 *   - Escrow PDA seeds: `[b"escrow", agent_pubkey(32), service_id(32)]`.
 *   - Vault ATA: `ATA(escrow_pda, usdc_mint)` (escrow PDA is off-curve, so the
 *     derivation must allow an off-curve owner).
 *   - Instruction data (56 bytes): `anchorDiscriminator("deposit")[8]` ||
 *     `amount: u64 LE` || `service_id: [32]` || `expiry_slot: u64 LE`.
 *   - Account list sorted by writability; program key appended last (index 9).
 *   - Instruction account-index order: `[0, 4, 5, 1, 2, 3, 6, 7, 8]`.
 *   - Legacy message header `[1, 0, 6]`; length prefixes are single-byte
 *     compact-u16 (valid only for lengths <= 127 — reject larger rather than
 *     truncating).
 *   - Wire tx: `compact-u16(1) || ed25519_signature(64) || message`, then base64
 *     (standard alphabet).
 *
 * We deliberately do NOT use `@solana/web3.js`'s `Message` / `Transaction`
 * compilation to assemble the escrow message: those re-derive their own account
 * ordering and would not match the canonical writability-sorted layout
 * byte-for-byte (that v0-VersionedTransaction approach was the wire-drift bug
 * this builder replaces). We use web3.js / spl-token only for the primitives
 * whose output is canonical and already pinned as ground truth:
 * `PublicKey.findProgramAddressSync` (PDA + ATA derivation) — the golden vector
 * is the arbiter that these agree — plus a raw ed25519 signature over the
 * serialized message bytes. The Anchor discriminator and account ordering are
 * computed locally with the Node `crypto` stdlib.
 */

import { createHash, createPrivateKey, sign as edSign } from 'node:crypto';

import { Keypair, PublicKey } from '@solana/web3.js';
import {
  getAssociatedTokenAddressSync,
  TOKEN_PROGRAM_ID as SPL_TOKEN_PROGRAM_ID,
  ASSOCIATED_TOKEN_PROGRAM_ID as SPL_ATA_PROGRAM_ID,
} from '@solana/spl-token';

import { SigningError } from './sign.js';

// ---------------------------------------------------------------------------
// Well-known program constants (match crates/escrow-tx/src/pda.rs).
// ---------------------------------------------------------------------------

const TOKEN_PROGRAM_ID = 'TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA';
const ATA_PROGRAM_ID = 'ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL';
const SYSTEM_PROGRAM_ID = '11111111111111111111111111111111';

/**
 * Largest length representable as a single compact-u16 byte. Lengths above this
 * would require a continuation byte; emitting a bare byte there would corrupt
 * the encoding, so we reject (mirrors the Rust/Go/Python/TS builders).
 */
const COMPACT_U16_SINGLE_BYTE_MAX = 127;

const SIGNATURE_LENGTH = 64;
const PUBKEY_LENGTH = 32;
const IX_DATA_LENGTH = 56;

/**
 * DER prefix for a PKCS#8-wrapped Ed25519 private key (RFC 8410). The 32-byte
 * raw seed is appended to form a complete PKCS#8 key Node's crypto can import.
 * Mirrors the canonical SDK `Wallet.signMessage` path: we sign the raw legacy
 * message bytes directly (NOT via `Transaction.sign`, which would re-compile and
 * re-order the message and break byte-for-byte parity with the golden vector).
 */
const ED25519_PKCS8_PREFIX = Buffer.from('302e020100300506032b657004220420', 'hex');

// ---------------------------------------------------------------------------
// Escrow expiry-slot bounds (mirror the Rust/Go/Python/TS SDK signers).
//
// FINDING 2 (audit 2026-06-08): signer-core previously used
// `Math.max(Math.floor(maxTimeoutSeconds*1000/400), 10)` — a 10-slot floor with
// no upper clamp and no plausibility check. The gateway verifier rejects when
// expiry_slot - current_slot < MIN_EXPIRY_BUFFER_SLOTS (50)
// (crates/x402/src/escrow/verifier.rs), so a signer-core deposit with
// maxTimeoutSeconds < ~20 was dead-on-arrival. These constants are ported
// verbatim from sdks/typescript/src/signer.ts (and go/python/rust) so all
// signers choose compatible expiry slots and none is bounced.
// ---------------------------------------------------------------------------

/**
 * Upper bound on how far ahead an escrow may expire (~66 min). Rejects a
 * malicious/misconfigured gateway from pushing expiry to "never".
 */
export const MAX_ESCROW_EXPIRY_SLOTS_AHEAD = 10_000;

/**
 * Lower bound on expiry distance. Mirrors AND exceeds the gateway's
 * MIN_EXPIRY_BUFFER_SLOTS = 50 (crates/x402/src/escrow/verifier.rs); the extra
 * headroom (150 vs 50) absorbs slot-skew between the slot we read and the slot
 * the gateway verifies against. ~150 slots ~= 60 s.
 */
export const MIN_ESCROW_EXPIRY_SLOTS_AHEAD = 150;

/**
 * Floor below which a getSlot result is implausible and rejected. A stub /
 * genesis / freshly-reset validator can answer getSlot with a near-zero slot;
 * computing an escrow expiry from that base yields an already-expired (or
 * trivially-near-expiry) deposit the gateway would bounce. Mainnet/devnet have
 * been well past this for years. Fail closed rather than silently signing a
 * dead-on-arrival escrow deposit.
 */
export const MIN_PLAUSIBLE_SLOT = 1_000_000;

/**
 * Compute the slot at which a now-created escrow PDA should expire.
 *
 * Ported from the canonical SDKs' `escrowExpirySlot` / `escrow_expiry_slot`:
 * ~2.5 slots/s (1000 ms / 400 ms), clamped into
 * `[MIN_ESCROW_EXPIRY_SLOTS_AHEAD, MAX_ESCROW_EXPIRY_SLOTS_AHEAD]`. The floor
 * mirrors and exceeds the gateway's MIN_EXPIRY_BUFFER_SLOTS so a too-near expiry
 * is never bounced; the cap rejects an unbounded-future expiry. A negative
 * timeout is floored to 0 (never pushes expiry backwards into a dead-on-arrival
 * deposit). `currentSlot` and the effective offset stay within
 * Number.MAX_SAFE_INTEGER for any plausible slot, so the add cannot lose
 * precision.
 */
export function escrowExpirySlot(currentSlot: number, maxTimeoutSeconds: number): number {
  const timeout = Math.max(maxTimeoutSeconds, 0);
  // timeout * 1000 / 400, floored to whole slots (matches the Rust integer math).
  const timeoutSlots = Math.floor((timeout * 1000) / 400);
  const effective = Math.max(
    MIN_ESCROW_EXPIRY_SLOTS_AHEAD,
    Math.min(timeoutSlots, MAX_ESCROW_EXPIRY_SLOTS_AHEAD),
  );
  return currentSlot + effective;
}

/**
 * Fail closed if a getSlot result is implausibly low (stub/genesis/reset node).
 *
 * Computing an escrow expiry from a near-zero base slot yields an already- or
 * trivially-near-expiry deposit the gateway bounces; the MCP standalone-deposit
 * path would broadcast it on-chain first. Reject rather than sign a
 * dead-on-arrival escrow. Mirrors the `fetchCurrentSlot` MIN_PLAUSIBLE_SLOT
 * check in the canonical SDK signers.
 */
export function assertPlausibleSlot(slot: number): void {
  if (typeof slot !== 'number' || !Number.isSafeInteger(slot) || slot < 0) {
    throw new SigningError('getSlot RPC: missing or invalid result');
  }
  if (slot < MIN_PLAUSIBLE_SLOT) {
    throw new SigningError(
      `getSlot RPC returned implausibly low slot ${slot} ` +
        `(< ${MIN_PLAUSIBLE_SLOT}); refusing to build an escrow deposit`,
    );
  }
}

/**
 * Byte-exact base64 deposit transaction for the fixed golden-vector input,
 * copied verbatim from `GOLDEN_VECTOR_B64` in `crates/escrow-tx/src/deposit.rs`
 * (and `sdks/typescript/src/escrow.ts`'s `GOLDEN_VECTOR_B64`).
 *
 * DO NOT hand-edit. If this changes, the deposit-tx layout changed — the
 * on-chain/Rust value is ground truth. The drift-guard test in
 * `tests/escrow.test.ts` pins this against the Rust source file.
 *
 * Fixed input: agent keypair from seed `[42; 32]`, provider
 * `9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM`, USDC mint
 * `EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v`, program
 * `9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU`, `service_id=[7; 32]`,
 * `expiry_slot=1_000_750`, `recent_blockhash=[0xAB; 32]`, `amount=2625`.
 */
export const GOLDEN_VECTOR_B64 =
  'Ad449R7ht12UKokgbDUYh29Ey/wDEwPioT/QtJOPKwSO4RHIaFRqS0DH+bPpEo2vCK78jbRLpP/6FV/5TY+qLw8BAAYKGX9rI+FshTLGq8g4+s1ep4m+DHaykgM0A5v6iz02jWFyvMLyv5o/k5XBGVxL9QMaEsQP6v01aK7nXazb0N/wMBS12eVticTdq8DzgM47jC/J8D1uNnFG6ZNqwpSU6GOelSnAbR4dY/56biCP6roeTxLQ8Q4TL7a2ArnBn2RW9HJ+jAiHYL/eHd3PMsF/IJuCQu5SqvEx+s2I0OosbQsG8sb6evO+2606PWXzaqvJdDGxu+TC0vbg5HymAgNFL11hBt324ddloZPZy+FGzut5rBy0he1fWzeROoz1hX7/AKmMlyWPTiSJ8bs9ECkUjg2DC1oTmdr/EIQEjnvY2+n4WQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAgo61GtoIM8r3akxjdsa2N5CEHZ8QBYuAbfwPDOL78eGrq6urq6urq6urq6urq6urq6urq6urq6urq6urq6urqwEJCQAEBQECAwYHCDjyI8aJUuHytkEKAAAAAAAABwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcuRQ8AAAAAAA==';

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

/**
 * Inputs required to build an escrow deposit transaction.
 *
 * `serviceId`, `expirySlot`, and `recentBlockhash` are injected so the builder
 * is a pure, deterministic function (golden-vector testable). Production code
 * (the escrow signer in `sign.ts`) generates the CSPRNG `serviceId`, computes
 * `expirySlot` from the current slot via {@link escrowExpirySlot}, and fetches
 * the blockhash, then calls this builder.
 *
 * SECURITY: the builder reads the keypair's public key and signs the raw
 * message bytes via an in-process Node `KeyObject` (PKCS#8 import of the
 * 32-byte seed). The secret bytes are never logged and never leave this module.
 */
export interface EscrowDepositParams {
  /** Agent keypair whose ed25519 key signs the deposit and seeds the escrow PDA. */
  readonly keypair: Keypair;
  /** Base58-encoded provider wallet pubkey. */
  readonly providerWalletB58: string;
  /** Base58-encoded USDC mint pubkey. */
  readonly usdcMintB58: string;
  /** Base58-encoded escrow program id. */
  readonly escrowProgramIdB58: string;
  /** Amount to deposit in atomic USDC units (must be a positive integer). */
  readonly amount: number;
  /** 32-byte service identifier that seeds the escrow PDA. */
  readonly serviceId: Uint8Array;
  /** Slot at which the escrow deposit expires (passed to the on-chain instruction). */
  readonly expirySlot: number;
  /** Recent blockhash (raw 32 bytes) from `getLatestBlockhash`. */
  readonly recentBlockhash: Uint8Array;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/**
 * Compute the Anchor instruction discriminator: `sha256("global:<name>")[..8]`.
 */
export function anchorDiscriminator(name: string): Uint8Array {
  return new Uint8Array(createHash('sha256').update(`global:${name}`).digest().subarray(0, 8));
}

/**
 * Reject a non-integer, non-positive, or precision-unsafe atomic amount.
 *
 * JavaScript numbers are IEEE-754 doubles: only integers up to
 * `Number.MAX_SAFE_INTEGER` (2^53 - 1) round-trip faithfully, and a u64 amount
 * field above that cannot be represented exactly. A float would be silently
 * truncated into the on-chain u64 and mis-charge the agent. Fail closed at the
 * boundary — parity with the canonical SDKs' `assertValidAmount`.
 */
function assertValidAmount(amount: number): void {
  if (!Number.isInteger(amount)) {
    throw new SigningError('deposit amount must be an integer number of atomic USDC units');
  }
  if (amount <= 0) {
    throw new SigningError('deposit amount must be greater than zero');
  }
  if (amount > Number.MAX_SAFE_INTEGER) {
    throw new SigningError('deposit amount exceeds the safe-integer range (cannot encode as u64)');
  }
}

/**
 * Reject a non-integer, non-finite, or non-positive expiry slot.
 *
 * `expiry_slot` is encoded directly into the on-chain `u64` via {@link u64LE}.
 * An `expirySlot` of 0 silently encodes a dead-on-arrival deposit, and
 * `NaN`/`Infinity`/`-1` make `BigInt(value)` throw a raw untyped error rather
 * than a `SigningError`. Fail closed here, parallel to {@link assertValidAmount}.
 */
function assertValidExpirySlot(expirySlot: number): void {
  if (!Number.isInteger(expirySlot)) {
    throw new SigningError('expiry_slot must be a positive integer');
  }
  if (expirySlot <= 0) {
    throw new SigningError('expiry_slot must be a positive integer');
  }
}

/** Encode a non-negative safe-integer as 8 little-endian bytes (u64). */
function u64LE(value: number): Uint8Array {
  const buf = Buffer.alloc(8);
  buf.writeBigUInt64LE(BigInt(value));
  return new Uint8Array(buf);
}

/**
 * Decode a base58 pubkey into 32 raw bytes, wrapping any failure in a
 * SigningError that names the offending field but never echoes secret material.
 */
function decodeBs58Pubkey(field: string, b58: string): Uint8Array {
  let key: PublicKey;
  try {
    key = new PublicKey(b58);
  } catch (e) {
    const reason = e instanceof Error ? e.message : String(e);
    throw new SigningError(`invalid address for ${field}: ${reason}`);
  }
  const bytes = key.toBytes();
  if (bytes.length !== PUBKEY_LENGTH) {
    throw new SigningError(`invalid address for ${field}: expected 32 bytes, got ${bytes.length}`);
  }
  return bytes;
}

/**
 * Sign raw message bytes with the keypair's Ed25519 key, returning the 64-byte
 * detached signature. The 32-byte seed is imported into an in-process Node
 * `KeyObject` (PKCS#8) and used for the signature only — it is never logged.
 * Mirrors the canonical SDK `Wallet.signMessage`.
 */
function signMessage(keypair: Keypair, message: Uint8Array): Uint8Array {
  const seed = keypair.secretKey.slice(0, 32);
  const pkcs8 = Buffer.concat([ED25519_PKCS8_PREFIX, Buffer.from(seed)]);
  const keyObject = createPrivateKey({ key: pkcs8, format: 'der', type: 'pkcs8' });
  return new Uint8Array(edSign(null, Buffer.from(message), keyObject));
}

// ---------------------------------------------------------------------------
// Legacy message serializer (manual — must match the Rust byte layout exactly).
// ---------------------------------------------------------------------------

/**
 * Serialize a Solana legacy transaction message.
 *
 * Layout: `header(3) || acct_count(u8) || N*32 keys (+ program key) ||
 * blockhash(32) || ix_count(u8)=1 || program_id_index(u8) || ix_acct_count(u8)
 * || ix_acct_indices || data_len(u8) || data`.
 *
 * Length prefixes are emitted as a single byte — the correct compact-u16
 * encoding only for lengths <= 127. Larger values are rejected (not truncated),
 * mirroring the Rust/Go/Python/TS builders.
 */
function buildLegacyMessage(
  header: readonly [number, number, number],
  accounts: readonly Uint8Array[],
  programId: Uint8Array,
  recentBlockhash: Uint8Array,
  programIdIndex: number,
  ixAccountIndices: Uint8Array,
  ixData: Uint8Array,
): Uint8Array {
  const totalAccounts = accounts.length + 1; // +1 for the appended program key
  if (totalAccounts > COMPACT_U16_SINGLE_BYTE_MAX) {
    throw new SigningError(
      `account count ${totalAccounts} exceeds the 127-byte compact-u16 single-byte limit`,
    );
  }
  if (ixAccountIndices.length > COMPACT_U16_SINGLE_BYTE_MAX) {
    throw new SigningError(
      `instruction account count ${ixAccountIndices.length} exceeds the 127-byte compact-u16 single-byte limit`,
    );
  }
  if (ixData.length > COMPACT_U16_SINGLE_BYTE_MAX) {
    throw new SigningError(
      `instruction data length ${ixData.length} exceeds the 127-byte compact-u16 single-byte limit`,
    );
  }

  const parts: Uint8Array[] = [];
  parts.push(Uint8Array.from(header));
  parts.push(Uint8Array.from([totalAccounts]));
  for (const acc of accounts) {
    parts.push(acc);
  }
  parts.push(programId); // program key is the last account
  parts.push(recentBlockhash);
  parts.push(Uint8Array.from([1])); // instruction count
  parts.push(Uint8Array.from([programIdIndex]));
  parts.push(Uint8Array.from([ixAccountIndices.length]));
  parts.push(ixAccountIndices);
  parts.push(Uint8Array.from([ixData.length]));
  parts.push(ixData);

  return new Uint8Array(Buffer.concat(parts.map((p) => Buffer.from(p))));
}

// ---------------------------------------------------------------------------
// Public builder
// ---------------------------------------------------------------------------

/**
 * Build a signed escrow `deposit` transaction, base64-encoded (standard
 * alphabet).
 *
 * Returns a wire-format Solana legacy transaction ready for the gateway /
 * `sendTransaction`. Pure with respect to the network (no I/O): for fixed
 * inputs it always produces the same bytes, which is what lets
 * {@link GOLDEN_VECTOR_B64} pin the wire layout.
 *
 * @throws SigningError on a zero/negative/float/out-of-range amount, a
 *   non-positive/non-integer/non-finite expiry_slot, a malformed
 *   provider/mint/program address, a wrong-length service_id/blockhash, a
 *   PDA/ATA derivation failure, or a message-too-long length prefix.
 */
export function buildEscrowDepositTx(params: EscrowDepositParams): string {
  // Step 1: fail-closed amount + expiry-slot guards, BEFORE any derivation.
  assertValidAmount(params.amount);
  assertValidExpirySlot(params.expirySlot);

  // Step 2: validate the injected fixed-size fields.
  if (params.serviceId.length !== PUBKEY_LENGTH) {
    throw new SigningError(`service_id must be 32 bytes, got ${params.serviceId.length}`);
  }
  if (params.recentBlockhash.length !== PUBKEY_LENGTH) {
    throw new SigningError(`recent_blockhash must be 32 bytes, got ${params.recentBlockhash.length}`);
  }

  // Step 3: agent pubkey that seeds the escrow PDA and signs the deposit.
  const agentPubkey = params.keypair.publicKey.toBytes();

  // Step 4: parse all addresses.
  const provider = decodeBs58Pubkey('provider_wallet', params.providerWalletB58);
  const usdcMint = decodeBs58Pubkey('usdc_mint', params.usdcMintB58);
  const escrowProgram = decodeBs58Pubkey('escrow_program_id', params.escrowProgramIdB58);
  const tokenProgram = decodeBs58Pubkey('token_program', TOKEN_PROGRAM_ID);
  const ataProgram = decodeBs58Pubkey('ata_program', ATA_PROGRAM_ID);
  const systemProgram = decodeBs58Pubkey('system_program', SYSTEM_PROGRAM_ID);

  // Step 5: derive escrow PDA, agent ATA, and vault ATA. We use the canonical
  // web3.js / spl-token primitives; the golden vector is the arbiter that they
  // reproduce the on-chain layout. The vault ATA's owner is the escrow PDA
  // (off-curve), so `allowOwnerOffCurve` must be true.
  const escrowProgramKey = new PublicKey(escrowProgram);
  const usdcMintKey = new PublicKey(usdcMint);
  let escrowPda: PublicKey;
  try {
    [escrowPda] = PublicKey.findProgramAddressSync(
      [Buffer.from('escrow'), Buffer.from(agentPubkey), Buffer.from(params.serviceId)],
      escrowProgramKey,
    );
  } catch (e) {
    const reason = e instanceof Error ? e.message : String(e);
    throw new SigningError(`failed to derive escrow PDA: ${reason}`);
  }

  let agentAta: Uint8Array;
  let vaultAta: Uint8Array;
  try {
    agentAta = getAssociatedTokenAddressSync(
      usdcMintKey,
      new PublicKey(agentPubkey),
      false,
      SPL_TOKEN_PROGRAM_ID,
      SPL_ATA_PROGRAM_ID,
    ).toBytes();
    vaultAta = getAssociatedTokenAddressSync(
      usdcMintKey,
      escrowPda,
      true, // escrow PDA owner is off-curve
      SPL_TOKEN_PROGRAM_ID,
      SPL_ATA_PROGRAM_ID,
    ).toBytes();
  } catch (e) {
    const reason = e instanceof Error ? e.message : String(e);
    throw new SigningError(`failed to derive ATA: ${reason}`);
  }

  // Step 6: account list sorted by writability (writable signer, writable
  // non-signers, readonly non-signers). The program key is appended last
  // (index 9) inside the message serializer.
  const accounts: Uint8Array[] = [
    agentPubkey, // 0: signer, writable
    escrowPda.toBytes(), // 1: writable, non-signer
    agentAta, // 2: writable, non-signer
    vaultAta, // 3: writable, non-signer
    provider, // 4: readonly
    usdcMint, // 5: readonly
    tokenProgram, // 6: readonly
    ataProgram, // 7: readonly
    systemProgram, // 8: readonly
  ];

  // Step 7: instruction data = discriminator || amount(LE u64) || service_id ||
  // expiry_slot(LE u64) = 8 + 8 + 32 + 8 = 56 bytes.
  const ixData = new Uint8Array(
    Buffer.concat([
      Buffer.from(anchorDiscriminator('deposit')),
      Buffer.from(u64LE(params.amount)),
      Buffer.from(params.serviceId),
      Buffer.from(u64LE(params.expirySlot)),
    ]),
  );
  if (ixData.length !== IX_DATA_LENGTH) {
    throw new SigningError(`deposit ix_data must be 56 bytes, got ${ixData.length}`);
  }

  // Anchor program account order remapped into the writability-sorted layout.
  const ixAccountIndices = Uint8Array.from([0, 4, 5, 1, 2, 3, 6, 7, 8]);

  // Step 8: legacy message. Header [1, 0, 6]; program_id_index = 9 (last).
  const msg = buildLegacyMessage(
    [1, 0, 6],
    accounts,
    escrowProgram,
    params.recentBlockhash,
    9, // program_id_index = 9 (last account)
    ixAccountIndices,
    ixData,
  );

  // Step 9: sign the raw message bytes with the agent keypair (ed25519).
  const signature = signMessage(params.keypair, msg);
  if (signature.length !== SIGNATURE_LENGTH) {
    throw new SigningError(
      `unexpected ed25519 signature length ${signature.length} (want ${SIGNATURE_LENGTH})`,
    );
  }

  // Step 10: assemble wire tx: compact-u16(1) || signature(64) || message.
  const wire = Buffer.concat([Buffer.from([1]), Buffer.from(signature), Buffer.from(msg)]);

  return wire.toString('base64');
}
