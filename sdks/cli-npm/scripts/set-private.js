#!/usr/bin/env node
// Toggle the `private` flag on the @solvela/cli root package and all four
// platform packages in lockstep.
//
// Usage:
//   node scripts/set-private.js true   — set `"private": true` on all 5
//   node scripts/set-private.js false  — remove `"private": true` from all 5
//
// Wired up to npm scripts as `publish:prepare` (false) and `publish:cleanup`
// (true). The flow is:
//
//   npm run publish:prepare        # remove `"private": true` from all 5
//   npm publish --access public    # then publish each package separately
//   npm run publish:cleanup        # restore `"private": true` so the working
//                                  # tree returns to its committed state
//
// Why this exists: an `npm publish` against any of these five packages
// fails with EPRIVATE while `"private": true` is set, but the flag is the
// only thing keeping the working tree from accidentally publishing during
// a release rehearsal or a wrong-directory `npm publish` invocation. Hand-
// editing five files in lockstep is error-prone — one missed flip during
// publish results in three packages going public and the fourth 404'ing as
// an optional dep that the shim then fails-soft on.

"use strict";

const fs = require("fs");
const path = require("path");

const REPO = path.resolve(__dirname, "..");

const PACKAGE_FILES = [
  path.join(REPO, "package.json"),
  path.join(REPO, "platforms", "linux-x64", "package.json"),
  path.join(REPO, "platforms", "win32-x64", "package.json"),
  path.join(REPO, "platforms", "darwin-x64", "package.json"),
  path.join(REPO, "platforms", "darwin-arm64", "package.json"),
];

function parseTarget(arg) {
  if (arg === "true") return true;
  if (arg === "false") return false;
  return null;
}

// Match the literal "private": <bool> field including any inline whitespace,
// so we can replace just that line without going through JSON.parse +
// JSON.stringify. A round-trip through the JSON serializer would expand
// short arrays like `"os": ["darwin"]` onto multiple lines (npm-style
// 2-space indent always splits arrays), churning ~8 lines per platform
// file on every publish cycle. String-level replacement keeps the diff
// surgical: exactly one line changes per file.
const PRIVATE_LINE_RE = /^([ \t]*"private":[ \t]*)(true|false)([ \t]*,?)$/m;

function applyTarget(file, target) {
  const raw = fs.readFileSync(file, "utf8");

  // Sanity-check that the field exists in the expected single-line form
  // before we do any rewriting — guards against unexpectedly-formatted
  // package.json files that this script wasn't built for.
  const match = raw.match(PRIVATE_LINE_RE);
  if (!match) {
    process.stderr.write(
      `[set-private] ERROR: ${file} has no \`"private": <bool>\` line in the expected format.\n` +
        "  This script expects each tracked package.json to declare the field on its own line.\n",
    );
    process.exit(1);
  }

  const before = match[2];
  const after = target ? "true" : "false";

  if (before === after) {
    console.log(`  unchanged       ${path.relative(REPO, file)}`);
    return;
  }

  // Single-line replace; everything else in the file (object key order,
  // array compaction, trailing newlines) is left exactly as it was.
  const updated = raw.replace(PRIVATE_LINE_RE, `$1${after}$3`);
  fs.writeFileSync(file, updated);

  console.log(`  ${before} -> ${after}    ${path.relative(REPO, file)}`);
}

function main() {
  const target = parseTarget(process.argv[2]);
  if (target === null) {
    process.stderr.write("Usage: node scripts/set-private.js <true|false>\n");
    process.exit(1);
  }

  console.log(
    `Setting \`private: ${target}\` on ${PACKAGE_FILES.length} package.json files:`,
  );
  for (const file of PACKAGE_FILES) {
    applyTarget(file, target);
  }
}

main();
