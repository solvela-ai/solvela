<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# src

## Purpose
CLI source tree. `main.rs` is the clap entry point; individual subcommands live under `commands/`.

## Key Files
| File | Description |
|------|-------------|
| `main.rs` | clap top-level parser, subcommand dispatch, global options (`--gateway`, config path, wallet) |

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `commands/` | One module per subcommand (see `commands/AGENTS.md`) |

## For AI Agents

### Working In This Directory
- Register new subcommands in `commands/mod.rs` and wire them into `main.rs`'s dispatch match.
- Errors bubble up with `anyhow::Result<()>` from `main`; user-facing messages are printed via `eprintln!` before exiting.
- Tokio runtime is spun up in `main` with `#[tokio::main]`.

### Testing Requirements
```bash
cargo test -p solvela-cli
```

### Common Patterns
- One subcommand = one file under `commands/`, exposing `pub async fn run(args: …) -> Result<()>`.
- Output formatting: plain text by default; opt-in `--json` flag where it makes sense.

## Dependencies

### Internal
- `x402`, `solvela-router`.

### External
- `clap`, `tokio`, `reqwest`, `anyhow`.

<!-- MANUAL: -->
