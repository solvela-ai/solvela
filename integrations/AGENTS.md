<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# integrations

## Purpose
External-framework integrations that let third-party agent runtimes pay through the Solvela gateway. Each subdirectory is a standalone package with its own build system.

## Key Files
_(no loose files — see subdirectories)_

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `elizaos/` | ElizaOS plugin (TypeScript) — exposes Solvela as an action + provider (see `elizaos/AGENTS.md`) |
| `openclaw/` | OpenClaw integration (TypeScript) — routes OpenClaw requests through Solvela (see `openclaw/AGENTS.md`) |

## For AI Agents

### Working In This Directory
- Each integration builds independently — don't add a root `package.json` or monorepo tooling here unless the user asks.
- Every integration must keep private keys client-side (the host runtime holds the wallet). The gateway only sees signed transactions.
- New integration targets (AutoGen, CrewAI, LangChain, …) live in their own subdirectory under `integrations/`.

### Testing Requirements
Each subdirectory has its own test command — see its `AGENTS.md`.

### Common Patterns
- Thin adapter pattern: translate host-framework request → OpenAI-compat chat body; handle 402 response by signing + retrying.
- Distribute via npm with a `solvela-*` naming convention.

## Dependencies

### Internal
- The Solvela gateway HTTP contract (`/v1/chat/completions` + x402 `PAYMENT-SIGNATURE` header).
- Client SDKs under `../sdks/` may be used where convenient.

### External
- Host framework runtimes (`@elizaos/core`, OpenClaw packages, etc.).

<!-- MANUAL: -->
