# @solvela/cli

Solvela CLI — MCP installer and wallet management for Solana x402 payments.

## Installation

```bash
npm install -g @solvela/cli
```

Or with pnpm / yarn:

```bash
pnpm add -g @solvela/cli
yarn global add @solvela/cli
```

## Usage

```bash
# Check version
solvela --version

# Install the Solvela MCP server into Claude Desktop / Cursor
solvela mcp install

# Wallet management
solvela wallet create
solvela wallet balance

# View available models
solvela models

# Run a chat completion
solvela chat "What is x402?"
```

## How it works

This package is a thin JS shim that detects your platform and architecture,
then delegates to the matching native Rust binary from an optional dependency:

| Platform | Package |
|---|---|
| Linux x64 | `@solvela/cli-linux-x64` |
| Windows x64 | `@solvela/cli-win32-x64` |
| macOS x64 | `@solvela/cli-darwin-x64` |
| macOS ARM64 | `@solvela/cli-darwin-arm64` |

npm/pnpm/yarn automatically install only the package matching your OS/arch
via `optionalDependencies` — no postinstall scripts, no binary downloads.

## Alternative installs

**Shell (Linux/macOS):**
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.solvela.xyz | sh
```

**Cargo:**
```bash
cargo install solvela-cli
```

## License

MIT
