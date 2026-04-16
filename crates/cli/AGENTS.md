<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# cli

## Purpose
The `solvela` command-line tool. A `clap`-derive binary that lets users manage a Solana wallet, send chat requests that pay via x402, inspect available models, run health checks, view usage stats, and diagnose their local setup (`doctor`). Binary name: `solvela`. Crate name: `solvela-cli`.

## Key Files
| File | Description |
|------|-------------|
| `Cargo.toml` | Manifest — `[[bin]]` name = "solvela"; dev-deps include `wiremock` for HTTP mocking |

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `src/` | CLI implementation (see `src/AGENTS.md`) |

## For AI Agents

### Working In This Directory
- New subcommands go in `src/commands/<name>.rs` and are registered in `src/commands/mod.rs`.
- Signing happens client-side — the CLI holds the private key, the gateway never sees it.
- Secrets (private keys loaded from disk) must pass through the `secrecy` crate — never log them, never print them.
- `anyhow` is allowed here (binary), but prefer typed errors for library-level helpers.

### Testing Requirements
```bash
cargo test -p solvela-cli
cargo test -p solvela-cli -- --nocapture
```
HTTP integration tests use `wiremock` to simulate gateway responses.

### Common Patterns
- `clap` derive API with nested subcommands.
- Reqwest client configured with a short default timeout.
- Base58-encoded keys / signatures on the wire.

## Dependencies

### Internal
- `x402` — payment signing
- `solvela-router` — model-registry lookups

### External
- `clap`, `reqwest`, `tokio`, `anyhow`, `base64`, `bs58`, `ed25519-dalek`, `secrecy`, `hdrhistogram` (for loadtest output).

<!-- MANUAL: -->
