#!/usr/bin/env node
/**
 * `solvela-dropin` CLI — subcommands `serve` (default) / `open` / `close`.
 *
 * `serve` binds 127.0.0.1 ONLY. A hosted sidecar is forbidden (design doc §4):
 * signing vouchers on users' behalf from a server would reintroduce the
 * custodial control problem the sidecar exists to remove.
 */

import { once } from 'node:events';
import { parseArgs } from 'node:util';

import bs58 from 'bs58';

import { createSidecarServer } from './server.js';
import { openChannel } from './open.js';
import { closeChannel } from './close.js';
import { assertNotPending, loadState, saveState, defaultStatePath, type DropinState } from './state.js';

const DEFAULT_PORT = 8484;

const USAGE = `solvela-dropin — Claude Code drop-in signing sidecar

Usage:
  solvela-dropin open --gateway <url> --amount <usdc> --wallet <keypair.json>
                      [--rpc-url <url>] [--port <n>] [--yes]
                      [--expected-mint <b58>] [--expected-recipient <b58>]
  solvela-dropin serve [--port <n>]
  solvela-dropin close

open   Fund + open a spend-down channel, write ~/.solvela/dropin.json, and
       print the two env vars (ANTHROPIC_BASE_URL / ANTHROPIC_AUTH_TOKEN).
       The gateway-reported USDC mint must match canonical mainnet USDC
       unless --expected-mint pins another (e.g. devnet); --expected-recipient
       pins the deposit recipient. Mismatches are hard rejects that --yes
       does NOT bypass.
serve  Run the sidecar on 127.0.0.1 (default port ${DEFAULT_PORT}).
close  Cooperatively close the channel; the unspent balance is refunded to
       the funding wallet. Re-run to poll the refund status.
`;

async function serve(args: string[]): Promise<void> {
  const { values } = parseArgs({
    args,
    options: {
      port: { type: 'string' },
      state: { type: 'string' },
    },
  });
  const statePath = values.state ?? defaultStatePath();
  const state = await loadState(statePath);
  assertNotPending(state, statePath);
  const port = values.port !== undefined ? Number(values.port) : state.port;
  if (!Number.isInteger(port) || port <= 0 || port > 65535) {
    throw new Error(`invalid --port: ${values.port}`);
  }

  const channelId = bs58.decode(state.channel_id);
  if (channelId.length !== 32) {
    throw new Error('state file channel_id is not a 32-byte base58 value');
  }
  const signSeed = bs58.decode(state.session_seed_b58);

  const persisted: DropinState = { ...state, port };
  // Rate-limit the persist-failure warning: log once when writes start
  // failing, again only after a write succeeds (a broken disk must not spam
  // one line per draw).
  let persistFailureLogged = false;
  const server = createSidecarServer({
    gatewayUrl: state.gateway_url,
    localToken: state.local_token,
    channelId,
    signSeed,
    lastCumulative: BigInt(state.last_cumulative),
    onLastCumulative: (last) => {
      // Best-effort optimization: the gateway's ledger is ground truth and the
      // tracker resyncs from it, so a lost write costs one round-trip, never money.
      persisted.last_cumulative = last.toString();
      void saveState(statePath, persisted).then(
        () => {
          persistFailureLogged = false;
        },
        (err: unknown) => {
          if (persistFailureLogged) return;
          persistFailureLogged = true;
          console.error(
            `[solvela-dropin] could not persist last_cumulative to ${statePath}: ${
              err instanceof Error ? err.message : String(err)
            } — harmless for funds (the gateway ledger is ground truth); suppressing repeats until a write succeeds`,
          );
        },
      );
    },
  });

  // 127.0.0.1 ONLY — never expose the signing surface off-box.
  server.listen(port, '127.0.0.1');
  await once(server, 'listening');
  console.error(`[solvela-dropin] sidecar listening on http://127.0.0.1:${port} (channel ${state.channel_id})`);
  console.error('[solvela-dropin] point Claude Code at it:');
  console.error(`  export ANTHROPIC_BASE_URL=http://127.0.0.1:${port}`);
  console.error(`  export ANTHROPIC_AUTH_TOKEN=${state.local_token}`);
  console.error('[solvela-dropin] note: draws are strictly serial per channel — parallel requests queue.');
}

async function open(args: string[]): Promise<void> {
  const { values } = parseArgs({
    args,
    options: {
      gateway: { type: 'string' },
      amount: { type: 'string' },
      wallet: { type: 'string' },
      'rpc-url': { type: 'string' },
      'expected-mint': { type: 'string' },
      'expected-recipient': { type: 'string' },
      port: { type: 'string' },
      state: { type: 'string' },
      yes: { type: 'boolean', default: false },
    },
  });
  if (!values.gateway || !values.amount || !values.wallet) {
    throw new Error('open requires --gateway, --amount, and --wallet\n\n' + USAGE);
  }
  const port = values.port !== undefined ? Number(values.port) : DEFAULT_PORT;
  if (!Number.isInteger(port) || port <= 0 || port > 65535) {
    throw new Error(`invalid --port: ${values.port}`);
  }
  await openChannel({
    gateway: values.gateway,
    amount: values.amount,
    walletPath: values.wallet,
    rpcUrl: values['rpc-url'],
    expectedMint: values['expected-mint'],
    expectedRecipient: values['expected-recipient'],
    yes: values.yes ?? false,
    port,
    statePath: values.state,
  });
}

async function close(args: string[]): Promise<void> {
  const { values } = parseArgs({
    args,
    options: { state: { type: 'string' } },
  });
  await closeChannel({ statePath: values.state });
}

async function main(): Promise<void> {
  const argv = process.argv.slice(2);
  const hasSubcommand = argv.length > 0 && !argv[0].startsWith('-');
  const cmd = hasSubcommand ? argv[0] : 'serve';
  const rest = hasSubcommand ? argv.slice(1) : argv;

  switch (cmd) {
    case 'serve':
      await serve(rest);
      return;
    case 'open':
      await open(rest);
      return;
    case 'close':
      await close(rest);
      return;
    case 'help':
    case '--help':
      console.log(USAGE);
      return;
    default:
      throw new Error(`unknown subcommand: ${cmd}\n\n${USAGE}`);
  }
}

main().catch((err: unknown) => {
  // Error messages in this package never carry key material (see open/close/
  // state docs) — safe to print.
  console.error(`[solvela-dropin] ${err instanceof Error ? err.message : String(err)}`);
  process.exitCode = 1;
});
