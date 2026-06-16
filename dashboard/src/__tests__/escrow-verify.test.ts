import { describe, it, expect } from "vitest";
import {
  address,
  getAddressEncoder,
  getBase64Encoder,
  getBase64Decoder,
  getCompiledTransactionMessageDecoder,
} from "@solana/kit";

import { verifyDepositMessage } from "@/lib/escrow/verify";
import type { DecodedIntent } from "@/lib/escrow/types";

// ── Golden-vector ground truth (the canonical signed-tx vector) ──────────────
// GOLDEN_VECTOR_B64 lives in sdks/typescript/src/escrow.ts and is
// `compact-u16(1) || sig(64) || message`. The deposit-tx endpoint returns ONLY
// the message (base64). We compute the positive-test message fixture here from
// the golden vector — never hardcode a guess.
const GOLDEN_VECTOR_B64 =
  "Ad449R7ht12UKokgbDUYh29Ey/wDEwPioT/QtJOPKwSO4RHIaFRqS0DH+bPpEo2vCK78jbRLpP/6FV/5TY+qLw8BAAYKGX9rI+FshTLGq8g4+s1ep4m+DHaykgM0A5v6iz02jWFyvMLyv5o/k5XBGVxL9QMaEsQP6v01aK7nXazb0N/wMBS12eVticTdq8DzgM47jC/J8D1uNnFG6ZNqwpSU6GOelSnAbR4dY/56biCP6roeTxLQ8Q4TL7a2ArnBn2RW9HJ+jAiHYL/eHd3PMsF/IJuCQu5SqvEx+s2I0OosbQsG8sb6evO+2606PWXzaqvJdDGxu+TC0vbg5HymAgNFL11hBt324ddloZPZy+FGzut5rBy0he1fWzeROoz1hX7/AKmMlyWPTiSJ8bs9ECkUjg2DC1oTmdr/EIQEjnvY2+n4WQAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAgo61GtoIM8r3akxjdsa2N5CEHZ8QBYuAbfwPDOL78eGrq6urq6urq6urq6urq6urq6urq6urq6urq6urq6urqwEJCQAEBQECAwYHCDjyI8aJUuHytkEKAAAAAAAABwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcuRQ8AAAAAAA==";

const b64enc = getBase64Encoder();
const b64dec = getBase64Decoder();
const addrEnc = getAddressEncoder();

/** Base58 pubkey string → its raw 32 bytes (matches verify.ts derivation). */
function pubkeyBytes(addr: string): Uint8Array {
  return new Uint8Array(addrEnc.encode(address(addr)));
}

/** Strip the `compact-u16(1) || sig(64)` prefix → message bytes only. */
function goldenMessageBytes(): Uint8Array {
  const full = new Uint8Array(b64enc.encode(GOLDEN_VECTOR_B64));
  return full.subarray(65);
}

function goldenMessageB64(): string {
  return b64dec.decode(goldenMessageBytes());
}

// Connected-wallet + request fixtures matching the golden vector.
const AGENT = "2iXtA8oeZqUU5pofxK971TCEvFGfems2AcDRaZHKD2pQ";
const PROGRAM = "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU";
const MINT = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
const PROVIDER = "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM";
const ESCROW_PDA = "8itRK9Q7QErNubyjhuTjs85FHAfmQ7FzKjPRp6FBsaf1";
const VAULT_ATA = "B3Gfznfz8k5ePdQgAqRkWCvL5U5Pg1jL8SbEoGKt7BXX";
const SERVICE_ID_B64 = "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc="; // [7;32]
const AMOUNT = "2625";
const EXPIRY = 1000750;
const BLOCKHASH = "CZ8YUVdk7znjrUmnb5n7kgySk9yRAsQDYmyCxzfSky9t";

function goldenIntent(): DecodedIntent {
  return {
    program_id: PROGRAM,
    usdc_mint: MINT,
    provider: PROVIDER,
    escrow_pda: ESCROW_PDA,
    vault_ata: VAULT_ATA,
    amount: AMOUNT,
    service_id: SERVICE_ID_B64,
    expiry_slot: EXPIRY,
    recent_blockhash: BLOCKHASH,
  };
}

function goldenExpected() {
  return { agentWallet: AGENT, amountAtomic: AMOUNT, serviceIdB64: SERVICE_ID_B64 };
}

/**
 * Decode the golden message, apply a byte/struct mutation, and re-encode to
 * base64. Lets negative tests tamper with the actual signed bytes the wallet
 * would sign.
 */
function mutatedMessageB64(mutate: (data: Uint8Array) => Uint8Array): string {
  // We mutate the *instruction data* region in place. The message layout keeps
  // the 56-byte ix data contiguous at the tail; decode to find it, mutate the
  // raw bytes, re-encode.
  const bytes = new Uint8Array(goldenMessageBytes());
  return b64dec.decode(mutate(bytes));
}

/**
 * Decode a compiled message and narrow it to the legacy shape (which carries
 * `instructions`). The kit decoder returns a versioned union; legacy messages
 * always have an `instructions` array here.
 */
function decodeLegacy(bytes: Uint8Array) {
  const dec = getCompiledTransactionMessageDecoder().decode(bytes);
  if (dec.version !== "legacy") throw new Error("not a legacy message");
  return dec;
}

// Find the byte offset of the 56-byte instruction data inside the message so we
// can flip individual bytes. The ix.data is the last 56 bytes before any
// trailing structure; we locate it by decoding and matching.
function ixDataOffset(bytes: Uint8Array): number {
  const dec = decodeLegacy(bytes);
  const ixData = dec.instructions[0].data ?? new Uint8Array(0);
  // Search for the 56-byte ix data sequence in the message bytes.
  outer: for (let i = bytes.length - ixData.length; i >= 0; i--) {
    for (let j = 0; j < ixData.length; j++) {
      if (bytes[i + j] !== ixData[j]) continue outer;
    }
    return i;
  }
  throw new Error("could not locate ix data in message bytes");
}

describe("verifyDepositMessage — positive", () => {
  it("accepts the golden-vector message with a matching intent + connected wallet", async () => {
    const result = await verifyDepositMessage({
      messageB64: goldenMessageB64(),
      decodedIntent: goldenIntent(),
      expected: goldenExpected(),
    });

    expect(result.ok).toBe(true);
    if (result.ok) {
      expect(result.deposit.agentWallet).toBe(AGENT);
      expect(result.deposit.amountAtomic).toBe(BigInt(2625));
      expect(result.deposit.amountUsdc).toBe("0.002625");
      expect(result.deposit.escrowPda).toBe(ESCROW_PDA);
      expect(result.deposit.vaultAta).toBe(VAULT_ATA);
      expect(result.deposit.provider).toBe(PROVIDER);
      expect(result.deposit.usdcMint).toBe(MINT);
      expect(result.deposit.programId).toBe(PROGRAM);
      expect(result.deposit.expirySlot).toBe(EXPIRY);
      expect(result.deposit.recentBlockhash).toBe(BLOCKHASH);
    }
  });
});

describe("verifyDepositMessage — malformed input (truly bad base64)", () => {
  it("rejects non-base64 garbage without throwing", async () => {
    const result = await verifyDepositMessage({
      messageB64: "!!!not base64!!!",
      decodedIntent: goldenIntent(),
      expected: goldenExpected(),
    });
    expect(result.ok).toBe(false);
  });

  it("rejects a base64 string that does not decode to a legacy message", async () => {
    const result = await verifyDepositMessage({
      messageB64: b64dec.decode(new Uint8Array([1, 2, 3, 4])),
      decodedIntent: goldenIntent(),
      expected: goldenExpected(),
    });
    expect(result.ok).toBe(false);
  });
});

describe("verifyDepositMessage — tamper negatives (ok:false, never throw)", () => {
  it("rejects when decoded_intent.amount disagrees with the message", async () => {
    const intent = goldenIntent();
    intent.amount = "9999"; // message still says 2625
    const result = await verifyDepositMessage({
      messageB64: goldenMessageB64(),
      // expected.amountAtomic stays at the (tampered) intent value so we isolate
      // the message-vs-intent mismatch.
      decodedIntent: intent,
      expected: { ...goldenExpected(), amountAtomic: "9999" },
    });
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.reason.toLowerCase()).toContain("amount");
  });

  it("rejects when user-expected amount disagrees with the message", async () => {
    const result = await verifyDepositMessage({
      messageB64: goldenMessageB64(),
      decodedIntent: goldenIntent(),
      // The intent matches the message (2625) but the USER asked for a different
      // amount — the wallet must not sign the wrong number.
      expected: { ...goldenExpected(), amountAtomic: "5000" },
    });
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.reason.toLowerCase()).toContain("amount");
  });

  it("rejects when the connected wallet differs (re-derived PDA mismatch)", async () => {
    // A different connected pubkey re-derives a DIFFERENT escrow PDA, so the
    // message's escrow account no longer belongs to this user. This is the core
    // fund-safety check.
    const otherWallet = "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM";
    const result = await verifyDepositMessage({
      messageB64: goldenMessageB64(),
      decodedIntent: goldenIntent(),
      expected: { ...goldenExpected(), agentWallet: otherWallet },
    });
    expect(result.ok).toBe(false);
  });

  it("rejects a tampered decoded_intent.escrow_pda", async () => {
    const intent = goldenIntent();
    intent.escrow_pda = VAULT_ATA; // plausible-looking but wrong PDA
    const result = await verifyDepositMessage({
      messageB64: goldenMessageB64(),
      decodedIntent: intent,
      expected: goldenExpected(),
    });
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.reason.toLowerCase()).toContain("escrow");
  });

  it("rejects a tampered decoded_intent.vault_ata", async () => {
    const intent = goldenIntent();
    intent.vault_ata = ESCROW_PDA; // wrong vault
    const result = await verifyDepositMessage({
      messageB64: goldenMessageB64(),
      decodedIntent: intent,
      expected: goldenExpected(),
    });
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.reason.toLowerCase()).toContain("vault");
  });

  it("rejects a blockhash mismatch (intent vs message lifetimeToken)", async () => {
    const intent = goldenIntent();
    intent.recent_blockhash = "11111111111111111111111111111111";
    const result = await verifyDepositMessage({
      messageB64: goldenMessageB64(),
      decodedIntent: intent,
      expected: goldenExpected(),
    });
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.reason.toLowerCase()).toContain("blockhash");
  });

  it("rejects a service_id mismatch", async () => {
    const otherSid = b64dec.decode(new Uint8Array(32).fill(8));
    const intent = goldenIntent();
    intent.service_id = otherSid;
    const result = await verifyDepositMessage({
      messageB64: goldenMessageB64(),
      decodedIntent: intent,
      expected: { ...goldenExpected(), serviceIdB64: otherSid },
    });
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.reason.toLowerCase()).toContain("service");
  });

  it("rejects a non-deposit discriminator (first ix byte flipped)", async () => {
    const tampered = mutatedMessageB64((bytes) => {
      const off = ixDataOffset(bytes);
      bytes[off] = bytes[off] ^ 0xff; // corrupt discriminator byte 0
      return bytes;
    });
    const result = await verifyDepositMessage({
      messageB64: tampered,
      decodedIntent: goldenIntent(),
      expected: goldenExpected(),
    });
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.reason.toLowerCase()).toContain("deposit");
  });

  it("rejects a message with the in-band amount bytes tampered", async () => {
    // Flip a byte in the u64 amount region [8..16] of the ix data. The decoded
    // amount changes, disagreeing with both intent and expected.
    const tampered = mutatedMessageB64((bytes) => {
      const off = ixDataOffset(bytes);
      bytes[off + 8] = bytes[off + 8] ^ 0xff;
      return bytes;
    });
    const result = await verifyDepositMessage({
      messageB64: tampered,
      decodedIntent: goldenIntent(),
      expected: goldenExpected(),
    });
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.reason.toLowerCase()).toContain("amount");
  });

  it("rejects an expiry mismatch (intent expiry ≠ in-band expiry)", async () => {
    const intent = goldenIntent();
    intent.expiry_slot = EXPIRY + 1; // intent claims a different expiry
    const result = await verifyDepositMessage({
      messageB64: goldenMessageB64(),
      decodedIntent: intent,
      expected: goldenExpected(),
    });
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.reason.toLowerCase()).toContain("expiry");
  });

  it("rejects a 2-instruction message", async () => {
    // Duplicate the single instruction so the message carries two — a deposit
    // message must contain EXACTLY one instruction.
    const tampered = mutatedMessageB64((bytes) => {
      const dec = decodeLegacy(bytes);
      const ix = dec.instructions[0];
      // The compiled instructions vector ends the message. It is prefixed by a
      // compact-u16 count (1) just before the single instruction. Rebuild the
      // message tail with two copies and count=2.
      const off = ixDataOffset(bytes);
      const accountIndices = ix.accountIndices ?? [];
      const ixData = ix.data ?? new Uint8Array(0);
      // The instruction structure right before ix.data: [programIdIndex(1)]
      // [accounts compact-len(1)][accountIndices(9)][data compact-len(1)][data(56)]
      // Reconstruct the full single-instruction blob: start = off - (1+1+9+1).
      const ixStart = off - (1 + 1 + accountIndices.length + 1);
      const instrBlob = bytes.subarray(ixStart, off + ixData.length);
      // Everything before the instructions count compact-u16 (1 byte = 0x01).
      const head = bytes.subarray(0, ixStart - 1);
      const out = new Uint8Array(head.length + 1 + instrBlob.length * 2);
      out.set(head, 0);
      out[head.length] = 0x02; // instruction count = 2
      out.set(instrBlob, head.length + 1);
      out.set(instrBlob, head.length + 1 + instrBlob.length);
      return out;
    });
    const result = await verifyDepositMessage({
      messageB64: tampered,
      decodedIntent: goldenIntent(),
      expected: goldenExpected(),
    });
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.reason.toLowerCase()).toContain("instruction");
  });
});

describe("verifyDepositMessage — pinned program/mint + header (V-2/V-3/V-4)", () => {
  // A valid base58 pubkey that is NOT the pinned escrow program / mint.
  const OTHER_PROGRAM = "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM";
  const OTHER_MINT = "4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU"; // devnet USDC

  it("rejects an intent program_id that differs from the pinned escrow program", async () => {
    // The message slot 9 still names the real program, but a hostile gateway
    // swapped the intent's program_id. Pinning rejects on the intent field even
    // though the message ↔ message checks would pass.
    const intent = goldenIntent();
    intent.program_id = OTHER_PROGRAM;
    const result = await verifyDepositMessage({
      messageB64: goldenMessageB64(),
      decodedIntent: intent,
      expected: goldenExpected(),
    });
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.reason.toLowerCase()).toContain("program");
  });

  it("rejects an intent usdc_mint that differs from the pinned mint", async () => {
    const intent = goldenIntent();
    intent.usdc_mint = OTHER_MINT;
    const result = await verifyDepositMessage({
      messageB64: goldenMessageB64(),
      decodedIntent: intent,
      expected: goldenExpected(),
    });
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.reason.toLowerCase()).toContain("mint");
  });

  it("rejects a message whose static account 9 (program) is overwritten", async () => {
    // Overwrite the 32 bytes at static-account index 9 with a different valid
    // pubkey: offset = 3 (header) + 1 (count) + 9*32. staticAccounts[9] then
    // fails the pin even though the intent still names the real program.
    const off9 = 3 + 1 + 9 * 32;
    const replacement = pubkeyBytes(OTHER_PROGRAM);
    const tampered = mutatedMessageB64((bytes) => {
      bytes.set(replacement, off9);
      return bytes;
    });
    const result = await verifyDepositMessage({
      messageB64: tampered,
      decodedIntent: goldenIntent(),
      expected: goldenExpected(),
    });
    expect(result.ok).toBe(false);
    if (!result.ok) expect(result.reason.toLowerCase()).toContain("program");
  });

  it("rejects a message with a tampered legacy header (readonly-non-signer count)", async () => {
    // Flip message byte[2] (numReadonlyNonSignerAccounts) from 6 to 4.
    const tampered = mutatedMessageB64((bytes) => {
      bytes[2] = 4;
      return bytes;
    });
    const result = await verifyDepositMessage({
      messageB64: tampered,
      decodedIntent: goldenIntent(),
      expected: goldenExpected(),
    });
    expect(result.ok).toBe(false);
  });
});
