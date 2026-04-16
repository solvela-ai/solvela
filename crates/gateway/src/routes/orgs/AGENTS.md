<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# orgs

## Purpose
HTTP endpoints for the enterprise org/team/API-key system. Every route is gated by an `OrgContext` extractor (`RequireOrg` or `RequireOrgAdmin`) populated by `middleware::api_key`. Routes never read org info from query params or request bodies — always from extensions.

## Key Files
| File | Description |
|------|-------------|
| `mod.rs` | Module root; wires the routes into an `axum::Router` with the correct extractor gates |
| `crud.rs` | Create/read/update/delete for organizations themselves |
| `teams.rs` | Team CRUD within an organization |
| `api_keys.rs` | API-key issuance, listing, revocation (stores only hashes + prefixes) |
| `audit.rs` | Read-only audit-log queries scoped to the caller's org |
| `budget.rs` | Per-wallet hourly / monthly budget reads and writes (migration 007 added hourly limits) |
| `analytics.rs` | Usage aggregations (per-member, per-model, per-time-window) |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- Extractors, not query params: handler signature is `pub async fn foo(org: RequireOrg, State(app): State<…>, …)`.
- Admin-only endpoints use `RequireOrgAdmin`; never check roles manually in the handler.
- API-key endpoints must hash + prefix keys at write time (hashing lives in `crate::orgs::queries`).
- Every mutating endpoint should emit an audit-log row via `crate::audit` (fire-and-forget).

### Testing Requirements
```bash
cargo test -p gateway orgs
cargo test -p gateway --test integration
```

### Common Patterns
- `sqlx` queries go through `crate::orgs::queries`; handlers should not write SQL directly.
- Paginated reads use consistent `?limit=&cursor=` params.

## Dependencies

### Internal
- `crate::orgs` (models + queries), `crate::middleware::api_key`, `crate::audit`, `crate::error`.

### External
- `axum`, `serde`, `serde_json`, `sqlx`, `uuid`, `chrono`.

<!-- MANUAL: -->
