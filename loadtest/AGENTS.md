<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# loadtest

## Purpose
Containerized load-testing harness deployable to Fly.io. Builds an image that drives traffic at the gateway at configurable concurrency/duration, useful for regression and capacity testing.

## Key Files
| File | Description |
|------|-------------|
| `Dockerfile` | Builds the load-test image (based on the `solvela` CLI loadtest subcommand) |
| `fly.toml` | Fly.io app definition — machine size, regions, env vars |
| `run.sh` | Entrypoint shell script — parses env, invokes the loadtest binary |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- Keep the harness stateless — each run is a fresh machine.
- Configure target URL + concurrency + duration via env vars (`SOLVELA_GATEWAY_URL`, `LOADTEST_CONCURRENCY`, `LOADTEST_DURATION_SECS`).
- Prefer the `solvela loadtest` subcommand (from `crates/cli/src/commands/loadtest/`) over ad-hoc scripts — it already uses `hdrhistogram` for latency percentiles.
- Never run a load test against production without explicit approval.

### Testing Requirements
- Manual smoke test locally via `docker build . && docker run … ./run.sh` against a dev gateway.
- Verify latency histograms look reasonable before using results for capacity planning.

### Common Patterns
- Output: hdrhistogram CSV + summary stats to stdout.
- Exit code: 0 on run completion (even if SLOs missed), non-zero only on harness failures.

## Dependencies

### Internal
- `solvela` CLI built from `crates/cli/`.

### External
- Fly.io (for deployed runs), Docker, the upstream gateway being tested.

<!-- MANUAL: -->
