#!/usr/bin/env node
// Build sdks/signer-core/dist if missing.
//
// Called by consumer packages (sdks/mcp, sdks/openclaw-provider, sdks/ai-sdk-provider)
// as a prebuild / pretest hook because npm does not auto-build `file:` deps. CI handles
// this with explicit workflow steps; this script gives local dev the same behavior so
// `npm --prefix sdks/<consumer> run build` works on a fresh clone without a manual
// `cd ../signer-core && npm ci && npm run build` dance first.
//
// Fast path: if `dist/` already exists, exits silently. To force a rebuild, run
// `npm run build` in sdks/signer-core/ directly or delete its dist directory.
import { existsSync } from 'node:fs';
import { execSync } from 'node:child_process';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
if (existsSync(resolve(root, 'dist'))) process.exit(0);

process.stderr.write('[ensure-built] Bootstrapping @solvela/signer-core (file: dep — npm does not auto-build)…\n');
execSync('npm ci --no-audit --no-fund && npm run build', { cwd: root, stdio: 'inherit' });
