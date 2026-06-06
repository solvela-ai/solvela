import { randomBytes, createHash } from 'node:crypto';

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

import {
  PaymentAccept,
  PaymentPayload,
  Resource,
  SolanaPayload,
  EscrowPayload,
} from './types.js';
import { SignerError } from './errors.js';
import { Wallet } from './wallet.js';
import { USDC_MINT, X402_VERSION } from './constants.js';
import { buildDepositTx } from './escrow.js';

/**
 * USDC has 6 decimals. The SPL `TransferChecked` instruction carries this byte
 * so the gateway verifier can confirm the mint + decimals on-chain. Mirrors the
 * Rust SDK's `const USDC_DECIMALS: u8 = 6`
 * (`sdks/rust/crates/solvela-client/src/signer.rs`) and the Python SDK's
 * `USDC_DECIMALS = 6` (`sdks/python/src/solvela/signer.py`).
 */
export const USDC_DECIMALS = 6;

// --- Escrow expiry-slot bounds (mirror the Rust/Go/Python SDK signers) ---
//
// Solana slots are ~400 ms. These bounds are ported verbatim from
// sdks/rust/crates/solvela-client/src/signer.rs (and sdks/go/signer.go,
// sdks/python/.../signer.py) so all SDKs choose compatible expiry slots and
// none is bounced by the gateway.

/**
 * Upper bound on how far ahead an escrow may expire (~66 min). Rejects a
 * malicious/misconfigured gateway from pushing expiry to "never".
 */
const MAX_ESCROW_EXPIRY_SLOTS_AHEAD = 10_000;

/**
 * Lower bound on expiry distance. Mirrors AND exceeds the gateway's
 * MIN_EXPIRY_BUFFER_SLOTS = 50 (crates/x402/src/escrow/verifier.rs); the extra
 * headroom (150 vs 50) absorbs slot-skew between the slot we read and the slot
 * the gateway verifies against. ~150 slots ~= 60 s.
 */
const MIN_ESCROW_EXPIRY_SLOTS_AHEAD = 150;

/**
 * Floor below which a getSlot result is implausible and rejected. A stub /
 * genesis / freshly-reset validator can answer getSlot with a near-zero slot;
 * computing an escrow expiry from that base yields an already-expired (or
 * trivially-near-expiry) deposit the gateway would bounce. Mainnet/devnet have
 * been well past this for years. Fail closed rather than silently signing a
 * dead-on-arrival escrow deposit.
 */
const MIN_PLAUSIBLE_SLOT = 1_000_000;

const BLOCKHASH_LENGTH = 32;

/**
 * RPC request timeout (ms). A bare `fetch()` has no timeout, so a stalled RPC
 * endpoint would hang the payment path forever. Both the Go SDK
 * (`http.Client{Timeout: 30s}`) and the Python SDK (`httpx ... timeout=30`)
 * enforce this; mirror it with an `AbortController` + `setTimeout`, the same
 * pattern `src/transport.ts` uses.
 */
const RPC_TIMEOUT_MS = 30_000;

/**
 * Cap on a successful (200 OK) JSON-RPC response body, in bytes. The
 * getSlot/getLatestBlockhash responses this signer reads are tiny (a few hundred
 * bytes), but we still bound the read so a misbehaving/hostile RPC endpoint
 * cannot stream an unbounded body into memory and OOM the agent. Mirrors the Go
 * SDK's `maxRPCBodyBytes = 64 << 10` (`io.LimitReader`).
 */
const MAX_RPC_BODY_BYTES = 65_536;

/**
 * Render a parenthesized, message-free description of a JSON-RPC error object,
 * suitable for appending to a `SignerError`. It surfaces only the numeric code
 * (never the RPC message text — GHSA-cgqx-mg48-949v), and clearly distinguishes
 * an unparseable / code-less error from a real code. Mirrors the Go SDK's
 * `rpcErrorCodeSuffix`.
 */
function rpcErrorCodeSuffix(rawErr: unknown): string {
  if (rawErr !== null && typeof rawErr === 'object' && 'code' in rawErr) {
    const code = (rawErr as { code: unknown }).code;
    if (typeof code === 'number' && Number.isInteger(code)) {
      return `(code: ${code})`;
    }
  }
  return '(unparseable error object, no numeric code)';
}

/**
 * Compute the slot at which a now-created escrow PDA should expire.
 *
 * Ported from the Rust SDK's `escrow_expiry_slot`: ~2.5 slots/s (1000 ms /
 * 400 ms), clamped into `[MIN_ESCROW_EXPIRY_SLOTS_AHEAD,
 * MAX_ESCROW_EXPIRY_SLOTS_AHEAD]`. The floor mirrors and exceeds the gateway's
 * MIN_EXPIRY_BUFFER_SLOTS so a too-near expiry is never bounced; the cap rejects
 * an unbounded-future expiry. A negative timeout is floored to 0 (never pushes
 * expiry backwards into a dead-on-arrival deposit). `currentSlot` and the
 * effective offset stay within Number.MAX_SAFE_INTEGER for any plausible slot,
 * so the add cannot lose precision. Exported for direct unit testing.
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
   * `escrow` -> on-chain `deposit` into a per-request escrow PDA, byte-exact
   *             with the canonical builder (returns an `EscrowPayload`).
   *
   * There is NO silent fallback: an `escrow`-selected payment always produces an
   * escrow deposit, never an `exact` transfer, and an `exact`-selected payment
   * never produces an escrow deposit. Silently routing an escrow-selected
   * payment as an exact transfer is the scheme-mismatch money-path bug the
   * `solvela-x402` skill warns against (it is the exact bug that previously
   * existed in the Go/Python signers and in this TS signer before escrow
   * support landed). An unknown scheme is likewise rejected, never
   * default-routed.
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
      return this.signEscrowPayment(amountAtomic, resource, accepted);
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
      const msg = e instanceof Error ? e.message : String(e);
      throw new SignerError(`Failed to sign payment: ${msg}`);
    }
  }

  /**
   * Build and sign an escrow `deposit` transaction (`escrow` scheme).
   *
   * Generates a fresh CSPRNG `service_id`, computes the expiry slot from the
   * current slot and `accepted.maxTimeoutSeconds` (mirroring the Rust/Go/Python
   * SDKs), fetches a recent blockhash, and delegates the byte-exact transaction
   * construction to the shared, golden-vector-pinned `buildDepositTx`. There is
   * NO silent fallback to an exact transfer.
   */
  private async signEscrowPayment(
    amountAtomic: number,
    resource: Resource,
    accepted: PaymentAccept,
  ): Promise<PaymentPayload> {
    // Fail clearly if escrow was selected but no program id was offered — never
    // silently fall back to an exact transfer.
    if (!accepted.escrowProgramId) {
      throw new SignerError('escrow scheme selected but escrow_program_id is missing');
    }

    // Reject a bad amount BEFORE any RPC/network call (mirrors the exact path
    // and the Go/Python signers). A zero/negative/float/out-of-range amount
    // would otherwise be mis-encoded into the on-chain u64 and mis-charge the
    // agent.
    this.assertValidAmount(amountAtomic);

    // Validate `maxTimeoutSeconds` BEFORE any RPC call. It comes straight from
    // the untrusted gateway 402 with no parse-boundary validation; a
    // NaN/Infinity/non-integer value propagates through `escrowExpirySlot`
    // (`Math.max(NaN, 0) = NaN`) into `buildDepositTx` and surfaces as a
    // confusing wrapped error. Reject only non-finite/non-integer values here —
    // a *negative* finite integer is intentionally allowed, since
    // `escrowExpirySlot` floors it to 0 then clamps to the 150-slot floor
    // (parity with Go's `escrowExpirySlot`). This validation is scoped to the
    // escrow path only — the exact path does not consume this field, so we must
    // not start rejecting 402s over it (e.g. in `PaymentAccept.fromJSON`).
    if (!Number.isInteger(accepted.maxTimeoutSeconds)) {
      throw new SignerError(
        'maxTimeoutSeconds must be a finite integer number of seconds',
      );
    }

    try {
      // Per-request CSPRNG service_id (#118 invariant): two identical requests
      // must never share a service_id -> escrow PDA -> vault ATA. Hash 32 random
      // bytes so the result is distinct per call (mirrors the Rust/Go SDKs).
      const serviceId = new Uint8Array(createHash('sha256').update(randomBytes(32)).digest());

      const currentSlot = await this.fetchCurrentSlot();
      const expirySlot = escrowExpirySlot(currentSlot, accepted.maxTimeoutSeconds);

      const recentBlockhash = await this.fetchLatestBlockhash();

      const depositTx = buildDepositTx({
        wallet: this.wallet,
        providerWalletB58: accepted.payTo,
        usdcMintB58: USDC_MINT,
        escrowProgramIdB58: accepted.escrowProgramId,
        amount: amountAtomic,
        serviceId,
        expirySlot,
        recentBlockhash,
      });

      const payload = new EscrowPayload(
        depositTx,
        Buffer.from(serviceId).toString('base64'),
        this.wallet.address(),
      );
      return new PaymentPayload(X402_VERSION, resource, accepted, payload);
    } catch (e) {
      if (e instanceof SignerError) throw e;
      const msg = e instanceof Error ? e.message : String(e);
      throw new SignerError(`Failed to sign escrow deposit: ${msg}`);
    }
  }

  /**
   * Fetch the current slot via JSON-RPC `getSlot` (commitment confirmed).
   *
   * Fails closed: any HTTP failure, malformed body, RPC error, or
   * missing/invalid result is a `SignerError` — never a silent default slot,
   * which would let the escrow expiry be computed from a bogus base. Also
   * rejects an implausibly-low slot (stub/genesis node). Mirrors the Go/Python
   * `fetchCurrentSlot`. Only the numeric RPC error code is surfaced, never the
   * message text (which can echo node internals — GHSA-cgqx-mg48-949v).
   */
  private async fetchCurrentSlot(): Promise<number> {
    const data = await this.postRpc('getSlot', [{ commitment: 'confirmed' }], 'getSlot');
    if (data.error !== undefined && data.error !== null) {
      throw new SignerError(`getSlot RPC error ${rpcErrorCodeSuffix(data.error)}`);
    }
    const slot = data.result;
    // Use Number.isSafeInteger (not Number.isInteger): a slot above
    // Number.MAX_SAFE_INTEGER cannot be added to the expiry offset without
    // precision loss, so reject it rather than computing a bogus expiry. The
    // amount guard already uses the safe-integer bound; keep this consistent.
    if (typeof slot !== 'number' || !Number.isSafeInteger(slot) || slot < 0) {
      throw new SignerError('getSlot RPC: missing or invalid result');
    }
    if (slot < MIN_PLAUSIBLE_SLOT) {
      throw new SignerError(
        `getSlot RPC returned implausibly low slot ${slot} ` +
          `(< ${MIN_PLAUSIBLE_SLOT}); refusing to build an escrow deposit`,
      );
    }
    return slot;
  }

  /**
   * Fetch a recent blockhash via JSON-RPC `getLatestBlockhash` and decode it to
   * raw 32 bytes. Fails closed on any HTTP/JSON/RPC failure or a
   * missing/malformed blockhash, never returning a default. Mirrors the
   * Go/Python `fetchLatestBlockhash`. The builder needs the raw 32-byte
   * blockhash (NOT the base58 string), so we bs58-decode it here.
   */
  private async fetchLatestBlockhash(): Promise<Uint8Array> {
    const data = await this.postRpc(
      'getLatestBlockhash',
      [{ commitment: 'finalized' }],
      'Blockhash',
    );
    const result = data.result as { value?: { blockhash?: unknown } } | undefined;
    const blockhashStr = result?.value?.blockhash;
    if (typeof blockhashStr !== 'string' || blockhashStr.length === 0) {
      if (data.error !== undefined && data.error !== null) {
        throw new SignerError(`RPC did not return a blockhash ${rpcErrorCodeSuffix(data.error)}`);
      }
      throw new SignerError('RPC did not return a blockhash');
    }
    let raw: Uint8Array;
    try {
      raw = bs58.decode(blockhashStr);
    } catch {
      throw new SignerError('RPC returned a malformed blockhash');
    }
    if (raw.length !== BLOCKHASH_LENGTH) {
      throw new SignerError('RPC returned a malformed blockhash');
    }
    return raw;
  }

  /**
   * Issue a JSON-RPC POST and return the decoded body. HTTP-level failures,
   * non-200 status, and malformed JSON are surfaced as `SignerError` before any
   * field access — so a 429/5xx HTML body never leaks node internals into the
   * error string (only the status code is surfaced). Mirrors the Go SDK's
   * `postRPC`.
   *
   * A bare `fetch()` has no timeout; a stalled RPC would hang the payment path
   * forever, so we bound the request with an `AbortController` +
   * {@link RPC_TIMEOUT_MS} (same pattern as `src/transport.ts`, and parity with
   * the Go `http.Client{Timeout: 30s}` / Python `httpx ... timeout=30`). The
   * 200-path body is read as raw bytes and rejected if it exceeds
   * {@link MAX_RPC_BODY_BYTES} so a hostile RPC cannot OOM the agent. The cap is
   * measured in bytes (`ArrayBuffer.byteLength`) to be byte-exact with the Go
   * `io.LimitReader` cap — `String.length` counts UTF-16 code units, which
   * under-counts multi-byte UTF-8 sequences and would let an over-cap body slip
   * through. The bytes are then UTF-8 decoded and `JSON.parse`d.
   */
  private async postRpc(
    method: string,
    params: unknown[],
    label: string,
  ): Promise<{ result?: unknown; error?: unknown }> {
    const controller = new AbortController();
    const timeoutId = setTimeout(() => controller.abort(), RPC_TIMEOUT_MS);
    try {
      let resp: Response;
      try {
        resp = await fetch(this.rpcUrl, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ jsonrpc: '2.0', id: 1, method, params }),
          signal: controller.signal,
        });
      } catch (e) {
        // An abort (timeout) and a transport failure both surface here; name the
        // timeout case explicitly so a stalled RPC is diagnosable. Never echo the
        // underlying error text (it can leak node URLs/internals).
        if ((e as Error).name === 'AbortError') {
          throw new SignerError(`${label} RPC request timed out after ${RPC_TIMEOUT_MS}ms`);
        }
        throw new SignerError(`${label} RPC request failed`);
      }
      if (resp.status !== 200) {
        throw new SignerError(`${label} RPC HTTP ${resp.status}`);
      }
      // Read the success body as raw bytes under a size cap (Go's io.LimitReader
      // equivalent) BEFORE parsing, so an unbounded/hostile body cannot OOM the
      // agent. The cap is measured in bytes — not String.length (UTF-16 code
      // units) — to be byte-exact with Go's io.LimitReader. Only then UTF-8
      // decode + JSON.parse, preserving the malformed-JSON handling.
      let text: string;
      try {
        const buf = await resp.arrayBuffer();
        if (buf.byteLength > MAX_RPC_BODY_BYTES) {
          throw new SignerError(
            `${label} RPC: response body exceeds ${MAX_RPC_BODY_BYTES} bytes`,
          );
        }
        text = new TextDecoder('utf-8', { fatal: false }).decode(buf);
      } catch (e) {
        if (e instanceof SignerError) {
          throw e;
        }
        throw new SignerError(`${label} RPC: malformed JSON body`);
      }
      try {
        return JSON.parse(text) as { result?: unknown; error?: unknown };
      } catch {
        throw new SignerError(`${label} RPC: malformed JSON body`);
      }
    } finally {
      clearTimeout(timeoutId);
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
    // Validate unconditionally at the lowest level so a direct external call to
    // this public, barrel-exported builder cannot bypass the amount guard. A
    // zero/negative/float/out-of-range amount would otherwise be silently
    // mis-encoded into the u64 (e.g. a negative amount wraps to ~18.4
    // quintillion atomic USDC). The duplicate call in signExactPayment is a
    // harmless no-op once validation passes.
    this.assertValidAmount(amountAtomic);

    const mint = new PublicKey(USDC_MINT);
    const sender = this.wallet.publicKey();
    const recipientPubkey = new PublicKey(recipient);

    const senderAta = await getAssociatedTokenAddress(mint, sender);
    const recipientAta = await getAssociatedTokenAddress(mint, recipientPubkey);

    // SPL Token TransferChecked: createTransferCheckedInstruction emits the
    // canonical 4-account layout [source, mint, destination, authority] and the
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
    const wire = Buffer.concat([Buffer.from([1]), signature, messageBytes]);

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

  const feePayerMeta = metaByKey.get(feePayerKey);
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
