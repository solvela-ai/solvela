<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# commands

## Purpose
Per-subcommand modules for the `solvela` CLI. Each file is responsible for one top-level command's argument parsing (via nested clap `Args`), business logic, and output formatting.

## Key Files
| File | Description |
|------|-------------|
| `mod.rs` | Re-exports each subcommand module and defines the top-level `Command` enum |
| `chat.rs` | `solvela chat` — send a prompt, pay via x402, stream or print the response |
| `wallet.rs` | `solvela wallet` — generate / import / inspect a Solana wallet keypair |
| `models.rs` | `solvela models` — list available models, profiles, and their pricing |
| `health.rs` | `solvela health` — probe the gateway `/health` endpoint |
| `stats.rs` | `solvela stats` — per-wallet usage stats (requires gateway DB) |
| `doctor.rs` | `solvela doctor` — diagnostic checks (gateway reachable, RPC OK, wallet balance, fee-payer configured) |
| `recover.rs` | `solvela recover` — rescue stuck escrow or payment state |
| `solana_tx.rs` | Shared helpers for building / signing Solana transactions used by multiple subcommands |

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `loadtest/` | Subcommand implementation for `solvela loadtest` (concurrent load generator) |

## For AI Agents

### Working In This Directory
- Each command file owns its own `Args` struct; keep them self-contained.
- Shared building blocks (transaction construction, wallet loading) belong in `solana_tx.rs`, not duplicated.
- Never print raw private-key material. Use the `secrecy` crate and `Debug` redactions.
- For network calls, use a shared `reqwest::Client` with sensible timeouts.

### Testing Requirements
```bash
cargo test -p solvela-cli
```
Gateway HTTP is mocked with `wiremock` in tests.

### Common Patterns
- `async fn run(args: Args, cfg: Config) -> anyhow::Result<()>` as the command entry signature.
- Display output using `println!` for success, `eprintln!` for diagnostics.

## Dependencies

### Internal
- `x402` — payment signing and facilitator calls.
- `solvela-router` — model lookup.

### External
- `clap`, `reqwest`, `tokio`, `anyhow`, `base64`, `bs58`, `ed25519-dalek`, `secrecy`, `hdrhistogram`.

<!-- MANUAL: -->
