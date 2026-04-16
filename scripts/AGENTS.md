<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# scripts

## Purpose
Developer and operational helper scripts. Small, self-contained shell/POSIX scripts that wrap repeatable workflows (load generation, one-off maintenance).

## Key Files
| File | Description |
|------|-------------|
| `load-test.sh` | Drives a local or remote gateway with concurrent chat requests for load testing |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- Keep scripts POSIX-sh compatible when possible; use bash-specific features only when necessary and declare `#!/usr/bin/env bash` if so.
- `set -euo pipefail` at the top of every script.
- Don't hardcode gateway URLs or wallet keys — take them from env vars or CLI args with sensible defaults.
- Mark every new script executable (`chmod +x`) and include a `# Usage:` comment block.

### Testing Requirements
No formal tests; validate manually against a dev gateway before merging.

### Common Patterns
- Log to stderr (`>&2`), data to stdout.
- Exit non-zero on any failure.

## Dependencies

### Internal
- Gateway HTTP contract; `solvela` CLI (built from `crates/cli`) where useful.

### External
- `curl`, `jq`, `bash`.

<!-- MANUAL: -->
