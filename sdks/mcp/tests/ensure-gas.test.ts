/**
 * Tests for the best-effort gas top-up (src/ensure-gas.ts).
 *
 * The faucet drips a dust of SOL to USDC-funded wallets so the agent can pay
 * its own network gas. ensureGas is best-effort: it must NEVER throw and must
 * swallow every failure (disabled/unreachable/declined), only ever surfacing a
 * result object for these tests.
 *
 * Harness: Node's built-in test runner (node --test), matching the rest of the
 * MCP suite. RPC + faucet HTTP are injected mocks — no network.
 */

import { describe, it } from 'node:test';
import assert from 'node:assert/strict';

import type { TransactionConfirmationStatus } from '@solana/web3.js';

import { ensureGas } from '../src/ensure-gas.ts';

// A real, valid base58 Solana pubkey (PublicKey() must accept it).
const ADDRESS = '9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM';

/** Minimal Connection mock satisfying the Pick used by ensureGas. */
function mockConnection(balance: number, status?: TransactionConfirmationStatus) {
  return {
    async getBalance() {
      return balance;
    },
    async getSignatureStatus() {
      // Mirror web3.js `RpcResponseAndContext<SignatureStatus | null>`: the
      // `context` wrapper plus a full SignatureStatus when a status is given.
      return {
        context: { slot: 0 },
        value: status
          ? { slot: 0, confirmations: null, err: null, confirmationStatus: status }
          : null,
      };
    },
  } as const;
}

/** A fetch mock returning the given faucet JSON body. */
function mockFetch(body: unknown, ok = true): typeof fetch {
  return (async () =>
    ({
      ok,
      async json() {
        return body;
      },
    }) as unknown as Response) as unknown as typeof fetch;
}

describe('ensureGas', () => {
  it('skips the faucet when the balance is already sufficient', async () => {
    let faucetCalled = false;
    const res = await ensureGas({
      address: ADDRESS,
      gatewayUrl: 'https://gw.example',
      rpcUrl: 'https://rpc.example',
      connection: mockConnection(50_000_000), // well above threshold
      fetchImpl: (async () => {
        faucetCalled = true;
        return {} as Response;
      }) as unknown as typeof fetch,
    });
    assert.deepEqual(res, { action: 'skipped', reason: 'sufficient_balance' });
    assert.equal(faucetCalled, false, 'must not call the faucet when funded');
  });

  it('requests + confirms a drip when below threshold', async () => {
    const res = await ensureGas({
      address: ADDRESS,
      gatewayUrl: 'https://gw.example',
      rpcUrl: 'https://rpc.example',
      connection: mockConnection(0, 'confirmed'),
      fetchImpl: mockFetch({ funded: true, tx_signature: 'DripSig123', lamports: 10_000_000 }),
    });
    assert.equal(res.action, 'requested');
    assert.equal((res as { funded: boolean }).funded, true);
    assert.equal((res as { txSignature?: string }).txSignature, 'DripSig123');
  });

  it('does not throw and reports decline when the faucet declines', async () => {
    const res = await ensureGas({
      address: ADDRESS,
      gatewayUrl: 'https://gw.example',
      rpcUrl: 'https://rpc.example',
      connection: mockConnection(0),
      fetchImpl: mockFetch({ funded: false, reason: 'insufficient_usdc' }),
    });
    assert.equal(res.action, 'requested');
    assert.equal((res as { funded: boolean }).funded, false);
    assert.equal((res as { reason?: string }).reason, 'insufficient_usdc');
  });

  it('swallows a faucet network error (never throws)', async () => {
    const res = await ensureGas({
      address: ADDRESS,
      gatewayUrl: 'https://gw.example',
      rpcUrl: 'https://rpc.example',
      connection: mockConnection(0),
      fetchImpl: (async () => {
        throw new Error('ECONNREFUSED');
      }) as unknown as typeof fetch,
    });
    assert.equal(res.action, 'error');
    assert.match((res as { reason: string }).reason, /ECONNREFUSED/);
  });

  it('swallows a balance-read error (never throws)', async () => {
    const res = await ensureGas({
      address: ADDRESS,
      gatewayUrl: 'https://gw.example',
      rpcUrl: 'https://rpc.example',
      connection: {
        async getBalance() {
          throw new Error('rpc down');
        },
        async getSignatureStatus() {
          return { context: { slot: 0 }, value: null };
        },
      },
      fetchImpl: mockFetch({ funded: true }),
    });
    assert.equal(res.action, 'error');
    assert.match((res as { reason: string }).reason, /rpc down/);
  });

  it('treats a malformed faucet response as a decline (untrusted external data)', async () => {
    const res = await ensureGas({
      address: ADDRESS,
      gatewayUrl: 'https://gw.example',
      rpcUrl: 'https://rpc.example',
      connection: mockConnection(0),
      // Garbage body — no funded field.
      fetchImpl: mockFetch({ unexpected: 'shape' }),
    });
    assert.equal(res.action, 'requested');
    assert.equal((res as { funded: boolean }).funded, false);
  });
});
