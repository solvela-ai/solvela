<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# orgs

## Purpose
Enterprise org/team/API-key data model and queries. Provides the building blocks used by `middleware::api_key` (to attach `OrgContext` to a request) and by `routes::orgs` (to expose CRUD over HTTP). Backed by PostgreSQL tables declared in `migrations/005_organizations.sql` and related migrations.

## Key Files
| File | Description |
|------|-------------|
| `mod.rs` | Module root; re-exports models and queries |
| `models.rs` | `Organization`, `Team`, `ApiKey`, `Membership`, role enums, budget limits |
| `queries.rs` | Typed sqlx queries — `find_by_api_key`, `list_teams`, `create_api_key`, budget reads, etc. |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- Schema changes live in `migrations/` — every new column needs a migration file and a `CREATE … IF NOT EXISTS` or `ALTER TABLE … IF NOT EXISTS` guard (migrations are idempotent and applied automatically on startup).
- API-key hashing is done here; never store plaintext keys. Prefix lookups by a key prefix stored alongside the hash.
- Role checks (`admin`, `member`) happen in `middleware::api_key` — keep the query layer role-agnostic.
- When adding a new entity, prefer separate tables over JSONB blobs — audit, budget, and analytics reads benefit from structured columns.

### Testing Requirements
```bash
cargo test -p gateway orgs
```
Integration tests in `../../tests/integration.rs` use a temporary schema.

### Common Patterns
- `sqlx::query_as!` macros where feasible; fall back to `sqlx::query_as::<_, T>(…)` for dynamic SQL.
- UUID v4 primary keys (`uuid::Uuid::new_v4()`).
- Fire-and-forget writes via `tokio::spawn` — never block the request hot path on audit inserts.

## Dependencies

### Internal
- `crate::audit` for audit-log emission.

### External
- `sqlx`, `uuid`, `chrono`, `sha2`, `serde`.

<!-- MANUAL: -->
