<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# plans

## Purpose
Implementation plans authored via the `superpowers:writing-plans` skill. High-traffic reference — `2026-04-10-go-sdk-signing-support.md` is the canonical source of context for the in-flight Go SDK signing work.

## Key Files
| File | Description |
|------|-------------|
| `2026-04-10-go-sdk-signing-support.md` | **Active** — Go SDK signing implementation plan (high-traffic: referenced ~200× in project memory) |
| `2026-04-12-load-testing.md` | Load-testing execution plan |
| `2026-04-11-solvela-rebrand.md` | RustyClawRouter → Solvela rebrand plan |
| `2026-04-10-load-testing-full-payment-path.md` | Plan for the full payment-path load test |
| `2026-04-10-load-testing-full-payment-path-review.md` | Reviewer notes on the load-testing plan |
| `2026-04-08-escrow-client-sdk-support.md` | Escrow client-side SDK support |
| `2026-04-07-escrow-mainnet-deployment.md` | Escrow mainnet deploy plan |
| `2026-03-31-litesvm-escrow-tests.md` | LiteSVM-based escrow test coverage |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- **Before touching Go SDK signing code, read `2026-04-10-go-sdk-signing-support.md` cover-to-cover.** It's the single source of truth for that workstream.
- Every plan should follow the `superpowers:writing-plans` structure — goal, dependencies, credentials, review checkpoints, "What I need from you".
- Update a plan's Status / Outcome section when work lands; don't delete finished plans (they're history).
- Paired specs (same date) live in `../specs/` — cross-link them.

### Testing Requirements
_(n/a — editorial)_

### Common Patterns
- Dated filenames, one initiative per file.
- Reviewer checkpoints explicit and named.

## Dependencies

### Internal
- Code modules and migration files referenced per-plan.
- `../specs/` for paired design specs.

### External
- `superpowers:writing-plans` skill.

<!-- MANUAL: -->
