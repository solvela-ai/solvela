/**
 * Startup contract test for the .mcpb / Claude Desktop launch environment.
 *
 * Claude Desktop substitutes '' (empty string) for every blank optional
 * user_config field in manifest.json — so a default install launches the
 * server with SOLVELA_SESSION_BUDGET='' and SOLANA_WALLET_KEY=''. Empty
 * must mean "unset", never a fatal. Regression: pre-fix, Number('') === 0
 * tripped the positive-number guard and every default install died on boot.
 */

import { describe, it, before } from 'node:test';
import assert from 'node:assert/strict';
import { spawn, execSync } from 'node:child_process';
import { mkdtemp, rm } from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

import { GatewayClient } from '../src/client.ts';

const pkgRoot = path.join(path.dirname(fileURLToPath(import.meta.url)), '..');
// src/ imports use tsc-style .js specifiers, so the raw .ts entrypoint can't
// run under type stripping — spawn the compiled output, built here so this
// test can never silently pass against a stale or missing dist/.
const serverEntry = path.join(pkgRoot, 'dist', 'index.js');

before(() => {
  execSync('npm run build --silent', { cwd: pkgRoot, stdio: 'pipe', timeout: 120_000 });
});

interface SpawnResult {
  code: number | null;
  stdout: string;
  stderr: string;
  gotInitResponse: boolean;
}

/** Spawn the real server entrypoint with the given env; send an MCP
 *  initialize request; resolve once we see the response or the process dies. */
async function spawnServer(env: Record<string, string>): Promise<SpawnResult> {
  // Sandbox HOME so wallet auto-create never touches the real ~/.solvela.
  const home = await mkdtemp(path.join(os.tmpdir(), 'solvela-startup-test-'));
  const child = spawn(process.execPath, [serverEntry], {
    env: { PATH: process.env['PATH'] ?? '', HOME: home, ...env },
    stdio: ['pipe', 'pipe', 'pipe'],
  });

  let stdout = '';
  let stderr = '';
  child.stdout.on('data', (d: Buffer) => (stdout += d.toString()));
  child.stderr.on('data', (d: Buffer) => (stderr += d.toString()));
  child.on('error', (err) => (stderr += `spawn error: ${err.message}\n`));

  child.stdin.write(
    JSON.stringify({
      jsonrpc: '2.0',
      id: 1,
      method: 'initialize',
      params: {
        protocolVersion: '2025-06-18',
        capabilities: {},
        clientInfo: { name: 'startup-test', version: '0' },
      },
    }) + '\n',
  );

  try {
    return await new Promise<SpawnResult>((resolve) => {
      const finish = (code: number | null) =>
        resolve({ code, stdout, stderr, gotInitResponse: stdout.includes('"serverInfo"') });
      const poll = setInterval(() => {
        if (stdout.includes('"serverInfo"')) {
          clearInterval(poll);
          child.kill();
          finish(0);
        }
      }, 100);
      child.on('exit', (code: number | null) => {
        clearInterval(poll);
        finish(code);
      });
      setTimeout(() => {
        clearInterval(poll);
        child.kill('SIGKILL');
        finish(null);
      }, 20_000).unref();
    });
  } finally {
    await rm(home, { recursive: true, force: true });
  }
}

// The env shape a default .mcpb install produces: URL fields set (their
// manifest user_config entries carry defaults), '' for the two blank optional
// fields (see manifest user_config). The blanks are what's under test; the
// URLs point at an unreachable local port so startup's best-effort gas check
// fails fast instead of hitting real mainnet RPC and the PRODUCTION faucet
// (which would drip real SOL to a throwaway test wallet on every run).
const desktopDefaultEnv = {
  SOLVELA_API_URL: 'http://127.0.0.1:9',
  SOLANA_RPC_URL: 'http://127.0.0.1:9',
  SOLANA_WALLET_KEY: '',
  SOLVELA_SESSION_BUDGET: '',
};

describe('server startup under the Claude Desktop (.mcpb) default env', () => {
  it('boots and answers initialize with blank optional fields passed as empty strings', async () => {
    const r = await spawnServer(desktopDefaultEnv);
    assert.ok(
      !r.stderr.includes('Fatal'),
      `server must not fatal on empty-string optional env; stderr: ${r.stderr}`,
    );
    assert.ok(r.gotInitResponse, `expected initialize response; stderr: ${r.stderr}`);
  });

  it('boots when Desktop passes UNSUBSTITUTED templates (settings never saved)', async () => {
    // Observed on Desktop 1.1.5368: until the user saves the extension's
    // settings once, env arrives as the literal manifest templates. URL vars
    // stay stubbed here so the SOLVELA_MCPB defaults branch cannot reach the
    // production gateway/faucet from CI.
    const r = await spawnServer({
      ...desktopDefaultEnv,
      SOLVELA_MCPB: '1',
      // eslint-disable-next-line no-template-curly-in-string
      SOLANA_WALLET_KEY: '${user_config.solana_wallet_key}',
      // eslint-disable-next-line no-template-curly-in-string
      SOLVELA_SESSION_BUDGET: '${user_config.session_budget}',
    });
    assert.ok(
      !r.stderr.includes('Fatal'),
      `server must treat unsubstituted templates as unset; stderr: ${r.stderr}`,
    );
    assert.ok(r.gotInitResponse, `expected initialize response; stderr: ${r.stderr}`);
  });

  it('still fails closed on a genuinely invalid budget', async () => {
    const r = await spawnServer({ ...desktopDefaultEnv, SOLVELA_SESSION_BUDGET: 'abc' });
    assert.equal(r.code, 1);
    assert.ok(r.stderr.includes('not a positive number'), `stderr: ${r.stderr}`);
  });
});

describe('GatewayClient apiUrl under a cleared gateway_url field', () => {
  it("treats '' as unset and falls back to the default, never a relative URL", () => {
    const saved = process.env['SOLVELA_API_URL'];
    process.env['SOLVELA_API_URL'] = '';
    try {
      const client = new GatewayClient();
      assert.equal(client.apiUrl, 'https://api.solvela.ai');
    } finally {
      if (saved === undefined) delete process.env['SOLVELA_API_URL'];
      else process.env['SOLVELA_API_URL'] = saved;
    }
  });
});
