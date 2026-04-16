<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# sdks

## Purpose
Per-SDK guides — install, configure, signing, common flows. One page per SDK (Python / TypeScript / Go / MCP).

## For AI Agents

### Working In This Directory
- Keep idiomatic to each language — don't force a single shape on all four SDKs.
- Link to the authoritative README under `../../../sdks/<lang>/README.md`; this handbook should explain concepts + common flows, not re-list every API.
- When an SDK ships a breaking change, update the relevant page before the next release.

### Testing Requirements
- Walk through each SDK's examples on a clean install when making changes.

### Common Patterns
- Quickstart → config → first paid call → error handling → streaming.

## Dependencies

### Internal
- `../../../sdks/<lang>/` per-SDK source and README.

### External
- Each SDK's package registry page (npm / PyPI / Go modules).

<!-- MANUAL: -->
