/**
 * Type surface for the Solvela OpenClaw Provider Plugin.
 *
 * What is REAL (imported from upstream npm packages):
 *
 *   - StreamFn — comes from @earendil-works/pi-agent-core. Real signature is
 *     (model, context, options?) => AssistantMessageEventStream | Promise<...>.
 *     See node_modules/@earendil-works/pi-agent-core/dist/types.d.ts and
 *     node_modules/@earendil-works/pi-ai/dist/stream.d.ts. The OpenClaw runtime
 *     invokes wrapped StreamFns as 3-arg — canonical refs:
 *       - openclaw/openclaw@main: src/agents/pi-embedded-runner/extra-params.ts:418
 *       - openclaw/openclaw@main: src/agents/pi-embedded-runner/openai-stream-wrappers.ts:567
 *       - openclaw/openclaw@main: src/plugin-sdk/provider-stream-shared.test.ts:78
 *
 * What is STILL a hand-rolled stub (no upstream npm equivalent today):
 *
 *   - The plugin SDK surface (`OpenClawApi`, `ProviderConfig`, `StreamFnContext`,
 *     `CatalogContext`, `DynamicModelContext`, `ResolvedModel`, `ProviderAuthMethod`,
 *     `CatalogResult`). These shapes live in openclaw/openclaw@main:src/plugins/types.ts
 *     but are not yet published as a standalone package. They are conservatively
 *     mirrored here for the hooks this plugin uses. Re-verify against canonical
 *     before adding new fields.
 *
 * PR 1 (type honesty) intentionally exposes the bug at compile time:
 * the `wrapStreamFn` body in src/index.ts is built around a fictional
 * 1-arg (params) shape — the OpenClaw runtime calls it as 3-arg
 * (model, context, options). The typechecker now flags this. PR 3 fixes
 * the runtime path (undici dispatcher); only then will typecheck go green.
 *
 * DO NOT mark the broken sites with @ts-expect-error to "fix" CI — the
 * compile failure is the deliverable for PR 1.
 */

import type { SolvelaModel } from './models.generated.js';

// Real StreamFn — sourced from upstream. Never redeclare locally.
export type { StreamFn } from '@earendil-works/pi-agent-core';
import type { StreamFn } from '@earendil-works/pi-agent-core';

/**
 * Context passed to the wrapStreamFn hook.
 *
 * Stub — mirrored from openclaw/openclaw@main:src/plugins/types.ts. Only the
 * fields this plugin reads. The `streamFn` field is now typed against the
 * real StreamFn (3-arg) — wrapping it requires `(model, context, options)`.
 */
export interface StreamFnContext {
  /** The inner stream function to wrap. */
  streamFn?: StreamFn;
  /** The resolved model for this call. */
  model?: ResolvedModel;
  [key: string]: unknown;
}

/** A resolved model entry (as returned by catalog.run). Stub — see file header. */
export interface ResolvedModel {
  id: string;
  name: string;
  provider?: string;
  [key: string]: unknown;
}

/** Context passed to catalog.run. Stub — see file header. */
export interface CatalogContext {
  resolveProviderApiKey: (providerId: string) => { apiKey?: string };
  [key: string]: unknown;
}

/** Result shape returned by catalog.run. Stub — see file header. */
export interface CatalogResult {
  provider: {
    baseUrl: string;
    apiKey: string;
    api: 'openai-completions';
    models: SolvelaModel[];
  };
}

/** Context passed to resolveDynamicModel. Stub — see file header. */
export interface DynamicModelContext {
  modelId: string;
  [key: string]: unknown;
}

/** Auth method declaration for the provider manifest. Stub — see file header. */
export interface ProviderAuthMethod {
  method: 'api-key' | 'oauth' | 'none';
  envVar?: string;
  [key: string]: unknown;
}

/** The OpenClaw plugin API object injected by the host at load time. Stub — see file header. */
export interface OpenClawApi {
  registerProvider(config: ProviderConfig): void;
  [key: string]: unknown;
}

/** Full provider registration config. Stub — see file header. */
export interface ProviderConfig {
  id: string;
  label: string;
  docsPath?: string;
  envVars?: string[];
  auth: ProviderAuthMethod[];
  catalog: {
    order: 'simple' | 'profile' | 'paired' | 'late';
    run: (ctx: CatalogContext) => Promise<CatalogResult | null>;
  };
  resolveDynamicModel?: (ctx: DynamicModelContext) => ResolvedModel | null;
  wrapStreamFn?: (ctx: StreamFnContext) => StreamFn;
}
