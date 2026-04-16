<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# src

## Purpose
OpenClaw integration source. Entry point, a typed config loader, and the request router that forwards calls to Solvela.

## Key Files
| File | Description |
|------|-------------|
| `index.ts` | Package entry — exports the plugin/factory to OpenClaw hosts |
| `config.ts` | Typed config: gateway URL, default model, wallet key source, timeouts |
| `router.ts` | Forwards an OpenClaw request to `/v1/chat/completions`, handles 402 with sign-and-retry |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- Keep `index.ts` a flat export surface.
- `config.ts` is the only place env vars are read; the rest of the code takes a typed config object.
- Never log signed transactions or private-key material.

### Testing Requirements
```bash
npm --prefix integrations/openclaw test
```

### Common Patterns
- Separate pure transform functions (no IO) from the network-calling `router`.

## Dependencies

### Internal
- Solvela gateway HTTP.

### External
- OpenClaw runtime (see `../package.json`).

<!-- MANUAL: -->
