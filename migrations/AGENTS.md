<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# migrations

## Purpose
PostgreSQL SQL migrations. Applied automatically on gateway startup via `run_migrations()` (in `crates/gateway/src/main.rs`). Must be idempotent (`CREATE TABLE IF NOT EXISTS`, `CREATE INDEX IF NOT EXISTS`, `ALTER TABLE … ADD COLUMN IF NOT EXISTS`) so startups on a partially migrated DB are safe. There is no rollback system — forward-only migrations.

## Key Files
| File | Description |
|------|-------------|
| `001_initial_schema.sql` | Baseline — `usage_logs`, `wallet_budgets`, indexes |
| `002_escrow_claim_queue.sql` | `escrow_claim_queue` table — pending claim submissions |
| `003_phase_g_request_session_ids.sql` | Request/session correlation IDs for observability |
| `004_claim_queue_next_retry_at.sql` | Adds `next_retry_at` to `escrow_claim_queue` for backoff-based retries |
| `005_organizations.sql` | Enterprise org/team/member tables + API-key table |
| `006_audit_logs.sql` | Per-org audit-log table |
| `007_hourly_spend_limits.sql` | Adds hourly spend limits alongside existing monthly budgets |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- Naming: `NNN_descriptive_slug.sql`, zero-padded, monotonically increasing.
- **Every** statement must be idempotent: `CREATE TABLE IF NOT EXISTS`, `CREATE INDEX IF NOT EXISTS`, `ALTER TABLE … ADD COLUMN IF NOT EXISTS`, `ALTER TABLE … DROP COLUMN IF EXISTS`.
- Never `DROP TABLE` without an explicit, reviewed migration — production has data.
- Keep each migration file focused; prefer splitting a large change over bundling unrelated edits.
- Large backfills should be a separate migration from the schema change that enables them.

### Testing Requirements
- `cargo test -p gateway` exercises the migration runner against a temporary schema when `DATABASE_URL` is set.
- Locally: `docker compose up -d` + restart the gateway runs them end-to-end.

### Common Patterns
- Use UUIDs for primary keys (`uuid::Uuid::new_v4()` on the Rust side).
- Index every foreign-key column.
- Use `TIMESTAMPTZ` (not `TIMESTAMP`) for any time column.

## Dependencies

### Internal
- Executed by `crates/gateway/src/main.rs::run_migrations`.

### External
- PostgreSQL 16 (see `docker-compose.yml`), `sqlx` for execution.

<!-- MANUAL: -->
