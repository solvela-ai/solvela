#!/usr/bin/env tsx
/**
 * generate-models.ts
 *
 * Reads config/models.toml and emits src/generated/models.ts.
 *
 * TOML table keys (e.g. "openai-gpt-4o") are used directly as the union
 * members so the mapping is 1:1 unambiguous — model_id values are NOT used
 * because multiple TOML entries can share the same model_id string.
 *
 * Path resolution:
 *   SOLVELA_MODELS_TOML env var  →  use that path
 *   default                      →  path.resolve(__dirname, '../../../config/models.toml')
 *                                   (script lives at sdks/ai-sdk-provider/scripts/;
 *                                    three ".." walk back to the repo root)
 */

import * as fs from "node:fs";
import * as path from "node:path";
import { fileURLToPath } from "node:url";
import toml from "@iarna/toml";

// ---------------------------------------------------------------------------
// Path resolution
// ---------------------------------------------------------------------------

// ESM does not expose __dirname; derive it from import.meta.url.
const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

const tomlPath =
  process.env["SOLVELA_MODELS_TOML"] ??
  path.resolve(__dirname, "../../../config/models.toml");

const outputPath = path.resolve(__dirname, "../src/generated/models.ts");

// ---------------------------------------------------------------------------
// TOML types
// ---------------------------------------------------------------------------

interface ModelEntry {
  provider: string;
  model_id: string;
  display_name: string;
  input_cost_per_million: number;
  output_cost_per_million: number;
  context_window: number;
  supports_streaming?: boolean;
  supports_tools?: boolean;
  supports_vision?: boolean;
  reasoning?: boolean;
  supports_structured_output?: boolean;
  max_output_tokens?: number;
}

interface ModelsToml {
  models: Record<string, ModelEntry>;
}

// ---------------------------------------------------------------------------
// Parse
// ---------------------------------------------------------------------------

const raw = fs.readFileSync(tomlPath, "utf-8");
const parsed = toml.parse(raw) as unknown as ModelsToml;

if (!parsed.models || typeof parsed.models !== "object") {
  console.error(`[generate-models] No [models] table found in ${tomlPath}`);
  process.exit(1);
}

// Sort deterministically by TOML table key so the output is stable across
// different @iarna/toml insertion-order behaviour.
const sortedKeys = Object.keys(parsed.models).sort();

// ---------------------------------------------------------------------------
// Emit
// ---------------------------------------------------------------------------

const HEADER = `\
// AUTO-GENERATED — DO NOT EDIT.
// Run \`npm run generate-models\` to regenerate from config/models.toml.
`;

// Build the SolvelaModelId union.
// Each member is the TOML table key (e.g. "openai-gpt-4o").
// The union ends with | (string & {}) — the Vercel AI SDK idiom that keeps
// IDE autocomplete for known members while accepting any string at runtime
// without a TypeScript error.
const unionLines = sortedKeys
  .map((key) => `  | ${JSON.stringify(key)}`)
  .join("\n");

const typeBlock = `\
export type SolvelaModelId =
${unionLines}
  | (string & {});
`;

// Build the MODELS const array.
// Every field from the TOML entry is included; missing booleans default to
// false; missing numbers are omitted (undefined, not zero) so consumers can
// distinguish "not specified" from "0".
function boolField(v: boolean | undefined): boolean {
  return v === true;
}

function numFieldOrUndefined(
  v: number | undefined
): number | undefined {
  return typeof v === "number" ? v : undefined;
}

interface ModelRecord {
  id: string;
  provider: string;
  modelId: string;
  displayName: string;
  inputCostPerMillion: number;
  outputCostPerMillion: number;
  contextWindow: number;
  supportsStreaming: boolean;
  supportsTools: boolean;
  supportsVision: boolean;
  reasoning: boolean;
  supportsStructuredOutput: boolean;
  maxOutputTokens: number | undefined;
}

const modelObjects: ModelRecord[] = sortedKeys.map((key) => {
  const m = parsed.models[key];
  return {
    id: key,
    provider: m.provider,
    modelId: m.model_id,
    displayName: m.display_name,
    inputCostPerMillion: m.input_cost_per_million,
    outputCostPerMillion: m.output_cost_per_million,
    contextWindow: m.context_window,
    supportsStreaming: boolField(m.supports_streaming),
    supportsTools: boolField(m.supports_tools),
    supportsVision: boolField(m.supports_vision),
    reasoning: boolField(m.reasoning),
    supportsStructuredOutput: boolField(m.supports_structured_output),
    maxOutputTokens: numFieldOrUndefined(m.max_output_tokens),
  };
});

function renderObject(obj: ModelRecord): string {
  const lines: string[] = [
    `    id: ${JSON.stringify(obj.id)},`,
    `    provider: ${JSON.stringify(obj.provider)},`,
    `    modelId: ${JSON.stringify(obj.modelId)},`,
    `    displayName: ${JSON.stringify(obj.displayName)},`,
    `    inputCostPerMillion: ${obj.inputCostPerMillion},`,
    `    outputCostPerMillion: ${obj.outputCostPerMillion},`,
    `    contextWindow: ${obj.contextWindow},`,
    `    supportsStreaming: ${obj.supportsStreaming},`,
    `    supportsTools: ${obj.supportsTools},`,
    `    supportsVision: ${obj.supportsVision},`,
    `    reasoning: ${obj.reasoning},`,
    `    supportsStructuredOutput: ${obj.supportsStructuredOutput},`,
  ];
  if (obj.maxOutputTokens !== undefined) {
    lines.push(`    maxOutputTokens: ${obj.maxOutputTokens},`);
  }
  return `  {\n${lines.join("\n")}\n  }`;
}

const modelsBlock = `\
export const MODELS = [
${modelObjects.map(renderObject).join(",\n")}
] as const;
`;

// Combine and write with exactly one trailing newline.
const output = [HEADER, typeBlock, modelsBlock].join("\n");

// Ensure output directory exists (handles .gitkeep replacement).
fs.mkdirSync(path.dirname(outputPath), { recursive: true });
fs.writeFileSync(outputPath, output, "utf-8");

console.log(
  `[generate-models] Wrote ${sortedKeys.length} models → ${outputPath}`
);
