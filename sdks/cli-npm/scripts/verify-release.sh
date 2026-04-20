#!/usr/bin/env bash
# T4-G.5 — Local smoke test for the @solvela/cli npm shim.
#
# What this does:
#   1. Builds the solvela-cli Rust binary for the current host platform.
#   2. Copies the binary into the matching platforms/<platform>/bin/ directory.
#   3. Runs the JS shim (bin/solvela.js) with --version to confirm end-to-end resolution.
#
# What this does NOT do:
#   - Tag a release
#   - Publish to npm
#   - Upload to GitHub Releases
#   - Trigger any CI workflow
#
# Usage:
#   cd /path/to/solvela
#   bash sdks/cli-npm/scripts/verify-release.sh

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
CLI_NPM="$REPO_ROOT/sdks/cli-npm"

# ── 1. Detect host platform ───────────────────────────────────────────────
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS/$ARCH" in
  Linux/x86_64)
    PLATFORM_PKG="linux-x64"
    BINARY_NAME="solvela"
    ;;
  Darwin/x86_64)
    PLATFORM_PKG="darwin-x64"
    BINARY_NAME="solvela"
    ;;
  Darwin/arm64)
    PLATFORM_PKG="darwin-arm64"
    BINARY_NAME="solvela"
    ;;
  *)
    echo "[verify-release] ERROR: Unsupported host platform: $OS/$ARCH"
    echo "  Supported: Linux/x86_64, Darwin/x86_64, Darwin/arm64"
    echo "  (Windows verification must run in PowerShell or WSL with cross-compile)"
    exit 1
    ;;
esac

echo "[verify-release] Host platform: $OS/$ARCH → $PLATFORM_PKG"

# ── 2. Build the release binary ──────────────────────────────────────────
echo "[verify-release] Building solvela-cli (release)..."
cargo build --release -p solvela-cli --manifest-path "$REPO_ROOT/Cargo.toml"

SOURCE_BIN="$REPO_ROOT/target/release/$BINARY_NAME"
if [ ! -f "$SOURCE_BIN" ]; then
  echo "[verify-release] ERROR: Expected binary not found at: $SOURCE_BIN"
  exit 1
fi

# ── 3. Copy binary into the platform package ──────────────────────────────
DEST_DIR="$CLI_NPM/platforms/$PLATFORM_PKG/bin"
DEST_BIN="$DEST_DIR/$BINARY_NAME"

echo "[verify-release] Copying binary to: $DEST_BIN"
mkdir -p "$DEST_DIR"
cp "$SOURCE_BIN" "$DEST_BIN"
chmod +x "$DEST_BIN"

# ── 4. Wire up node_modules so require.resolve works ──────────────────────
# The shim uses require.resolve('@solvela/cli-<platform>/package.json').
# Without npm install, we symlink the platform package into node_modules.
NODE_MODULES="$CLI_NPM/node_modules/@solvela"
mkdir -p "$NODE_MODULES"

PKG_NODE_NAME="cli-$PLATFORM_PKG"
LINK_TARGET="$NODE_MODULES/$PKG_NODE_NAME"

if [ -L "$LINK_TARGET" ]; then
  rm "$LINK_TARGET"
fi
ln -sf "$CLI_NPM/platforms/$PLATFORM_PKG" "$LINK_TARGET"
echo "[verify-release] Linked: $LINK_TARGET -> $CLI_NPM/platforms/$PLATFORM_PKG"

# ── 5. Run the shim ───────────────────────────────────────────────────────
echo "[verify-release] Running: node $CLI_NPM/bin/solvela.js --version"
echo "---"
node "$CLI_NPM/bin/solvela.js" --version
echo "---"

echo "[verify-release] SUCCESS: shim correctly resolved and executed the native binary."
echo ""
echo "  Binary: $DEST_BIN"
echo "  Shim:   $CLI_NPM/bin/solvela.js"
echo ""
echo "  This was a LOCAL smoke test only."
echo "  No release was tagged. No publish was triggered."
