import {
  Connection,
  Message,
  PublicKey,
  type CompiledInstruction,
} from '@solana/web3.js';
import {
  getAssociatedTokenAddress,
  createTransferCheckedInstruction,
} from '@solana/spl-token';
import bs58 from 'bs58';

import { PaymentAccept, PaymentPayload, Resource, SolanaPayload } from './types.js';
import { SignerError } from './errors.js';
import { Wallet } from './wallet.js';
import { USDC_MINT, X402_VERSION } from './constants.js';

/**
 * USDC has 6 decimals. The SPL `TransferChecked` instruction carries this byte
 * so the gateway verifier can confirm the mint + decimals on-chain. Mirrors the
 * Rust SDK's `const USDC_DECIMALS: u8 = 6`
 * (`sdks/rust/crates/solvela-client/src/signer.rs`) and the Python SDK's
 * `USDC_DECIMALS = 6` (`sdks/python/src/solvela/signer.py`).
 */
export const USDC_DECIMALS = 6;

export interface Signer {
  signPayment(
    amountAtomic: number,
    recipient: string,
    resource: Resource,
    accepted: PaymentAccept,
  ): Promise<PaymentPayload>;
}

export class KeypairSigner implements Signer {
  constructor(
    private readonly wallet: Wallet,
    private readonly rpcUrl: string = 'https://api.mainnet-beta.solana.com',
  ) {}

  /**
   * Build and sign a payment transaction, branching on the selected scheme.
   *
   * `exact`  -> SPL-token `TransferChecked` to the gateway's pay_to ATA
   *             (returns a `SolanaPayload`).
   *
   * There is NO silent fallback: an `escrow`-selected payment is rejected with
   * a clear `SignerError` rather than being settled as an `exact` transfer.
   * Silently routing an escrow-selected payment as an exact transfer is the
   * scheme-mismatch money-path bug the `solvela-x402` skill warns against (it
   * is the exact bug that existed in the Go/Python signers). The TS SDK does
   * not yet implement the escrow deposit builder; until it does, the signer
   * fails closed rather than producing a wrong-scheme transfer. An unknown
   * scheme is likewise rejected, never default-routed.
   */
  async signPayment(
    amountAtomic: number,
    recipient: string,
    resource: Resource,
    accepted: PaymentAccept,
  ): Promise<PaymentPayload> {
    if (accepted.scheme === 'exact') {
      return this.signExactPayment(amountAtomic, recipient, resource, accepted);
    }
    if (accepted.scheme === 'escrow') {
      // No silent fallback: escrow was selected but the TS SDK cannot build an
      // escrow deposit yet. Refuse rather than settle an exact transfer.
      throw new SignerError(
        'escrow payment scheme is not yet supported by the TypeScript SDK; ' +
          'refusing to silently fall back to an exact transfer',
      );
    }
    // `accepted.scheme` is a closed union (`PaymentAccept.fromJSON` rejects
    // unknown wire schemes), so this is only reachable if the type is bypassed.
    // Fail closed rather than default-route.
    throw new SignerError(
      `Unsupported payment scheme: ${JSON.stringify(accepted.scheme).slice(0, 64)}`,
    );
  }

  /**
   * Build and sign a USDC-SPL `TransferChecked` transaction (`exact` scheme).
   *
   * Fetches a recent blockhash via JSON-RPC, then delegates the byte-exact
   * construction to {@link buildExactTransferTx}, which pins the canonical
   * `TransferChecked` (instruction discriminator 12) wire layout the live
   * gateway `exact` verifier accepts. The gateway rejects a plain SPL
   * `Transfer` (discriminator 3) because it cannot verify the USDC mint
   * on-chain from it; `TransferChecked` carries the mint + decimals so the
   * verifier can (see `crates/x402/src/solana.rs`).
   */
  private async signExactPayment(
    amountAtomic: number,
    recipient: string,
    resource: Resource,
    accepted: PaymentAccept,
  ): Promise<PaymentPayload> {
    // Fail closed on a bad amount BEFORE any RPC/network call (mirrors the
    // Python/Rust signers). Atomic USDC is an integer count; a float would be
    // silently truncated into the on-chain u64 and mis-charge the agent.
    this.assertValidAmount(amountAtomic);

    try {
      const connection = new Connection(this.rpcUrl);
      const { blockhash } = await connection.getLatestBlockhash('finalized');

      const txB64 = await this.buildExactTransferTx(amountAtomic, recipient, blockhash);

      return new PaymentPayload(
        X402_VERSION,
        resource,
        accepted,
        new SolanaPayload(txB64),
      );
    } catch (e) {
      if (e instanceof SignerError) throw e;
      throw new SignerError(`Failed to sign payment: ${(e as Error).message}`);
    }
  }

  /**
   * Reject a non-integer, non-positive, or precision-unsafe atomic amount.
   *
   * JavaScript numbers are IEEE-754 doubles: only integers up to
   * `Number.MAX_SAFE_INTEGER` (2^53 - 1) round-trip faithfully. An amount above
   * that cannot be represented exactly as a `u64` and would silently mis-encode
   * the transfer, so it is rejected here rather than producing a wrong on-chain
   * amount. A non-integer (float) amount would be silently truncated by the
   * instruction builder. Fail closed at the boundary — parity with the Python
   * `_sign_exact_payment` guard and the Rust `build_exact_transfer_tx` zero check.
   */
  private assertValidAmount(amountAtomic: number): void {
    if (!Number.isInteger(amountAtomic)) {
      throw new SignerError(
        'exact transfer amount must be an integer number of atomic USDC units',
      );
    }
    if (amountAtomic <= 0) {
      throw new SignerError('exact transfer amount must be greater than zero');
    }
    if (amountAtomic > Number.MAX_SAFE_INTEGER) {
      throw new SignerError(
        'exact transfer amount exceeds the safe-integer range (cannot encode as u64)',
      );
    }
  }

  /**
   * Build and sign a USDC-SPL `TransferChecked` (discriminator 12) tx against a
   * caller-supplied `recentBlockhash`.
   *
   * Pure with respect to the network (no I/O): for fixed inputs it always
   * produces the same bytes, which is what lets `EXACT_GOLDEN_VECTOR_B64`
   * (`sdks/rust/crates/solvela-client/src/signer.rs`) pin the wire layout. The
   * agent wallet is the transaction fee payer (`account_keys[0]`) and the sole
   * signer.
   *
   * Instruction accounts, in canonical SPL `TransferChecked` order:
   * `[source_ata, mint, dest_ata, authority]` where
   * `source_ata = ATA(agent, USDC_MINT)` and `dest_ata = ATA(recipient, USDC_MINT)`;
   * instruction data is `[12] || amount(u64 LE) || decimals(=6)`.
   */
  async buildExactTransferTx(
    amountAtomic: number,
    recipient: string,
    blockhash: string,
  ): Promise<string> {
    const mint = new PublicKey(USDC_MINT);
    const sender = this.wallet.publicKey();
    const recipientPubkey = new PublicKey(recipient);

    const senderAta = await getAssociatedTokenAddress(mint, sender);
    const recipientAta = await getAssociatedTokenAddress(mint, recipientPubkey);

    // SPL Token TransferChecked: createTransferCheckedInstruction emits the
    // canonical 4-account layout [source, mint, destination, owner] and the
    // instruction data [12] || amount(u64 LE) || decimals(1). The mint + the
    // decimals byte are what let the gateway verify the USDC mint on-chain; the
    // live verifier rejects the plain Transfer (discriminator 3) the old code
    // emitted (crates/x402/src/solana.rs).
    const ix = createTransferCheckedInstruction(
      senderAta,
      mint,
      recipientAta,
      sender,
      amountAtomic,
      USDC_DECIMALS,
    );

    // Compile the message with the SAME account ordering the Solana Rust SDK's
    // `Message::new` produces, so the bytes match the cross-SDK golden vector
    // (EXACT_GOLDEN_VECTOR_B64 in sdks/rust/crates/solvela-client/src/signer.rs)
    // and the Python SDK byte-for-byte. solana-web3.js's `Transaction.add(ix)`
    // compiler preserves instruction-appearance order WITHIN each writability
    // partition, whereas the Rust SDK's `CompiledKeys` sorts each partition by
    // pubkey bytes. For TransferChecked that flips the two readonly accounts
    // (token-program `06dd…` sorts before USDC mint `c6fa…`), producing a
    // different — though semantically equivalent — wire layout the golden vector
    // would reject. We replicate the Rust sort here. The agent wallet is the fee
    // payer (account_keys[0]) and sole signer.
    const message = compileCanonicalMessage(sender, blockhash, [
      { programId: ix.programId, keys: ix.keys, data: ix.data },
    ]);

    // Assemble the wire transaction manually: `compact-u16(1) || sig(64) || msg`.
    // We do NOT use `Transaction.sign()` / `Transaction.serialize()` here:
    // solana-web3.js re-compiles (and re-orders) the message at sign/serialize
    // time, which discards our canonical account ordering and breaks
    // byte-for-byte parity with the cross-SDK golden vector. Instead we sign the
    // canonical message bytes directly and prepend the single signature.
    const messageBytes = message.serialize();
    const signature = this.wallet.signMessage(messageBytes);
    if (signature.length !== SIGNATURE_LENGTH) {
      throw new SignerError(
        `unexpected signature length ${signature.length} (want ${SIGNATURE_LENGTH})`,
      );
    }
    // The agent is the sole signer; the signature count fits in a single
    // compact-u16 byte (1 <= 127). Mirrors the escrow wire-format assembly.
    const wire = Buffer.concat([
      Buffer.from([1]),
      Buffer.from(signature),
      Buffer.from(messageBytes),
    ]);

    return wire.toString('base64');
  }
}

const SIGNATURE_LENGTH = 64;

interface CompilableInstruction {
  programId: PublicKey;
  keys: { pubkey: PublicKey; isSigner: boolean; isWritable: boolean }[];
  data: Buffer;
}

/**
 * Compile a legacy Solana `Message` with account ordering identical to the
 * Solana Rust SDK's `Message::new` / `CompiledKeys` (the canonical layout the
 * gateway verifier and the cross-SDK golden vectors are pinned against).
 *
 * Ordering rules (matching `CompiledKeys::try_into_message_components`):
 *   1. The fee payer is always `account_keys[0]` (writable signer).
 *   2. Remaining accounts are partitioned into:
 *        writable-signers, readonly-signers, writable-non-signers,
 *        readonly-non-signers
 *      and, WITHIN each partition, sorted ascending by raw pubkey bytes.
 *   3. Program IDs are folded in as readonly-non-signers.
 *
 * solana-web3.js's own `Transaction.compileMessage` does NOT sort within a
 * partition (it preserves first-seen order), which is why we compile manually.
 */
function compileCanonicalMessage(
  feePayer: PublicKey,
  recentBlockhash: string,
  instructions: CompilableInstruction[],
): Message {
  interface KeyMeta {
    pubkey: PublicKey;
    isSigner: boolean;
    isWritable: boolean;
  }

  // Collect every account + program id, OR-ing the signer/writable flags.
  const metaByKey = new Map<string, KeyMeta>();
  const upsert = (pubkey: PublicKey, isSigner: boolean, isWritable: boolean): void => {
    const k = pubkey.toBase58();
    const existing = metaByKey.get(k);
    if (existing) {
      existing.isSigner = existing.isSigner || isSigner;
      existing.isWritable = existing.isWritable || isWritable;
    } else {
      metaByKey.set(k, { pubkey, isSigner, isWritable });
    }
  };

  // Fee payer first — forced writable signer.
  upsert(feePayer, true, true);
  for (const ix of instructions) {
    for (const key of ix.keys) {
      upsert(key.pubkey, key.isSigner, key.isWritable);
    }
    // Program ids are readonly, non-signer.
    upsert(ix.programId, false, false);
  }

  const byBytes = (a: KeyMeta, b: KeyMeta): number =>
    Buffer.compare(a.pubkey.toBuffer(), b.pubkey.toBuffer());

  const all = [...metaByKey.values()];
  // The fee payer stays pinned at index 0; everything else is partitioned and
  // byte-sorted within its partition (matching the Rust SDK).
  const feePayerKey = feePayer.toBase58();
  const rest = all.filter((m) => m.pubkey.toBase58() !== feePayerKey);

  const writableSigners = rest.filter((m) => m.isSigner && m.isWritable).sort(byBytes);
  const readonlySigners = rest.filter((m) => m.isSigner && !m.isWritable).sort(byBytes);
  const writableNonSigners = rest.filter((m) => !m.isSigner && m.isWritable).sort(byBytes);
  const readonlyNonSigners = rest.filter((m) => !m.isSigner && !m.isWritable).sort(byBytes);

  const feePayerMeta = all.find((m) => m.pubkey.toBase58() === feePayerKey);
  if (!feePayerMeta) {
    throw new SignerError('internal: fee payer missing from compiled account set');
  }

  const ordered: KeyMeta[] = [
    feePayerMeta,
    ...writableSigners,
    ...readonlySigners,
    ...writableNonSigners,
    ...readonlyNonSigners,
  ];

  const accountKeys = ordered.map((m) => m.pubkey);
  const indexOf = new Map<string, number>();
  accountKeys.forEach((k, i) => indexOf.set(k.toBase58(), i));

  const numRequiredSignatures = ordered.filter((m) => m.isSigner).length;
  const numReadonlySignedAccounts = ordered.filter((m) => m.isSigner && !m.isWritable).length;
  const numReadonlyUnsignedAccounts = ordered.filter((m) => !m.isSigner && !m.isWritable).length;

  const compiledInstructions: CompiledInstruction[] = instructions.map((ix) => {
    const programIdIndex = indexOf.get(ix.programId.toBase58());
    if (programIdIndex === undefined) {
      throw new SignerError('internal: program id missing from compiled account set');
    }
    const accounts = ix.keys.map((key) => {
      const idx = indexOf.get(key.pubkey.toBase58());
      if (idx === undefined) {
        throw new SignerError('internal: instruction account missing from compiled set');
      }
      return idx;
    });
    return {
      programIdIndex,
      accounts,
      // CompiledInstruction.data is base58-encoded in the legacy Message form.
      data: bs58.encode(ix.data),
    };
  });

  return new Message({
    header: {
      numRequiredSignatures,
      numReadonlySignedAccounts,
      numReadonlyUnsignedAccounts,
    },
    accountKeys: accountKeys.map((k) => k.toBase58()),
    recentBlockhash,
    instructions: compiledInstructions,
  });
}
