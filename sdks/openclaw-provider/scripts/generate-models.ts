/**
 * Model registry codegen — reads config/models.toml and emits
 * src/models.generated.ts for the OpenClaw Provider Plugin.
 *
 * DO NOT RUN manually except via: npm run generate:models
 * This script is invoked automatically as part of `npm run build` via prebuild.
 *
 * Usage:
 *   npm run generate:models
 */

import { readFileSync, writeFileSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

// @iarna/toml is a devDependency — safe to use in scripts only
// eslint-disable-next-line @typescript-eslint/no-require-imports
const toml = await import('@iarna/toml');

const __dirname = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(__dirname, '../../../');
const MODELS_TOML = resolve(REPO_ROOT, 'config/models.toml');
const OUT_FILE = resolve(__dirname, '../src/models.generated.ts');

interface TomlModel {
  provider: string;
  model_id: string;
  display_name: string;
  input_cost_per_million: number;
  output_cost_per_million: number;
  context_window: number;
  supports_streaming?: boolean;
  supports_tools?: boolean;
  supports_vision?: boolean;
  supports_structured_output?: boolean;
  reasoning?: boolean;
  max_output_tokens?: number;
}

interface TomlFile {
  models: Record<string, TomlModel>;
}

const raw = readFileSync(MODELS_TOML, 'utf-8');
const parsed = toml.parse(raw) as unknown as TomlFile;

// Validate each TOML entry before emitting — fail loud on missing required fields (HF-P3-L4)
const REQUIRED_FIELDS: (keyof TomlModel)[] = [
  'provider',
  'model_id',
  'display_name',
  'input_cost_per_million',
  'output_cost_per_million',
  'context_window',
];

for (const [key, m] of Object.entries(parsed.models)) {
  for (const field of REQUIRED_FIELDS) {
    if (m[field] === undefined || m[field] === null || m[field] === '') {
      throw new Error(
        `[generate-models] TOML entry '${key}' is missing required field '${field}'. ` +
          'Fix config/models.toml before regenerating.',
      );
    }
  }
  if (typeof m.context_window !== 'number' || m.context_window <= 0) {
    throw new Error(
      `[generate-models] TOML entry '${key}' has invalid context_window: ${m.context_window}`,
    );
  }
  if (typeof m.input_cost_per_million !== 'number' || m.input_cost_per_million < 0) {
    throw new Error(
      `[generate-models] TOML entry '${key}' has invalid input_cost_per_million: ${m.input_cost_per_million}`,
    );
  }
  if (typeof m.output_cost_per_million !== 'number' || m.output_cost_per_million < 0) {
    throw new Error(
      `[generate-models] TOML entry '${key}' has invalid output_cost_per_million: ${m.output_cost_per_million}`,
    );
  }
}

const allModels = Object.entries(parsed.models).map(([key, m]) => ({
  key,
  id: `solvela/${m.model_id}`,
  name: m.display_name,
  provider: m.provider,
  contextWindow: m.context_window,
  maxTokens: m.max_output_tokens ?? Math.min(m.context_window, 32_768),
  inputCostPerMillion: m.input_cost_per_million,
  outputCostPerMillion: m.output_cost_per_million,
  supportsStreaming: m.supports_streaming ?? false,
  supportsTools: m.supports_tools ?? false,
  supportsVision: m.supports_vision ?? false,
  supportsStructuredOutput: m.supports_structured_output ?? false,
  reasoning: m.reasoning ?? false,
}));

// Deduplicate by gateway model_id (id field) — keep first occurrence.
// models.toml may contain multiple TOML keys pointing to the same gateway model_id.
const seenIds = new Set<string>();
const models = allModels.filter((m) => {
  if (seenIds.has(m.id)) return false;
  seenIds.add(m.id);
  return true;
});

const lines: string[] = [
  '// DO NOT EDIT — regenerate via: npm run generate:models',
  '// Source: config/models.toml',
  '//',
  '// This file is committed so the build does not require @iarna/toml at runtime.',
  '',
  'export interface SolvelaModel {',
  '  /** Namespaced model ID: "solvela/<gateway-model-id>" */',
  '  id: string;',
  '  /** Display name shown in OpenClaw model picker */',
  '  name: string;',
  '  /** Upstream provider (openai | anthropic | google | deepseek | xai) */',
  '  provider: string;',
  '  contextWindow: number;',
  '  maxTokens: number;',
  '  /** Provider cost per million input tokens (before 5% Solvela fee) */',
  '  inputCostPerMillion: number;',
  '  /** Provider cost per million output tokens (before 5% Solvela fee) */',
  '  outputCostPerMillion: number;',
  '  supportsStreaming: boolean;',
  '  supportsTools?: boolean;',
  '  supportsVision?: boolean;',
  '  supportsStructuredOutput?: boolean;',
  '  reasoning?: boolean;',
  '}',
  '',
  '/**',
  ' * All Solvela models generated from config/models.toml.',
  ' * Routing profiles (solvela/auto, solvela/eco, etc.) are added separately',
  ' * by registry.ts and are NOT included here.',
  ' */',
  `export const SOLVELA_MODELS: SolvelaModel[] = ${JSON.stringify(models.map(({ key: _key, ...rest }) => rest), null, 2)};`,
  '',
  `export const MODEL_COUNT = ${models.length};`,
  '',
];

writeFileSync(OUT_FILE, lines.join('\n'), 'utf-8');
const skipped = allModels.length - models.length;
process.stdout.write(
  `[generate-models] wrote ${models.length} models to src/models.generated.ts` +
    (skipped > 0 ? ` (${skipped} duplicate model_id entries skipped)` : '') +
    '\n',
);
