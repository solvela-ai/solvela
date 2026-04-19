/**
 * Manifest validation tests for openclaw.plugin.json.
 *
 * Ensures the manifest has the required shape for OpenClaw plugin loading.
 * configSchema has been removed (HF-P3-M7 option B) — config is env-based only.
 */

import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const MANIFEST_PATH = resolve(__dirname, '../openclaw.plugin.json');

describe('openclaw.plugin.json manifest', () => {
  it('loads and parses as valid JSON', () => {
    const raw = readFileSync(MANIFEST_PATH, 'utf-8');
    assert.doesNotThrow(() => JSON.parse(raw), 'manifest must be valid JSON');
  });

  it('has required top-level fields', () => {
    const manifest = JSON.parse(readFileSync(MANIFEST_PATH, 'utf-8'));
    assert.ok(typeof manifest.id === 'string' && manifest.id.length > 0, 'id must be a non-empty string');
    assert.ok(typeof manifest.version === 'string', 'version must be a string');
    assert.ok(Array.isArray(manifest.providers), 'providers must be an array');
    assert.ok(manifest.providers.includes('solvela'), 'providers must include "solvela"');
  });

  it('declares providerAuthEnvVars with SOLANA_WALLET_KEY and SOLANA_RPC_URL', () => {
    const manifest = JSON.parse(readFileSync(MANIFEST_PATH, 'utf-8'));
    assert.ok(
      typeof manifest.providerAuthEnvVars === 'object' && manifest.providerAuthEnvVars !== null,
      'providerAuthEnvVars must be an object',
    );
    const solvelaVars: string[] = manifest.providerAuthEnvVars['solvela'];
    assert.ok(Array.isArray(solvelaVars), 'providerAuthEnvVars.solvela must be an array');
    assert.ok(solvelaVars.includes('SOLANA_WALLET_KEY'), 'must declare SOLANA_WALLET_KEY');
    assert.ok(solvelaVars.includes('SOLANA_RPC_URL'), 'must declare SOLANA_RPC_URL');
  });

  it('configSchema is absent — config is env-based only (HF-P3-M7 option B)', () => {
    const manifest = JSON.parse(readFileSync(MANIFEST_PATH, 'utf-8'));
    assert.ok(
      !('configSchema' in manifest),
      'configSchema must be absent — plugin config is via environment variables, not schema',
    );
  });

  it('providerAuthChoices declares api-key method with SOLANA_WALLET_KEY', () => {
    const manifest = JSON.parse(readFileSync(MANIFEST_PATH, 'utf-8'));
    const choices = manifest.providerAuthChoices;
    assert.ok(Array.isArray(choices), 'providerAuthChoices must be an array');
    const solvelaChoice = choices.find(
      (c: { provider: string }) => c.provider === 'solvela',
    );
    assert.ok(solvelaChoice, 'must have a choice for solvela provider');
    assert.strictEqual(solvelaChoice.method, 'api-key');
    assert.strictEqual(solvelaChoice.envVar, 'SOLANA_WALLET_KEY');
  });
});
