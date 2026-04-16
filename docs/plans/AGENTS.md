<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# plans

## Purpose
Implementation plans — one file per initiative. Reflect the state of planning for cross-cutting work (phase work, rebrands, protocol migrations, competitive analyses). Undated files are evergreen; dated files are point-in-time artefacts.

## Key Files
Representative samples (current as of 2026-04-16):

| File | Description |
|------|-------------|
| `claude-mem.md` | Ongoing Claude memory integration plan |
| `2026-04-13-frontend-redesign.md` | Dashboard redesign plan |
| `2026-04-09-solana-dev-distribution.md` | Solana-dev skill distribution plan |
| `2026-04-04-x402-v2-migration.md` | x402 v2 protocol migration |
| `2026-03-11-phase-f-sdks.md` | Phase F — client SDK rollout |
| `2026-03-10-extract-rustyclaw-protocol.md` | Protocol extraction into a standalone crate |
| `2026-03-09-sse-heartbeat-provider-failover.md` | Provider failover + SSE heartbeats |
| `2026-03-09-ecosystem-upgrade-plan.md` | Ecosystem-level dep upgrade planning |
| `2026-03-08-blockrun-competitive-analysis.md` | BlockRun competitor analysis |
| `phase-8-escrow-hardening.md` | Phase 8 escrow work |
| `phase-9-service-marketplace.md` | Phase 9 service marketplace |
| `phase-12-monitoring.md` | Phase 12 monitoring |
| `phase-14-production-hardening.md` | Phase 14 production hardening |
| `phase-g-gateway-changes.md` | Phase G gateway changes |
| `ClawRouter-vs-Solvela-*.md` | Brand / positioning notes from the rebrand era |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- Dated filename convention: `YYYY-MM-DD-slug.md`. Undated slugs exist for phase work (`phase-N-*.md`) and evergreen positioning notes.
- **Update existing plans rather than forking them** when status changes; use a "Status" section at the top that reflects current reality.
- When a plan has landed, record the outcome in `CHANGELOG.md` / `HANDOFF.md` and leave the plan file for history.
- Don't merge duplicate plans on the same topic — consolidate into one and archive the others under a clear redirect.

### Testing Requirements
- None. Review is the gate. For plans that drive code changes, the implementing PR is the proof.

### Common Patterns
- Top of file: Goal, Status, Why now.
- Middle: Design / approach.
- Bottom: Open questions, references, follow-ups.

## Dependencies

### Internal
- Cross-refs to code modules, migration files, and other plans.

### External
_(none)_

<!-- MANUAL: -->
