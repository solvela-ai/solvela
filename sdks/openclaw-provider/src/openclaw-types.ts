/**
 * Minimal OpenClaw Provider Plugin SDK type stubs.
 *
 * These are typed against the documented API surface from
 * https://docs.openclaw.ai/plugins/sdk-provider-plugins
 *
 * The actual OpenClaw runtime injects the real `api` object at load time;
 * these types exist only for TypeScript type-checking during development.
 * They are intentionally conservative — only the hooks used by this plugin.
 *
 * WARNING: These are hand-rolled stubs of OpenClaw SDK types. Sync with
 * upstream when OpenClaw publishes @openclaw/sdk (HF-P3-M6).
 *
 * wrapStreamFn shape (HF-P3-H1): The real SDK docs confirm the factory shape:
 *   `(ctx: StreamFnContext) => StreamFn`
 * Plan §10.5 showed a flat `(request, next)` shape — that section is stale.
 * This file declares the correct factory shape.
 */

import type { SolvelaModel } from './models.generated.js';

/** A stream function that executes one inference call. */
export type StreamFn = (params: StreamParams) => Promise<StreamResult>;

/** Parameters passed to a stream function (outbound request). */
export interface StreamParams {
  /** Outbound HTTP headers — mutate to inject payment headers. */
  headers: Record<string, string>;
  /** Request body (JSON string). */
  body?: string;
  /** Target URL. */
  url?: string;
  [key: string]: unknown;
}

/** Result returned by a stream function. */
export type StreamResult = unknown;

/** Context passed to the wrapStreamFn hook. */
export interface StreamFnContext {
  /** The existing stream function to wrap. */
  streamFn?: StreamFn;
  /** The resolved model for this call. */
  model?: ResolvedModel;
  [key: string]: unknown;
}

/** A resolved model entry (as returned by catalog.run). */
export interface ResolvedModel {
  id: string;
  name: string;
  provider?: string;
  [key: string]: unknown;
}

/** Context passed to catalog.run. */
export interface CatalogContext {
  resolveProviderApiKey: (providerId: string) => { apiKey?: string };
  [key: string]: unknown;
}

/** Result shape returned by catalog.run. */
export interface CatalogResult {
  provider: {
    baseUrl: string;
    apiKey: string;
    api: 'openai-completions';
    models: SolvelaModel[];
  };
}

/** Context passed to resolveDynamicModel. */
export interface DynamicModelContext {
  modelId: string;
  [key: string]: unknown;
}

/** Auth method declaration for the provider manifest. */
export interface ProviderAuthMethod {
  method: 'api-key' | 'oauth' | 'none';
  envVar?: string;
  [key: string]: unknown;
}

/** The OpenClaw plugin API object injected by the host at load time. */
export interface OpenClawApi {
  registerProvider(config: ProviderConfig): void;
  [key: string]: unknown;
}

/** Full provider registration config. */
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
