#!/usr/bin/env node
// Solvela CLI — platform shim
// Detects the current OS + arch, resolves the native binary from the matching
// optional-dependency package (@solvela/cli-<platform>-<arch>), and execs it
// with the original argv.  Pattern: biome, turborepo, esbuild.

"use strict";

const { execFileSync } = require("child_process");
const fs = require("fs");
const path = require("path");

// Map Node's process.platform + process.arch to our package naming convention.
// Node values: https://nodejs.org/api/process.html#processplatform
const PLATFORM_MAP = {
  linux: { x64: "@solvela/cli-linux-x64" },
  win32: { x64: "@solvela/cli-win32-x64" },
  darwin: { x64: "@solvela/cli-darwin-x64", arm64: "@solvela/cli-darwin-arm64" },
};

const SUPPORTED = [
  "linux/x64  → @solvela/cli-linux-x64",
  "win32/x64  → @solvela/cli-win32-x64",
  "darwin/x64 → @solvela/cli-darwin-x64",
  "darwin/arm64 → @solvela/cli-darwin-arm64",
];

function getPlatformPackage() {
  const plat = process.platform;
  const arch = process.arch;
  const archMap = PLATFORM_MAP[plat];
  if (archMap) {
    const pkg = archMap[arch];
    if (pkg) return pkg;
  }
  return null;
}

function getBinaryPath(platformPkg) {
  // The platform package ships the binary at bin/solvela (or bin/solvela.exe).
  const binaryName = process.platform === "win32" ? "solvela.exe" : "solvela";
  try {
    // require.resolve locates the package root; we navigate to bin/ from there.
    const pkgRoot = path.dirname(
      require.resolve(path.join(platformPkg, "package.json"))
    );
    const binPath = path.join(pkgRoot, "bin", binaryName);
    // require.resolve only proves package.json exists — it says nothing about
    // whether the binary itself was extracted. Two real states leave the bin
    // missing while package.json is present:
    //   1. Dev state after `git clone`: platforms/<plat>/bin/ contains only
    //      .gitkeep until verify-release.sh (or a release build) runs.
    //   2. Corrupt/interrupted `npm install`: package.json is on disk but the
    //      bin/ payload was never extracted.
    // Without this check, execFileSync would throw a generic ENOENT and skip
    // the actionable "Try: npm install" guidance below.
    if (!fs.existsSync(binPath)) return null;
    return binPath;
  } catch (_) {
    return null;
  }
}

function main() {
  const platformPkg = getPlatformPackage();

  if (!platformPkg) {
    process.stderr.write(
      [
        `[solvela] Unsupported platform: ${process.platform}/${process.arch}`,
        "",
        "Supported platforms:",
        ...SUPPORTED.map((s) => `  ${s}`),
        "",
        "To install from source: cargo install --git https://github.com/solvela-ai/solvela solvela-cli",
        "",
      ].join("\n")
    );
    process.exit(1);
  }

  const binaryPath = getBinaryPath(platformPkg);

  if (!binaryPath) {
    process.stderr.write(
      [
        `[solvela] Could not resolve native binary from '${platformPkg}'.`,
        "",
        "This usually means the optional dependency was not installed.",
        "Try: npm install (or pnpm install / yarn install)",
        "",
        `Expected package: ${platformPkg}`,
        "",
      ].join("\n")
    );
    process.exit(1);
  }

  try {
    execFileSync(binaryPath, process.argv.slice(2), { stdio: "inherit" });
  } catch (err) {
    // execFileSync throws with .status when the child exits non-zero.
    // Pass through the exit code so callers see the real code.
    process.exit(typeof err.status === "number" ? err.status : 1);
  }
}

main();
