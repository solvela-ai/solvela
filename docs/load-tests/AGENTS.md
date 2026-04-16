<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# load-tests

## Purpose
Load-test results and analyses. Plans for load testing live under `../superpowers/plans/`; this directory captures the actual runs and what they taught us.

## Key Files
| File | Description |
|------|-------------|
| `2026-04-12-results.md` | Most recent full payment-path load-test results with latency + throughput + observations |

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `results/` | Raw output from individual runs (histograms, logs) |

## For AI Agents

### Working In This Directory
- Dated filename per run summary: `YYYY-MM-DD-results.md`.
- Capture: concurrency profile, duration, gateway commit SHA, environment, p50/p95/p99 latency, error rates, notable failures.
- Link to the raw data under `results/` and to the plan in `../superpowers/plans/`.
- Don't edit old results — write a new dated file if you re-run.

### Testing Requirements
_(n/a — this directory describes tests, doesn't run them)_

### Common Patterns
- Summary at the top; raw numbers below; interpretation at the bottom.

## Dependencies

### Internal
- `../superpowers/plans/` for the plans that motivated each run.
- `../../loadtest/` for the harness config.

### External
- `hdrhistogram` output, Grafana exports.

<!-- MANUAL: -->
