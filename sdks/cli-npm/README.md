# @solvela/cli

Solvela CLI — MCP installer and wallet management for Solana x402 payments.

> **⚠️ Not yet published to npm.** `@solvela/cli` is at `1.0.0-draft` and the package
> is marked `"private": true`. The install commands below will fail until first
> publish lands. For now, build from source: clone `solvela-ai/solvela` and run
> `npm install && npm run build` in `sdks/cli-npm/`. The Rust CLI (same surface,
> different language) is already published as `solvela-cli` on crates.io —
> `cargo install solvela-cli` is the supported install today.

## Installation (once published)

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
solvela wallet init      # generate a new keypair (prints the address to fund)
solvela wallet status    # show address + USDC-SPL balance

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
| Linux ARM64 | `@solvela/cli-linux-arm64` |
| Windows x64 | `@solvela/cli-win32-x64` |
| Windows ARM64 | `@solvela/cli-win32-arm64` |
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

## Documentation

[docs.solvela.ai/sdks/rust](https://docs.solvela.ai/sdks/rust) — full reference for the `solvela` CLI commands and wallet flows (this npm package is a distribution shim around the same Rust binary).

## License

MIT
