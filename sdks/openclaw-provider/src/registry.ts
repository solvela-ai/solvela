/**
 * Routing profile registry for the Solvela OpenClaw Provider Plugin.
 *
 * Exposes routing profiles (eco/auto/premium/free) as selectable "models"
 * in OpenClaw's model picker alongside real model entries from codegen.
 *
 * Profile model IDs use the prefix "solvela/" so they are clearly namespaced
 * in OpenClaw's unified picker.
 */

import type { SolvelaModel } from './models.generated.js';

/** A routing profile entry that appears in the model picker. */
export interface ProfileModel {
  id: string;
  name: string;
  description: string;
  /** The gateway routing profile keyword sent as the `model` field. */
  gatewayProfile: string;
  contextWindow: number;
  maxTokens: number;
}

/** All four routing profiles exposed as picker entries. */
export const ROUTING_PROFILES: ProfileModel[] = [
  {
    id: 'solvela/auto',
    name: 'Solvela Auto (smart router)',
    description: 'Solvela smart router picks the cheapest capable model for your prompt.',
    gatewayProfile: 'auto',
    contextWindow: 1_000_000,
    maxTokens: 32_768,
  },
  {
    id: 'solvela/eco',
    name: 'Solvela Eco (cheapest tier)',
    description: 'Forces the cheapest available model tier.',
    gatewayProfile: 'eco',
    contextWindow: 1_000_000,
    maxTokens: 32_768,
  },
  {
    id: 'solvela/premium',
    name: 'Solvela Premium (best quality)',
    description: 'Forces the highest-quality model tier.',
    gatewayProfile: 'premium',
    contextWindow: 200_000,
    maxTokens: 32_768,
  },
  {
    id: 'solvela/free',
    name: 'Solvela Free (open-source only)',
    description: 'Forces open-source, zero-cost models only.',
    gatewayProfile: 'free',
    contextWindow: 128_000,
    maxTokens: 32_768,
  },
];

/** All profile IDs for fast O(1) lookup. */
const PROFILE_ID_SET = new Set(ROUTING_PROFILES.map((p) => p.id));

/**
 * Resolve a dynamic model ID to its gateway model/profile string.
 *
 * Called by resolveDynamicModel when OpenClaw routes a request for a model
 * that is not in the static catalog (e.g. when the user selected a routing
 * profile or a freshly-added gateway model).
 *
 * For `solvela/` prefixed IDs: must be a known routing profile. Returns
 * the gatewayProfile keyword. Unknown `solvela/` IDs return the input
 * unchanged so the caller can detect the mismatch and throw (HF-P3-H4).
 *
 * For non-`solvela/` IDs: returned as-is (gateway accepts model_id directly).
 *
 * @param modelId - The model ID chosen by the user (e.g. "solvela/auto")
 * @returns The gateway-level model string to send in the request body.
 */
export function resolveDynamicModel(modelId: string): string {
  // Strip leading "solvela/" prefix if present for gateway routing
  const profile = ROUTING_PROFILES.find((p) => p.id === modelId);
  if (profile) return profile.gatewayProfile;

  // Known real model — return as-is (gateway accepts model_id directly).
  // Unknown solvela/* IDs also return as-is; the caller detects this and
  // throws a clear error (HF-P3-H4 — fail loud, not silent fallback).
  return modelId;
}

/**
 * Returns true when the given model ID refers to a routing profile.
 */
export function isRoutingProfile(modelId: string): boolean {
  return PROFILE_ID_SET.has(modelId);
}

/**
 * Build the OpenClaw model catalog entry for a routing profile.
 * Shaped to match the openai-completions model schema.
 */
export function profileToCatalogEntry(profile: ProfileModel): SolvelaModel {
  return {
    id: profile.id,
    name: profile.name,
    provider: 'solvela',
    contextWindow: profile.contextWindow,
    maxTokens: profile.maxTokens,
    inputCostPerMillion: 0,
    outputCostPerMillion: 0,
    supportsStreaming: true,
  };
}
