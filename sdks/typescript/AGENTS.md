<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# typescript

## Purpose
TypeScript/Node SDK for Solvela. Published to npm. Exposes a `Client`, local wallet support, x402 flow, and an OpenAI-compat shim so existing OpenAI SDK consumers can point at Solvela with minimal code changes.

## Key Files
| File | Description |
|------|-------------|
| `README.md` | Installation + quickstart |
| `package.json` | NPM manifest — entry points, scripts, deps |
| `package-lock.json` | Pinned dependency tree |
| `tsconfig.json` | TypeScript compiler config |

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `src/` | Library source (see `src/AGENTS.md`) |
| `tests/` | Test suite (see `tests/AGENTS.md`) |
| `dist/` | Compiled output (not checked in) |
| `node_modules/` | Installed deps (not checked in) |

## For AI Agents

### Working In This Directory
- Published artifact is the `dist/` built from `src/`; never hand-edit `dist/`.
- Keep type definitions exported so consumers get IDE autocomplete.
- Keep the private-key surface minimal — accept a `Uint8Array` or base58 string, never a file path the library reads on its own.

### Testing Requirements
```bash
npm --prefix sdks/typescript test
```

### Common Patterns
- Discriminated unions for errors (`{ kind: "payment-required"; body: … } | …`).
- Fetch API (Node 20+) rather than axios.

## Dependencies

### Internal
- Solvela gateway HTTP contract.

### External
- `@solana/web3.js` (or `@solana/kit`-derived helpers — check `package.json`), `tweetnacl` or `@noble/ed25519`.

<!-- MANUAL: -->
