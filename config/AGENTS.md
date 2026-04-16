<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# config

## Purpose
Static TOML configuration consumed by the gateway at startup. Non-secret values only — API keys and signing keys always come from environment variables (`SOLVELA_*` or provider-specific like `OPENAI_API_KEY`).

## Key Files
| File | Description |
|------|-------------|
| `default.toml` | Server defaults — host/port, Solana RPC URL, monitor thresholds, CORS, rate-limit defaults |
| `models.toml` | Model registry — per-model ID, provider, context window, per-token pricing (input / output / cached). Loaded by `crates/router/src/models.rs`. Covers 5 providers, 26+ models |
| `services.toml` | x402 service marketplace registry — service id, description, price, endpoint metadata. Loaded by `crates/gateway/src/services.rs` |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- **Never put secrets in these files** — API keys, fee-payer keypairs, database URLs all live in env vars. Custom `Debug` redaction in `gateway::config` enforces this on the code side; keep the config files clean.
- Adding a model: edit `models.toml` with id, provider, context window, and pricing per 1M tokens (input/output/cached). Restart the gateway to pick it up.
- Pricing is in atomic USDC units when loaded; `models.toml` uses a human-readable per-1M-token figure.
- Service-marketplace changes in `services.toml` propagate to `GET /services`.

### Testing Requirements
```bash
cargo test -p solvela-router    # loads models.toml in tests
cargo test -p gateway services  # loads services.toml
```

### Common Patterns
- TOML keys use snake_case.
- Every model entry has: `id`, `provider`, `display_name`, `context_window`, `input_price`, `output_price`, optional `cached_input_price`, capability flags.

## Dependencies

### Internal
- Consumed by `crates/router/src/models.rs` and `crates/gateway/src/services.rs`.

### External
_(none — static data)_

<!-- MANUAL: -->
