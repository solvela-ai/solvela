<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# src

## Purpose
ElizaOS plugin source. `index.ts` is the plugin export; `actions/` contains ElizaOS action definitions; `providers/` contains the gateway provider exposed to the agent runtime.

## Key Files
| File | Description |
|------|-------------|
| `index.ts` | Plugin entrypoint — exports `actions` + `providers` to ElizaOS |

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `actions/` | ElizaOS actions (see `actions/AGENTS.md`) |
| `providers/` | ElizaOS providers (see `providers/AGENTS.md`) |

## For AI Agents

### Working In This Directory
- Keep `index.ts` to a flat export; do not put business logic here.
- Any new public API (action or provider) must be added to `index.ts`'s exports.

### Testing Requirements
```bash
npm --prefix integrations/elizaos test
```

### Common Patterns
- One file per action / provider; named exports.

## Dependencies

### Internal
- `./actions`, `./providers`.

### External
- `@elizaos/core`.

<!-- MANUAL: -->
