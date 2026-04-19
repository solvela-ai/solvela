/**
 * Model registry tests — validates codegen output from config/models.toml.
 *
 * Runs generate-models.ts via tsx and checks the resulting SOLVELA_MODELS array.
 * This test requires @iarna/toml (devDependency) and tsx to be installed.
 */

import { describe, it, before } from 'node:test';
import assert from 'node:assert/strict';
import { execSync } from 'node:child_process';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));
const PKG_ROOT = resolve(__dirname, '..');

describe('Model registry codegen', () => {
  before(() => {
    // Run the codegen so models.generated.ts is up-to-date
    execSync('npm run generate:models', {
      cwd: PKG_ROOT,
      stdio: 'pipe',
    });
  });

  it('SOLVELA_MODELS is a non-empty array', async () => {
    // Dynamic import after codegen ensures we get the fresh file
    const { SOLVELA_MODELS } = await import('../src/models.generated.ts');
    assert.ok(Array.isArray(SOLVELA_MODELS), 'SOLVELA_MODELS must be an array');
    assert.ok(SOLVELA_MODELS.length > 0, 'SOLVELA_MODELS must be non-empty');
  });

  it('MODEL_COUNT matches SOLVELA_MODELS.length', async () => {
    const { SOLVELA_MODELS, MODEL_COUNT } = await import('../src/models.generated.ts');
    assert.strictEqual(
      SOLVELA_MODELS.length,
      MODEL_COUNT,
      'MODEL_COUNT must equal SOLVELA_MODELS.length',
    );
  });

  it('each model has required fields', async () => {
    const { SOLVELA_MODELS } = await import('../src/models.generated.ts');
    for (const model of SOLVELA_MODELS) {
      assert.ok(typeof model.id === 'string' && model.id.startsWith('solvela/'), `id must start with solvela/: ${model.id}`);
      assert.ok(typeof model.name === 'string' && model.name.length > 0, `name must be non-empty: ${model.id}`);
      assert.ok(typeof model.provider === 'string' && model.provider.length > 0, `provider must be non-empty: ${model.id}`);
      assert.ok(typeof model.contextWindow === 'number' && model.contextWindow > 0, `contextWindow must be positive: ${model.id}`);
      assert.ok(typeof model.maxTokens === 'number' && model.maxTokens > 0, `maxTokens must be positive: ${model.id}`);
      assert.ok(typeof model.inputCostPerMillion === 'number' && model.inputCostPerMillion >= 0, `inputCostPerMillion must be >= 0: ${model.id}`);
      assert.ok(typeof model.outputCostPerMillion === 'number' && model.outputCostPerMillion >= 0, `outputCostPerMillion must be >= 0: ${model.id}`);
      assert.ok(typeof model.supportsStreaming === 'boolean', `supportsStreaming must be boolean: ${model.id}`);
    }
  });

  it('contains expected providers (openai, anthropic, google, deepseek, xai)', async () => {
    const { SOLVELA_MODELS } = await import('../src/models.generated.ts');
    const providers = new Set(SOLVELA_MODELS.map((m) => m.provider));
    for (const expected of ['openai', 'anthropic', 'google', 'deepseek', 'xai']) {
      assert.ok(providers.has(expected), `expected provider '${expected}' in model list`);
    }
  });

  it('generated file has DO NOT EDIT header', async () => {
    const { readFileSync } = await import('node:fs');
    const content = readFileSync(resolve(PKG_ROOT, 'src/models.generated.ts'), 'utf-8');
    assert.ok(
      content.includes('DO NOT EDIT'),
      'generated file must contain DO NOT EDIT header',
    );
  });

  it('all model IDs are unique', async () => {
    const { SOLVELA_MODELS } = await import('../src/models.generated.ts');
    const ids = SOLVELA_MODELS.map((m) => m.id);
    const unique = new Set(ids);
    assert.strictEqual(unique.size, ids.length, 'all model IDs must be unique');
  });
});
