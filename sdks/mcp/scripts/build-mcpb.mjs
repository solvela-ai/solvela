#!/usr/bin/env node
/**
 * Build the Claude Desktop Extension bundle (.mcpb) for the Solvela MCP server.
 *
 * Steps:
 *   1. esbuild-bundle the tsc output (dist/index.js) into a single CJS file —
 *      no node_modules in the bundle, and the file:../signer-core symlink is
 *      resolved at bundle time.
 *   2. Assemble a staging dir (mcpb-build/) with manifest.json + server/index.js.
 *      The manifest version is overwritten from package.json so it can't drift.
 *   3. `mcpb pack` the staging dir into out/solvela-mcp-<version>.mcpb.
 *
 * Run via: npm run build:mcpb   (prebuild runs tsc first)
 */
import { execFileSync } from 'node:child_process';
import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const staging = join(root, 'mcpb-build');
const outDir = join(root, 'out');

const pkg = JSON.parse(readFileSync(join(root, 'package.json'), 'utf8'));
const manifest = JSON.parse(readFileSync(join(root, 'mcpb', 'manifest.json'), 'utf8'));
manifest.version = pkg.version;

rmSync(staging, { recursive: true, force: true });
mkdirSync(join(staging, 'server'), { recursive: true });
mkdirSync(outDir, { recursive: true });

// 1. Bundle. CJS output: the entry has no top-level await, and CJS avoids the
// ESM/CJS interop hazards of @solana/web3.js's dependency tree.
execFileSync(
  join(root, 'node_modules', '.bin', 'esbuild'),
  [
    join(root, 'dist', 'index.js'),
    '--bundle',
    '--platform=node',
    '--format=cjs',
    '--target=node18',
    '--outfile=' + join(staging, 'server', 'index.js'),
  ],
  { stdio: 'inherit' },
);

// 2. Manifest (version synced from package.json). The bundled package.json pins
// CJS interpretation of server/index.js regardless of where the host extracts
// the bundle (and of any parent package.json with "type": "module").
writeFileSync(join(staging, 'manifest.json'), JSON.stringify(manifest, null, 2) + '\n');
writeFileSync(join(staging, 'package.json'), JSON.stringify({ type: 'commonjs' }, null, 2) + '\n');

// 3. Validate + pack.
const mcpb = join(root, 'node_modules', '.bin', 'mcpb');
execFileSync(mcpb, ['validate', join(staging, 'manifest.json')], { stdio: 'inherit' });
const outFile = join(outDir, `solvela-mcp-${pkg.version}.mcpb`);
execFileSync(mcpb, ['pack', staging, outFile], { stdio: 'inherit' });

console.log(`\nBuilt ${outFile}`);
