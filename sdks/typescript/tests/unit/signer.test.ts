import { describe, it, expect } from 'vitest';
import { KeypairSigner } from '../../src/signer.js';
import { Wallet } from '../../src/wallet.js';

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
});
