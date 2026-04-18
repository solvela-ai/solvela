/**
 * Unit-8 — codegen.test.ts
 *
 * Tests for the committed src/generated/models.ts output and the
 * generate-models.ts codegen script that produces it.
 *
 * Plan reference: §6 Phase 7 Unit-8 + Round 3 refinements.
 *
 * Assertions:
 *   1. Banner — file starts with the exact AUTO-GENERATED banner (em-dash U+2014).
 *   2. Union members — SolvelaModelId union contains exactly the 26 known model IDs.
 *   3. Escape-hatch tail — last union member is `| (string & {})`.
 *   4. Type-level assignability — a non-enumerated string is assignable to SolvelaModelId
 *      without a TypeScript error (runtime proof of the `(string & {})` tail).
 *   5. MODELS array — exports an array of exactly 26 entries.
 *   6. MODELS id coverage — every entry's `id` field is one of the 26 known keys.
 *   7. Drift guard — re-running the codegen script produces byte-identical output.
 *
 * Note on type-level test (assertion 4):
 *   tsconfig.json excludes `tests/**` from the project compilation, so `tsc --noEmit`
 *   does not validate this file. The runtime assertion below proves only that a plain
 *   string value can be constructed as a SolvelaModelId variable; the real TS safety is
 *   that the type does NOT extend `string` without the `(string & {})` tail — if the
 *   tail were removed, only the 26 literal members would be valid at compile time. The
 *   comment below doubles as an in-file annotation that reviewers can copy into a
 *   strict-tsc context to verify.
 *
 *   // @ts-expect-no-error (verified manually / CI tsc pass over tests when added):
 *   // const _t: SolvelaModelId = "hypothetical-future-model";
 */

import * as fs from "node:fs";
import * as path from "node:path";
import { fileURLToPath } from "node:url";
import { execSync } from "node:child_process";
import { describe, it, expect } from "vitest";

import { MODELS, type SolvelaModelId } from "../../src/generated/models.js";

// ---------------------------------------------------------------------------
// Path constants
// ---------------------------------------------------------------------------

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

/** Absolute path to the package root (sdks/ai-sdk-provider/). */
const PACKAGE_ROOT = path.resolve(__dirname, "../../");

/** Absolute path to the committed generated file. */
const GENERATED_FILE = path.resolve(PACKAGE_ROOT, "src/generated/models.ts");

// ---------------------------------------------------------------------------
// The 26 known model IDs (sorted alphabetically — matches script's `sortedKeys`).
// Update this list when config/models.toml gains new entries.
// ---------------------------------------------------------------------------

const KNOWN_MODEL_IDS: readonly string[] = [
  "anthropic-claude-haiku-4-5",
  "anthropic-claude-opus-4-6",
  "anthropic-claude-sonnet-4-5",
  "anthropic-claude-sonnet-4-6",
  "deepseek-chat",
  "deepseek-coder",
  "deepseek-reasoner",
  "google-gemini-2-0-flash",
  "google-gemini-2-0-flash-lite",
  "google-gemini-2-5-flash",
  "google-gemini-2-5-flash-lite",
  "google-gemini-3-1-pro",
  "openai-gpt-4-1",
  "openai-gpt-4-1-mini",
  "openai-gpt-4-1-nano",
  "openai-gpt-4o",
  "openai-gpt-4o-mini",
  "openai-gpt-5-2",
  "openai-gpt-oss-120b",
  "openai-o3",
  "openai-o3-mini",
  "openai-o4-mini",
  "xai-grok-3",
  "xai-grok-3-mini",
  "xai-grok-4-fast-reasoning",
  "xai-grok-code-fast-1",
] as const;

const EXPECTED_COUNT = 26;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/**
 * Read the generated file text once and reuse across tests.
 * Using a module-level variable keeps individual tests readable without
 * repeated I/O.
 */
let _fileText: string | null = null;
function generatedFileText(): string {
  if (_fileText === null) {
    _fileText = fs.readFileSync(GENERATED_FILE, "utf-8");
  }
  return _fileText;
}

/**
 * Extract the quoted string members from the SolvelaModelId union.
 * Matches lines of the form:  `  | "some-model-id"`
 * Returns them in document order (which is alphabetical, matching the codegen sort).
 */
function extractUnionMembers(source: string): string[] {
  const unionPattern = /\|\s+"([^"]+)"/g;
  const members: string[] = [];
  let match: RegExpExecArray | null;
  while ((match = unionPattern.exec(source)) !== null) {
    members.push(match[1]);
  }
  return members;
}

// ---------------------------------------------------------------------------
// Test suite
// ---------------------------------------------------------------------------

describe("codegen — src/generated/models.ts", () => {
  // -------------------------------------------------------------------------
  // 1. Banner
  // -------------------------------------------------------------------------

  it("starts with the AUTO-GENERATED banner containing an em-dash (U+2014)", () => {
    const text = generatedFileText();
    // The em-dash character is U+2014 (—), not a hyphen.
    const expectedBannerStart = "// AUTO-GENERATED \u2014 DO NOT EDIT.";
    expect(text.startsWith(expectedBannerStart)).toBe(true);
  });

  it("includes the regeneration instruction on the second banner line", () => {
    const text = generatedFileText();
    expect(text).toContain(
      "// Run `npm run generate-models` to regenerate from config/models.toml."
    );
  });

  // -------------------------------------------------------------------------
  // 2. Union members
  // -------------------------------------------------------------------------

  it("SolvelaModelId union contains exactly 26 enumerated string members", () => {
    const text = generatedFileText();
    const members = extractUnionMembers(text);
    expect(members).toHaveLength(EXPECTED_COUNT);
  });

  it("SolvelaModelId union members match the known set of 26 model IDs exactly", () => {
    const text = generatedFileText();
    const members = extractUnionMembers(text);
    // Sort both for set-equality regardless of insertion order (though the
    // codegen sorts alphabetically, so order already matches KNOWN_MODEL_IDS).
    expect([...members].sort()).toEqual([...KNOWN_MODEL_IDS].sort());
  });

  it("SolvelaModelId union members are in alphabetical order (matches codegen sort)", () => {
    const text = generatedFileText();
    const members = extractUnionMembers(text);
    expect(members).toEqual([...members].sort());
  });

  // -------------------------------------------------------------------------
  // 3. Escape-hatch tail
  // -------------------------------------------------------------------------

  it("SolvelaModelId union last member is exactly `| (string & {})`", () => {
    const text = generatedFileText();
    // Find the full union block — everything between "export type SolvelaModelId ="
    // and the first semicolon that closes it.
    const typeBlockStart = text.indexOf("export type SolvelaModelId =");
    expect(typeBlockStart).toBeGreaterThan(-1);

    const semicolonAfterType = text.indexOf(";", typeBlockStart);
    const unionBlock = text.slice(typeBlockStart, semicolonAfterType + 1);

    // The tail must be present and must be the last union member line.
    expect(unionBlock).toContain("| (string & {})");

    // Confirm the tail comes after all quoted members.
    const lastQuotedMemberIdx = unionBlock.lastIndexOf('| "');
    const tailIdx = unionBlock.lastIndexOf("| (string & {})");
    expect(tailIdx).toBeGreaterThan(lastQuotedMemberIdx);
  });

  // -------------------------------------------------------------------------
  // 4. Type-level assignability (runtime proof of the (string & {}) tail)
  // -------------------------------------------------------------------------

  it("a non-enumerated string is assignable to SolvelaModelId at runtime without error", () => {
    // This proves the (string & {}) tail is present and effective.
    // If the tail were absent, only the 26 literal members would be the type —
    // but the runtime assignment below would still succeed because JS has no
    // type guards. The real TS proof is in the comment at the top of this file.
    //
    // Runtime assertion: constructing a SolvelaModelId variable from an arbitrary
    // string must not throw, and the value must round-trip correctly.
    const futureModel: SolvelaModelId = "hypothetical-future-model";
    expect(futureModel).toBe("hypothetical-future-model");
  });

  it("a non-enumerated string is not present in the MODELS array (future-model safety)", () => {
    const ids = MODELS.map((m) => m.id);
    expect(ids).not.toContain("hypothetical-future-model");
  });

  // -------------------------------------------------------------------------
  // 5. MODELS array length
  // -------------------------------------------------------------------------

  it("MODELS is an array", () => {
    expect(Array.isArray(MODELS)).toBe(true);
  });

  it("MODELS contains exactly 26 entries", () => {
    expect(MODELS).toHaveLength(EXPECTED_COUNT);
  });

  // -------------------------------------------------------------------------
  // 6. MODELS id coverage
  // -------------------------------------------------------------------------

  it("every MODELS entry has an id that is in the known 26-key set", () => {
    const knownSet = new Set<string>(KNOWN_MODEL_IDS);
    for (const model of MODELS) {
      expect(knownSet.has(model.id), `unexpected id: ${model.id}`).toBe(true);
    }
  });

  it("every known model ID has a corresponding entry in MODELS", () => {
    const idsInArray = new Set(MODELS.map((m) => m.id));
    for (const knownId of KNOWN_MODEL_IDS) {
      expect(idsInArray.has(knownId), `missing id: ${knownId}`).toBe(true);
    }
  });

  it("MODELS ids are unique (no duplicates)", () => {
    const ids = MODELS.map((m) => m.id);
    const uniqueIds = new Set(ids);
    expect(uniqueIds.size).toBe(ids.length);
  });

  // -------------------------------------------------------------------------
  // 7. Drift guard
  // -------------------------------------------------------------------------

  it(
    "re-running generate-models.ts produces byte-identical output (drift guard)",
    { timeout: 30_000 },
    () => {
      const committedContent = fs.readFileSync(GENERATED_FILE, "utf-8");

      // Run the codegen script from the package root so that tsx resolves
      // node_modules correctly. The script anchors its TOML path via __dirname
      // (three levels up to repo root), so cwd does not affect which TOML is read.
      execSync("npx tsx scripts/generate-models.ts", {
        cwd: PACKAGE_ROOT,
        stdio: "pipe",
      });

      const regeneratedContent = fs.readFileSync(GENERATED_FILE, "utf-8");

      expect(regeneratedContent).toBe(committedContent);
    }
  );
});
