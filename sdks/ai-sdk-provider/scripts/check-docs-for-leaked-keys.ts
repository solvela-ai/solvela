#!/usr/bin/env tsx
/**
 * check-docs-for-leaked-keys.ts
 *
 * Scans documentation files for strings that match the base58 pattern used by
 * Solana keypairs and wallet addresses. Exits non-zero if any match is found
 * outside the allow-list so the pre-commit hook and CI fail fast.
 *
 * Path resolution:
 *   Script lives at  sdks/ai-sdk-provider/scripts/
 *   Three ".." walk back to the repo root — same convention as generate-models.ts
 *
 * Files scanned:
 *   - sdks/ai-sdk-provider/README.md
 *   - sdks/ai-sdk-provider/examples/**  (.md, .mdx, .ts, .js — recursive)
 *   - dashboard/content/docs/sdks/ai-sdk.mdx
 *
 * Allow-list:
 *   EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v  (USDC SPL mint — public)
 */

import * as fs from "node:fs";
import * as path from "node:path";
import { fileURLToPath } from "node:url";

// ---------------------------------------------------------------------------
// Path resolution (ESM — no __dirname)
// ---------------------------------------------------------------------------

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const repoRoot = path.resolve(__dirname, "../../..");

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

const BASE58_RE = /[1-9A-HJ-NP-Za-km-z]{44,88}/g;

/**
 * Strings that match the base58 regex but are safe to appear in docs because
 * they are publicly known network constants, not secrets.
 */
const ALLOW_LIST = new Set<string>([
  "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v", // USDC SPL mint (mainnet)
]);

// ---------------------------------------------------------------------------
// File collection
// ---------------------------------------------------------------------------

/**
 * Resolve a path relative to the repo root.
 */
function repo(rel: string): string {
  return path.resolve(repoRoot, rel);
}

/**
 * Recursively collect files under `dir` whose extension is in `exts`.
 * Returns an empty array if the directory does not exist.
 */
function collectFiles(dir: string, exts: ReadonlySet<string>): string[] {
  if (!fs.existsSync(dir)) return [];

  const results: string[] = [];

  function walk(current: string): void {
    const entries = fs.readdirSync(current, { withFileTypes: true });
    for (const entry of entries) {
      const full = path.join(current, entry.name);
      if (entry.isDirectory()) {
        walk(full);
      } else if (entry.isFile()) {
        const ext = path.extname(entry.name).toLowerCase();
        if (exts.has(ext)) {
          results.push(full);
        }
      }
    }
  }

  walk(dir);
  return results;
}

// Static single-file targets (skip if not present — writer may not have
// authored them yet; the scan is idempotent).
const staticFiles: string[] = [
  repo("sdks/ai-sdk-provider/README.md"),
  repo("dashboard/content/docs/sdks/ai-sdk.mdx"),
].filter((f) => fs.existsSync(f));

// Recursive scan of examples/
const EXAMPLE_EXTS = new Set([".md", ".mdx", ".ts", ".js"]);
const exampleFiles = collectFiles(
  repo("sdks/ai-sdk-provider/examples"),
  EXAMPLE_EXTS
);

const allFiles = [...staticFiles, ...exampleFiles];

// ---------------------------------------------------------------------------
// Scanning
// ---------------------------------------------------------------------------

interface LeakReport {
  file: string;
  line: number;
  column: number;
  match: string;
}

function scanFile(filePath: string): LeakReport[] {
  const content = fs.readFileSync(filePath, "utf-8");
  const lines = content.split("\n");
  const reports: LeakReport[] = [];

  for (let lineIdx = 0; lineIdx < lines.length; lineIdx++) {
    const line = lines[lineIdx];
    BASE58_RE.lastIndex = 0;

    let m: RegExpExecArray | null;
    while ((m = BASE58_RE.exec(line)) !== null) {
      const match = m[0];
      if (!ALLOW_LIST.has(match)) {
        reports.push({
          file: filePath,
          line: lineIdx + 1,
          column: m.index + 1,
          match,
        });
      }
    }
  }

  return reports;
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

const allReports: LeakReport[] = [];

for (const file of allFiles) {
  const reports = scanFile(file);
  allReports.push(...reports);
}

if (allReports.length === 0) {
  const scanned = allFiles.length;
  console.log(`[check-docs-for-leaked-keys] OK — ${scanned} file(s) scanned, no leaked keys found.`);
  process.exit(0);
}

// Group by file for readable output.
const byFile = new Map<string, LeakReport[]>();
for (const r of allReports) {
  const existing = byFile.get(r.file);
  if (existing) {
    existing.push(r);
  } else {
    byFile.set(r.file, [r]);
  }
}

console.error("[check-docs-for-leaked-keys] ERROR — potential leaked keys detected:\n");

for (const [file, reports] of byFile) {
  const rel = path.relative(repoRoot, file);
  console.error(`  ${rel}`);
  for (const r of reports) {
    // Truncate long matches for readability but show enough to identify them.
    const preview =
      r.match.length > 20 ? `${r.match.slice(0, 10)}…${r.match.slice(-10)}` : r.match;
    console.error(`    line ${r.line}, col ${r.column}: ${preview} (${r.match.length} chars)`);
  }
  console.error("");
}

console.error(
  `  ${allReports.length} match(es) found. Review each match and either:\n` +
  `    • remove the value from the file, or\n` +
  `    • add it to ALLOW_LIST in scripts/check-docs-for-leaked-keys.ts\n` +
  `      only if it is a publicly-known constant (e.g. a well-known mint address).`
);

process.exit(1);
