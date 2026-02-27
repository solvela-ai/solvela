/**
 * Minimal wallet abstraction for Solana key management.
 *
 * In a full implementation this would use @solana/web3.js Keypair to derive
 * the public address from the private key and sign transactions. The
 * @solana/web3.js dependency is declared as an optional peerDependency so
 * users who only need the HTTP client without on-chain signing can skip it.
 */
export class Wallet {
  private privateKey?: string;

  /**
   * @param privateKey - Base58-encoded Solana private key.
   *   Falls back to SOLANA_WALLET_KEY environment variable.
   */
  constructor(privateKey?: string) {
    this.privateKey = privateKey || process.env.SOLANA_WALLET_KEY;
  }

  /** Whether a private key is available for signing. */
  get hasKey(): boolean {
    return !!this.privateKey;
  }

  /**
   * Returns the Solana public address derived from the private key.
   * Requires @solana/web3.js to be installed.
   * Returns null if no private key is set or if @solana/web3.js is unavailable.
   */
  get address(): string | null {
    if (!this.privateKey) return null;

    try {
      // Dynamic import to keep @solana/web3.js optional
      // eslint-disable-next-line @typescript-eslint/no-var-requires
      const { Keypair } = require('@solana/web3.js');
      const bs58 = require('bs58');
      const secretKey = bs58.decode(this.privateKey);
      const keypair = Keypair.fromSecretKey(secretKey);
      return keypair.publicKey.toBase58();
    } catch {
      // @solana/web3.js not installed or key format invalid
      return null;
    }
  }

  /**
   * Returns a redacted representation of the private key for debugging.
   * Shows first 4 and last 4 characters only.
   */
  get redactedKey(): string | null {
    if (!this.privateKey) return null;
    if (this.privateKey.length <= 8) return '****';
    return `${this.privateKey.slice(0, 4)}...${this.privateKey.slice(-4)}`;
  }
}
