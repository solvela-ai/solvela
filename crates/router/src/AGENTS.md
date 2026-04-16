<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# src

## Purpose
Implementation of the smart router. Three modules: `scorer` (classifies requests into tiers by 15 weighted dimensions), `profiles` (maps tiers → model IDs per routing profile), and `models` (loads and queries `config/models.toml`).

## Key Files
| File | Description |
|------|-------------|
| `lib.rs` | Module declarations |
| `scorer.rs` | 15-dimension rule-based scorer → `Tier { Simple, Medium, Complex, Reasoning }` |
| `profiles.rs` | Routing profiles: `eco`, `auto`, `premium`, `free` + tier→model mapping |
| `models.rs` | Model registry — loads `config/models.toml`, exposes pricing and capability lookups |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- Scoring is pure: no IO, no async, no mutation. Keep it that way — it runs on every request hot path.
- New dimensions require: (a) add a weighted feature, (b) add tests in the same file, (c) update `scorer` doc comments with the weight and rationale.
- Profiles are static at startup — reloading models.toml requires a gateway restart.
- Pricing uses atomic USDC units (u64, 6 decimals). Never f64.

### Testing Requirements
```bash
cargo test -p router
```
Each dimension should have focused unit tests with short example prompts.

### Common Patterns
- `thiserror` for error enums (e.g., `ModelError`, `ProfileError`).
- Model IDs are canonical strings from `models.toml` — never hardcode model IDs in the scorer.

## Dependencies

### Internal
- `solvela-protocol` — chat request types, cost types.

### External
- `serde`, `serde_json`, `toml`, `thiserror`, `tracing`.

<!-- MANUAL: -->
