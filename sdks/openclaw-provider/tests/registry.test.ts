/**
 * Routing profile registry tests.
 */

import { describe, it } from 'node:test';
import assert from 'node:assert/strict';

import {
  ROUTING_PROFILES,
  resolveDynamicModel,
  isRoutingProfile,
  profileToCatalogEntry,
} from '../src/registry.ts';

describe('ROUTING_PROFILES', () => {
  it('contains exactly four profiles: auto, eco, premium, free', () => {
    const ids = ROUTING_PROFILES.map((p) => p.id);
    assert.ok(ids.includes('solvela/auto'), 'must include solvela/auto');
    assert.ok(ids.includes('solvela/eco'), 'must include solvela/eco');
    assert.ok(ids.includes('solvela/premium'), 'must include solvela/premium');
    assert.ok(ids.includes('solvela/free'), 'must include solvela/free');
    assert.strictEqual(ids.length, 4, 'must have exactly 4 profiles');
  });

  it('each profile has required fields', () => {
    for (const p of ROUTING_PROFILES) {
      assert.ok(typeof p.id === 'string' && p.id.startsWith('solvela/'), `id must start with solvela/: ${p.id}`);
      assert.ok(typeof p.name === 'string' && p.name.length > 0, `name must be non-empty: ${p.id}`);
      assert.ok(typeof p.gatewayProfile === 'string', `gatewayProfile must be string: ${p.id}`);
      assert.ok(typeof p.contextWindow === 'number' && p.contextWindow > 0, `contextWindow must be positive: ${p.id}`);
      assert.ok(typeof p.maxTokens === 'number' && p.maxTokens > 0, `maxTokens must be positive: ${p.id}`);
    }
  });
});

describe('resolveDynamicModel', () => {
  it('solvela/auto resolves to "auto"', () => {
    assert.strictEqual(resolveDynamicModel('solvela/auto'), 'auto');
  });

  it('solvela/eco resolves to "eco"', () => {
    assert.strictEqual(resolveDynamicModel('solvela/eco'), 'eco');
  });

  it('solvela/premium resolves to "premium"', () => {
    assert.strictEqual(resolveDynamicModel('solvela/premium'), 'premium');
  });

  it('solvela/free resolves to "free"', () => {
    assert.strictEqual(resolveDynamicModel('solvela/free'), 'free');
  });

  it('real model IDs are returned as-is', () => {
    assert.strictEqual(resolveDynamicModel('claude-sonnet-4-20250514'), 'claude-sonnet-4-20250514');
    assert.strictEqual(resolveDynamicModel('gpt-4o'), 'gpt-4o');
  });

  it('unknown solvela/ ID is returned unchanged (caller detects and throws — HF-P3-H4)', () => {
    // registry.ts resolveDynamicModel returns the ID unchanged for unknown solvela/ prefixes.
    // The fail-loud behavior (HF-P3-H4) lives in index.ts's resolveDynamicModel wrapper,
    // which detects "resolved === input" and throws a descriptive error.
    // This test verifies the pass-through that enables caller detection.
    const result = resolveDynamicModel('solvela/unknown-profile');
    assert.strictEqual(result, 'solvela/unknown-profile');
  });
});

describe('isRoutingProfile', () => {
  it('returns true for all four profile IDs', () => {
    assert.ok(isRoutingProfile('solvela/auto'));
    assert.ok(isRoutingProfile('solvela/eco'));
    assert.ok(isRoutingProfile('solvela/premium'));
    assert.ok(isRoutingProfile('solvela/free'));
  });

  it('returns false for real model IDs', () => {
    assert.ok(!isRoutingProfile('gpt-4o'));
    assert.ok(!isRoutingProfile('claude-sonnet-4-20250514'));
    assert.ok(!isRoutingProfile('solvela/unknown'));
  });
});

describe('profileToCatalogEntry', () => {
  it('converts a profile to a SolvelaModel-shaped entry', () => {
    const profile = ROUTING_PROFILES.find((p) => p.id === 'solvela/auto')!;
    const entry = profileToCatalogEntry(profile);

    assert.strictEqual(entry.id, 'solvela/auto');
    assert.strictEqual(entry.provider, 'solvela');
    assert.ok(typeof entry.contextWindow === 'number');
    assert.ok(typeof entry.maxTokens === 'number');
    assert.strictEqual(entry.inputCostPerMillion, 0);
    assert.strictEqual(entry.outputCostPerMillion, 0);
    assert.strictEqual(entry.supportsStreaming, true);
  });
});
