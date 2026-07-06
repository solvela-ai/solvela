/**
 * `open` / `close` command tests + the fail-closed USDC decimal→atomic parser.
 *
 * The fake gateway here also plays the Solana JSON-RPC (getLatestBlockhash),
 * so `openChannel` runs its REAL code path end-to-end — keypair load, config
 * discovery, verify-what-you-sign confirmation, funding-tx build+sign, POST
 * /v1/channel/open, state-file write — with zero mocking seams in src/.
 */

import { after, describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { createServer, type Server, type ServerResponse } from 'node:http';
import { once } from 'node:events';
import * as fs from 'node:fs/promises';
import * as os from 'node:os';
import * as path from 'node:path';
import type { AddressInfo } from 'node:net';

import bs58 from 'bs58';
import { Keypair, VersionedTransaction } from '@solana/web3.js';
import { ed25519 } from '@noble/curves/ed25519';

import { buildCloseMessage } from '@solvela/signer-core';

import { usdcDecimalToAtomic, openChannel } from '../src/open.ts';
import { closeChannel } from '../src/close.ts';
import { loadState, saveState } from '../src/state.ts';

const USDC_MINT = 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v';
const SOLANA_NETWORK = 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp';
const PROVIDER_WALLET = bs58.encode(new Uint8Array(32).fill(1));
const CHANNEL_ID_B58 = bs58.encode(new Uint8Array(32).fill(0x11));
const SESSION_SEED = new Uint8Array(32).fill(7);

// ---------------------------------------------------------------------------
// usdcDecimalToAtomic — fail-closed money conversion (solvela-fintech rules)
// ---------------------------------------------------------------------------

describe('usdcDecimalToAtomic', () => {
  it('converts valid decimals with integer math', () => {
    assert.equal(usdcDecimalToAtomic('1.5'), 1_500_000n);
    assert.equal(usdcDecimalToAtomic('0.000001'), 1n);
    assert.equal(usdcDecimalToAtomic('12'), 12_000_000n);
    assert.equal(usdcDecimalToAtomic('0.5'), 500_000n);
  });

  it('fails closed on anything else — never quotes 0, never truncates', () => {
    for (const bad of [
      '',
      ' ',
      '0', // zero funding is meaningless
      '0.0',
      '-1',
      '1.2345678', // 7 dp would silently lose value if truncated — reject
      'abc',
      '1e6',
      'NaN',
      'Infinity',
      '1.',
      '.5',
      '1,5',
      '18446744073709551616', // > u64 in atomic terms regardless
    ]) {
      assert.throws(() => usdcDecimalToAtomic(bad), `should reject: ${JSON.stringify(bad)}`);
    }
  });
});

// ---------------------------------------------------------------------------
// Fake gateway (+ fake Solana RPC on the same server)
// ---------------------------------------------------------------------------

interface Recorded {
  method: string;
  url: string;
  body: string;
}

const openServers: Server[] = [];
after(async () => {
  await Promise.all(
    openServers.map(
      (s) =>
        new Promise<void>((resolve) => {
          s.closeAllConnections(); // a hung/red stream must never wedge the run
          s.close(() => resolve());
        }),
    ),
  );
});

interface FakeOptions {
  usdcMint?: string;
  providerWallet?: string;
  onChannelOpen?: (body: unknown, res: ServerResponse) => void;
  onChannelClose?: (body: unknown, res: ServerResponse) => void;
}

async function startFake(opts: FakeOptions = {}): Promise<{ url: string; requests: Recorded[] }> {
  const { onChannelOpen, onChannelClose } = opts;
  const requests: Recorded[] = [];
  const server = createServer(async (req, res) => {
    const chunks: Buffer[] = [];
    for await (const c of req) chunks.push(c as Buffer);
    const body = Buffer.concat(chunks).toString('utf-8');
    requests.push({ method: req.method ?? '', url: req.url ?? '', body });

    if (req.method === 'GET' && req.url?.startsWith('/v1/escrow/config')) {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(
        JSON.stringify({
          escrow_program_id: '9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU',
          current_slot: 1_000_000,
          network: SOLANA_NETWORK,
          usdc_mint: opts.usdcMint ?? USDC_MINT,
          provider_wallet: opts.providerWallet ?? PROVIDER_WALLET,
        }),
      );
      return;
    }
    if (req.method === 'POST' && req.url === '/') {
      // Solana JSON-RPC: the funding-tx builder fetches a recent blockhash.
      const rpc = JSON.parse(body) as { id: number; method: string };
      assert.equal(rpc.method, 'getLatestBlockhash');
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(
        JSON.stringify({
          jsonrpc: '2.0',
          id: rpc.id,
          result: {
            context: { slot: 1_000_000 },
            value: {
              blockhash: bs58.encode(new Uint8Array(32).fill(9)),
              lastValidBlockHeight: 1_000_100,
            },
          },
        }),
      );
      return;
    }
    if (req.method === 'POST' && req.url === '/v1/channel/open' && onChannelOpen) {
      onChannelOpen(JSON.parse(body), res);
      return;
    }
    if (req.method === 'POST' && req.url === '/v1/channel/close' && onChannelClose) {
      onChannelClose(JSON.parse(body), res);
      return;
    }
    res.writeHead(404, { 'content-type': 'application/json' });
    res.end('{"error":"not found"}');
  });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  openServers.push(server);
  return { url: `http://127.0.0.1:${(server.address() as AddressInfo).port}`, requests };
}

async function tmpDir(): Promise<string> {
  return fs.mkdtemp(path.join(os.tmpdir(), 'solvela-dropin-test-'));
}

/** Write a standard Solana id.json (array of 64 bytes) and return its keypair. */
async function writeWalletFile(dir: string): Promise<{ walletPath: string; keypair: Keypair }> {
  const keypair = Keypair.generate();
  const walletPath = path.join(dir, 'id.json');
  await fs.writeFile(walletPath, JSON.stringify(Array.from(keypair.secretKey)), { mode: 0o600 });
  return { walletPath, keypair };
}

// ---------------------------------------------------------------------------
// open
// ---------------------------------------------------------------------------

describe('open command', () => {
  it('refuses without confirmation — nothing signed, nothing POSTed, no state file', async () => {
    const fake = await startFake({
      onChannelOpen: (_body, res) => {
        res.writeHead(200).end('{}');
      },
    });
    const dir = await tmpDir();
    const { walletPath } = await writeWalletFile(dir);
    const statePath = path.join(dir, 'dropin.json');

    await assert.rejects(
      openChannel({
        gateway: fake.url,
        amount: '5',
        walletPath,
        rpcUrl: fake.url,
        yes: false,
        confirm: async () => false, // user says no
        port: 8484,
        statePath,
        log: () => {},
      }),
      /confirm/i,
    );

    assert.ok(!fake.requests.some((r) => r.url === '/v1/channel/open'), 'no open POST');
    assert.ok(!fake.requests.some((r) => r.url === '/'), 'no blockhash fetch — nothing was signed');
    await assert.rejects(fs.access(statePath), 'state file must not exist');
  });

  it('opens the channel: 0600 state file, env vars printed, session seed NEVER in output', async () => {
    let openBody: Record<string, unknown> | null = null;
    const fake = await startFake({
      onChannelOpen: (body, res) => {
        openBody = body as Record<string, unknown>;
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(
          JSON.stringify({
            channel_id: CHANNEL_ID_B58,
            agent_wallet: (body as Record<string, unknown>).agent_wallet,
            session_key: (body as Record<string, unknown>).session_key,
            deposited_atomic: 5_000_000,
            settled_atomic: 0,
            last_cumulative_atomic: 0,
            status: 'open',
            funding_tx_sig: 'FAKE_SIG',
            network: SOLANA_NETWORK,
          }),
        );
      },
    });
    const dir = await tmpDir();
    const { walletPath, keypair } = await writeWalletFile(dir);
    const statePath = path.join(dir, 'dropin.json');
    const lines: string[] = [];

    await openChannel({
      gateway: fake.url,
      amount: '5',
      walletPath,
      rpcUrl: fake.url,
      yes: true,
      port: 8484,
      statePath,
      log: (l) => lines.push(l),
    });

    // Open request shape: agent_wallet + fresh session_key + signed funding tx.
    assert.ok(openBody, 'gateway received the open request');
    const ob = openBody as unknown as Record<string, unknown>;
    assert.deepEqual(
      Object.keys(ob).sort(),
      ['agent_wallet', 'funding_tx', 'session_key'],
      'open body carries exactly the fields routes/channel.rs deny_unknown_fields accepts',
    );
    assert.equal(ob.agent_wallet, keypair.publicKey.toBase58());
    const sessionKeyB58 = String(ob.session_key);
    assert.equal(bs58.decode(sessionKeyB58).length, 32);
    assert.notEqual(sessionKeyB58, keypair.publicKey.toBase58(), 'session key is delegated, not the wallet');

    // The funding tx is a real signed VersionedTransaction paid by the wallet.
    const tx = VersionedTransaction.deserialize(new Uint8Array(Buffer.from(String(ob.funding_tx), 'base64')));
    assert.equal(tx.message.staticAccountKeys[0].toBase58(), keypair.publicKey.toBase58());
    assert.equal(tx.signatures.length, 1);
    // BLOCKER regression (round-1 review): the signature must CRYPTOGRAPHICALLY
    // verify against the real wallet key. `Keypair.fromSecretKey` ALIASES its
    // input, so zeroing the loaded buffer before signing produced a structurally
    // valid tx signed by an all-zero key — structure checks alone let that ship.
    assert.ok(
      ed25519.verify(tx.signatures[0], tx.message.serialize(), keypair.publicKey.toBytes()),
      'funding tx signature must verify against the REAL wallet pubkey',
    );

    // State file: 0600, complete, matches the gateway response.
    const stat = await fs.stat(statePath);
    assert.equal(stat.mode & 0o777, 0o600, 'state file must be 0600');
    const state = await loadState(statePath);
    assert.equal(state.channel_id, CHANNEL_ID_B58);
    assert.equal(state.gateway_url, fake.url);
    assert.equal(state.local_token.length >= 32, true);
    assert.equal(state.last_cumulative, '0');
    // The stored seed's pubkey must be the session_key we registered.
    const seed = bs58.decode(state.session_seed_b58);
    assert.equal(seed.length, 32);
    assert.equal(bs58.encode(ed25519.getPublicKey(seed)), sessionKeyB58);

    // Output: the two env vars, and NEVER the seed.
    const out = lines.join('\n');
    assert.match(out, /ANTHROPIC_BASE_URL=http:\/\/127\.0\.0\.1:8484/);
    assert.ok(out.includes(`ANTHROPIC_AUTH_TOKEN=${state.local_token}`));
    assert.ok(!out.includes(state.session_seed_b58), 'session seed leaked to stdout');
    // Verify-what-you-sign summary named the amount and recipient.
    assert.match(out, /5(\.0+)? USDC/);
    assert.ok(out.includes(PROVIDER_WALLET));
  });

  // HIGH-2 (round-1 review): the gateway-reported mint/recipient are the
  // attacker-controlled surface of a phished --gateway. The mint is pinned to
  // canonical mainnet USDC unless --expected-mint overrides; --expected-recipient
  // pins the recipient when given. --yes NEVER bypasses these checks.
  const DEVNET_MINT = '4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU';

  it('rejects a non-canonical gateway mint even with --yes — nothing signed', async () => {
    const fake = await startFake({
      usdcMint: DEVNET_MINT,
      onChannelOpen: (_body, res) => res.writeHead(200).end('{}'),
    });
    const dir = await tmpDir();
    const { walletPath } = await writeWalletFile(dir);
    const statePath = path.join(dir, 'dropin.json');

    await assert.rejects(
      openChannel({
        gateway: fake.url,
        amount: '5',
        walletPath,
        rpcUrl: fake.url,
        yes: true, // must NOT bypass the mint pin
        port: 8484,
        statePath,
        log: () => {},
      }),
      /mint/i,
    );

    assert.ok(!fake.requests.some((r) => r.url === '/'), 'no blockhash fetch — nothing was signed');
    assert.ok(!fake.requests.some((r) => r.url === '/v1/channel/open'), 'no open POST');
    await assert.rejects(fs.access(statePath), 'state file must not exist');
  });

  it('accepts a non-canonical mint when --expected-mint pins it explicitly', async () => {
    const fake = await startFake({
      usdcMint: DEVNET_MINT,
      onChannelOpen: (body, res) => {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(
          JSON.stringify({
            channel_id: CHANNEL_ID_B58,
            agent_wallet: (body as Record<string, unknown>).agent_wallet,
            session_key: (body as Record<string, unknown>).session_key,
            deposited_atomic: 5_000_000,
            settled_atomic: 0,
            last_cumulative_atomic: 0,
            status: 'open',
            funding_tx_sig: 'FAKE_SIG',
            network: SOLANA_NETWORK,
          }),
        );
      },
    });
    const dir = await tmpDir();
    const { walletPath } = await writeWalletFile(dir);
    const statePath = path.join(dir, 'dropin.json');

    await openChannel({
      gateway: fake.url,
      amount: '5',
      walletPath,
      rpcUrl: fake.url,
      yes: true,
      expectedMint: DEVNET_MINT,
      port: 8484,
      statePath,
      log: () => {},
    });

    assert.ok(fake.requests.some((r) => r.url === '/v1/channel/open'), 'open proceeded under the explicit pin');
    const state = await loadState(statePath);
    assert.equal(state.channel_id, CHANNEL_ID_B58);
  });

  it('rejects an --expected-recipient mismatch even with --yes — nothing signed', async () => {
    const fake = await startFake({
      onChannelOpen: (_body, res) => res.writeHead(200).end('{}'),
    });
    const dir = await tmpDir();
    const { walletPath } = await writeWalletFile(dir);
    const statePath = path.join(dir, 'dropin.json');

    await assert.rejects(
      openChannel({
        gateway: fake.url,
        amount: '5',
        walletPath,
        rpcUrl: fake.url,
        yes: true, // must NOT bypass the recipient pin
        expectedRecipient: bs58.encode(new Uint8Array(32).fill(3)), // ≠ PROVIDER_WALLET
        port: 8484,
        statePath,
        log: () => {},
      }),
      /recipient/i,
    );

    assert.ok(!fake.requests.some((r) => r.url === '/'), 'no blockhash fetch — nothing was signed');
    assert.ok(!fake.requests.some((r) => r.url === '/v1/channel/open'), 'no open POST');
    await assert.rejects(fs.access(statePath), 'state file must not exist');
  });
});

// ---------------------------------------------------------------------------
// close
// ---------------------------------------------------------------------------

describe('close command', () => {
  it('signs buildCloseMessage with the session key and POSTs the exact close shape', async () => {
    let closeBody: Record<string, unknown> | null = null;
    const fake = await startFake({
      onChannelClose: (body, res) => {
        closeBody = body as Record<string, unknown>;
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(
          JSON.stringify({
            channel_id: CHANNEL_ID_B58,
            deposited_atomic: 5_000_000,
            settled_atomic: 0,
            refundable_atomic: 4_989_500,
            status: 'closing',
            refund_status: 'reserved',
            tx_signature: null,
          }),
        );
      },
    });
    const dir = await tmpDir();
    const statePath = path.join(dir, 'dropin.json');
    await saveState(statePath, {
      gateway_url: fake.url,
      port: 8484,
      channel_id: CHANNEL_ID_B58,
      session_seed_b58: bs58.encode(SESSION_SEED),
      local_token: 'tok',
      last_cumulative: '10500',
    });
    const lines: string[] = [];

    await closeChannel({ statePath, log: (l) => lines.push(l) });

    assert.ok(closeBody, 'gateway received the close request');
    const cb = closeBody as unknown as Record<string, unknown>;
    // Exactly the CloseChannelRequest fields (deny_unknown_fields on the gateway).
    assert.deepEqual(Object.keys(cb).sort(), ['channel_id', 'signature']);
    assert.equal(cb.channel_id, CHANNEL_ID_B58);

    // The signature verifies over the canonical close message with the session key.
    const sig = new Uint8Array(Buffer.from(String(cb.signature), 'base64'));
    assert.equal(sig.length, 64);
    const msg = buildCloseMessage(bs58.decode(CHANNEL_ID_B58));
    assert.ok(ed25519.verify(sig, msg, ed25519.getPublicKey(SESSION_SEED)));

    // Refund outcome surfaced to the user.
    const out = lines.join('\n');
    assert.match(out, /closing/);
    assert.match(out, /reserved/);
    assert.match(out, /4989500/);
  });
});
