import { describe, it, expect } from 'vitest';
import { Wallet } from '../../src/wallet.js';
import { WalletError } from '../../src/errors.js';

describe('Wallet', () => {
  it('create() returns wallet and mnemonic', () => {
    const [wallet, mnemonic] = Wallet.create();
    expect(wallet.address()).toBeTruthy();
    expect(mnemonic.split(' ')).toHaveLength(12);
  });

  it('fromMnemonic() restores same address', () => {
    const [, mnemonic] = Wallet.create();
    const restored = Wallet.fromMnemonic(mnemonic);
    const again = Wallet.fromMnemonic(mnemonic);
    expect(restored.address()).toBe(again.address());
  });

  it('fromKeypairBytes() round-trips', () => {
    const [wallet] = Wallet.create();
    const bytes = wallet.toKeypairBytes();
    const restored = Wallet.fromKeypairBytes(bytes);
    expect(restored.address()).toBe(wallet.address());
  });

  it('fromKeypairB58() round-trips', () => {
    const [wallet] = Wallet.create();
    const b58 = wallet.toKeypairB58();
    const restored = Wallet.fromKeypairB58(b58);
    expect(restored.address()).toBe(wallet.address());
  });

  it('fromEnv() reads from environment', () => {
    const [wallet] = Wallet.create();
    const b58 = wallet.toKeypairB58();
    process.env['TEST_WALLET_KEY'] = b58;
    try {
      const envWallet = Wallet.fromEnv('TEST_WALLET_KEY');
      expect(envWallet.address()).toBe(wallet.address());
    } finally {
      delete process.env['TEST_WALLET_KEY'];
    }
  });

  it('fromEnv() throws on missing var', () => {
    expect(() => Wallet.fromEnv('NONEXISTENT_WALLET_VAR')).toThrow(WalletError);
  });

  it('toString() redacts secret', () => {
    const [wallet] = Wallet.create();
    const str = wallet.toString();
    expect(str).toContain('REDACTED');
    expect(str).toContain(wallet.address());
    expect(str).not.toContain(wallet.toKeypairB58());
  });

  it('publicKey() returns PublicKey', () => {
    const [wallet] = Wallet.create();
    const pk = wallet.publicKey();
    expect(pk.toBase58()).toBe(wallet.address());
  });

  it('signTransaction() signs a transaction with the wallet keypair', async () => {
    const { Transaction, SystemProgram, PublicKey } = await import('@solana/web3.js');
    const [wallet] = Wallet.create();
    const tx = new Transaction({
      recentBlockhash: '11111111111111111111111111111111',
      feePayer: wallet.publicKey(),
    });
    tx.add(
      SystemProgram.transfer({
        fromPubkey: wallet.publicKey(),
        toPubkey: new PublicKey('11111111111111111111111111111111'),
        lamports: 1,
      }),
    );
    const signed = wallet.signTransaction(tx);
    expect(signed.signatures.length).toBeGreaterThan(0);
    expect(signed.signatures[0].signature).not.toBeNull();
    // Public address is recoverable from signed payload — the secret never leaves the Wallet.
    expect(signed.feePayer?.toBase58()).toBe(wallet.address());
  });
});
