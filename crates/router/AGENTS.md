<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# router

## Purpose
Smart request router. Provides a 15-dimension rule-based scorer that classifies chat requests into tiers (Simple / Medium / Complex / Reasoning) plus routing profiles (`eco`, `auto`, `premium`, `free`) that map tiers to specific model IDs. Also owns the model registry that loads `config/models.toml` (per-token pricing for 26+ models across 5 providers). Crate name: `solvela-router`. Pure rule-based, <1µs per scoring call, zero external calls.

## Key Files
| File | Description |
|------|-------------|
| `Cargo.toml` | Manifest — minimal deps (serde, toml, thiserror, tracing) |

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `src/` | Scorer + profiles + model registry (see `src/AGENTS.md`) |

## For AI Agents

### Working In This Directory
- Scoring weights in `src/scorer.rs` are tuned — do not rebalance without a paired eval.
- Adding a new routing profile means: extend the profile enum, add a `ProfileConfig`, and wire it into the gateway's request pipeline.
- Keep this crate pure-functional — no HTTP, no tokio runtime dependency beyond plain async traits.

### Testing Requirements
```bash
cargo test -p router
cargo test -p router scorer    # pattern match
```

### Common Patterns
- Scoring is stateless: `(request) → Tier`. No side effects.
- Model prices in `models.toml` are per 1M tokens; convert to per-token inside the registry.

## Dependencies

### Internal
- `solvela-protocol` — request / model types.

### External
- `serde`, `serde_json`, `toml`, `thiserror`, `tracing`.

<!-- MANUAL: -->
