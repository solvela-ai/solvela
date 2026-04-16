<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# openclaw

## Purpose
OpenClaw integration — routes OpenClaw-framework requests through Solvela. TypeScript package with its own build + test pipeline.

## Key Files
| File | Description |
|------|-------------|
| `package.json` | NPM manifest |
| `package-lock.json` | Locked dependency tree |
| `tsconfig.json` | TypeScript compiler config |

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `src/` | Integration source (see `src/AGENTS.md`) |
| `tests/` | Unit tests (see `tests/AGENTS.md`) |
| `dist/` | Compiled output (not checked in) |
| `node_modules/` | Installed dependencies (not checked in) |

## For AI Agents

### Working In This Directory
- Keep the package standalone — no root-repo dependency sharing.
- Do not check `dist/` or `node_modules/` into git.
- Configuration surface lives in `src/config.ts`; new options go there.

### Testing Requirements
```bash
cd integrations/openclaw && npm install && npm test
```

### Common Patterns
- Same shape as `elizaos/`: thin router over the Solvela HTTP API with x402 retry on 402.

## Dependencies

### Internal
- Solvela gateway HTTP contract.

### External
- OpenClaw runtime packages (see `package.json`).

<!-- MANUAL: -->
