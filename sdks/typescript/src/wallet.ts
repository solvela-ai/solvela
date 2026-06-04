import { createPrivateKey, sign as edSign } from 'node:crypto';

import { Keypair, PublicKey, Transaction, VersionedTransaction } from '@solana/web3.js';
import * as bip39 from 'bip39';
import bs58 from 'bs58';

import { WalletError } from './errors.js';

// DER prefix for a PKCS#8-wrapped Ed25519 private key (RFC 8410). The 32-byte
// raw seed is appended to form a complete PKCS#8 key Node's crypto can import.
const ED25519_PKCS8_PREFIX = Buffer.from('302e020100300506032b657004220420', 'hex');

export class Wallet {
  private constructor(private readonly keypair: Keypair) {}

  /**
   * Sign a Solana transaction (legacy or versioned) with the wallet's keypair.
   * Prefer this over exposing the underlying Keypair through the public API —
   * callers should never need a raw secret reference.
   */
  signTransaction<T extends Transaction | VersionedTransaction>(tx: T): T {
    if (tx instanceof VersionedTransaction) {
      tx.sign([this.keypair]);
      return tx;
    }
    (tx as Transaction).sign(this.keypair);
    return tx;
  }

  /**
   * Sign raw message bytes with the wallet's Ed25519 key, returning the 64-byte
   * detached signature.
   *
   * This is needed by the `exact` payment signer, which compiles a legacy
   * Solana message with the canonical (Rust-SDK-sorted) account ordering and
   * then assembles the wire transaction manually — `Transaction.sign()` cannot
   * be used because solana-web3.js re-compiles (and re-orders) the message at
   * sign/serialize time, which would break byte-for-byte parity with the
   * cross-SDK golden vector. The secret never leaves the Wallet boundary: it is
   * imported into a Node `KeyObject` and used in-process for the signature only.
   */
  signMessage(message: Uint8Array): Uint8Array {
    const seed = this.keypair.secretKey.slice(0, 32);
    const pkcs8 = Buffer.concat([ED25519_PKCS8_PREFIX, Buffer.from(seed)]);
    const keyObject = createPrivateKey({ key: pkcs8, format: 'der', type: 'pkcs8' });
    return edSign(null, Buffer.from(message), keyObject);
  }

  static create(): [Wallet, string] {
    const mnemonic = bip39.generateMnemonic();
    const seed = bip39.mnemonicToSeedSync(mnemonic);
    const keypair = Keypair.fromSeed(seed.subarray(0, 32));
    return [new Wallet(keypair), mnemonic];
  }

  static fromMnemonic(phrase: string): Wallet {
    if (!bip39.validateMnemonic(phrase)) {
      throw new WalletError('Invalid mnemonic phrase');
    }
    const seed = bip39.mnemonicToSeedSync(phrase);
    const keypair = Keypair.fromSeed(seed.subarray(0, 32));
    return new Wallet(keypair);
  }

  static fromKeypairBytes(bytes: Uint8Array): Wallet {
    try {
      const keypair = Keypair.fromSecretKey(bytes);
      return new Wallet(keypair);
    } catch (e) {
      throw new WalletError(`Invalid keypair bytes: ${e}`);
    }
  }

  static fromKeypairB58(b58: string): Wallet {
    try {
      const bytes = bs58.decode(b58);
      return Wallet.fromKeypairBytes(bytes);
    } catch (e) {
      if (e instanceof WalletError) throw e;
      throw new WalletError(`Invalid base58 keypair: ${e}`);
    }
  }

  static fromEnv(varName: string): Wallet {
    const value = process.env[varName];
    if (!value) {
      throw new WalletError(`Environment variable ${varName} not set`);
    }
    return Wallet.fromKeypairB58(value);
  }

  address(): string {
    return this.keypair.publicKey.toBase58();
  }

  publicKey(): PublicKey {
    return this.keypair.publicKey;
  }

  /**
   * @deprecated Returns the raw secret key bytes. Only use for backup/export
   * to a secure store. Never log or transmit. Prefer `signTransaction` for
   * signing flows so the secret never leaves the Wallet boundary.
   */
  toKeypairBytes(): Uint8Array {
    return this.keypair.secretKey;
  }

  /**
   * @deprecated Returns the raw secret key in base58. Only use for
   * backup/export to a secure store. Never log or transmit. Prefer
   * `signTransaction` for signing flows.
   */
  toKeypairB58(): string {
    return bs58.encode(this.keypair.secretKey);
  }

  toString(): string {
    return `Wallet(address=${this.address()}, secret=REDACTED)`;
  }
}
