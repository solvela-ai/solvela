<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# operations

## Purpose
Ops handbook — deploy, monitor, scale, and respond to incidents. Fly.io deployment, DB migrations, Redis cache operations, on-call runbooks, balance-monitor thresholds.

## For AI Agents

### Working In This Directory
- Runbooks must be precise and up-to-date — incorrect ops docs are worse than no docs.
- When infra changes (new region, new secret, migration process update), edit here as part of the change.
- Keep thresholds (fee-payer balance alert, rate-limit defaults) mirrored from `config/default.toml` — link to it.

### Testing Requirements
- Dry-run runbooks on staging before relying on them in prod.

### Common Patterns
- Runbook template: symptom → diagnosis → mitigation → follow-up.
- Named infra objects: `solvela-gateway` (Fly app), `solvela.vercel.app` (dashboard), PostgreSQL, Redis.

## Dependencies

### Internal
- `fly.toml`, `docker-compose.yml`, `migrations/`, `config/`.

### External
- Fly.io, Vercel, Solana RPC providers.

<!-- MANUAL: -->
