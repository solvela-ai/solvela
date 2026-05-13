import { describe, it, expect } from 'vitest';
import { SolvelaClient } from '../../src/client.js';
import { Wallet } from '../../src/wallet.js';

describe('SolvelaClient', () => {
  it('creates without wallet', () => {
    const client = new SolvelaClient();
    expect(client).toBeDefined();
  });

  it('creates with wallet', () => {
    const [wallet] = Wallet.create();
    const client = new SolvelaClient({ wallet });
    expect(client).toBeDefined();
  });

  it('lastKnownBalance is undefined initially', () => {
    const client = new SolvelaClient();
    expect(client.lastKnownBalance()).toBeUndefined();
  });

  it('creates with custom config', () => {
    const client = new SolvelaClient({
      config: {
        gatewayUrl: 'https://custom.example.com',
        timeout: 60,
        enableCache: true,
      },
    });
    expect(client).toBeDefined();
  });

  it('toString redacts sensitive info', () => {
    const [wallet] = Wallet.create();
    const client = new SolvelaClient({ wallet });
    const str = client.toString();
    expect(str).toContain('SolvelaClient');
    expect(str).not.toContain(wallet.toKeypairB58());
  });
});
