# Fix Migration Runner Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the broken inline `MIGRATION_SQL` in `run_migrations()` with `sqlx::migrate!` so all seven migration files in `migrations/` apply correctly on every gateway startup, and reconcile the schema drift that left the freshly-created `solvela-db` cluster missing most expected tables and columns.

**Architecture:** Swap the hand-rolled `sqlx::raw_sql(MIGRATION_SQL)` call for `sqlx::migrate!("../../migrations").run(&pool).await?`. The sqlx macro embeds all seven SQL files into the binary at compile time and tracks applied versions in a `_sqlx_migrations` table. Before the first deploy of the fixed runner, run a one-shot DDL cleanup on `solvela-db` to drop the legacy `spend_logs` + `wallet_budgets` tables that the old `MIGRATION_SQL` created — those two tables have a drift (missing `updated_at` on `wallet_budgets`, among other mismatches) that a fresh migration pass reconciles cleanly because the DB currently has zero user rows.

**Tech Stack:** Rust 2021 · sqlx 0.8 (runtime-tokio, postgres, macros) · tokio · thiserror · PostgreSQL 17 (Fly postgres-flex) · Fly.io CLI for ops steps.

---

## Background

### What broke
`crates/gateway/src/main.rs:689` defines `run_migrations()` like this:

```rust
async fn run_migrations(pool: &sqlx::PgPool) -> anyhow::Result<()> {
    sqlx::raw_sql(gateway::usage::MIGRATION_SQL)
        .execute(pool)
        .await?;
    info!("database migrations applied");
    Ok(())
}
```

The `MIGRATION_SQL` constant in `crates/gateway/src/usage.rs:889-920` contains a hand-rolled version of migration 001 plus the additions from migration 003 (`request_id`, `session_id` columns on `spend_logs`). It does **not** include migrations 002, 004, 005, 006, or 007. Every other migration file in `migrations/` is referenced only by unit tests (via `include_str!`) and has never been executed against any production, staging, or local Fly database.

As a result, the production DB as of 2026-04-17 contains only two tables: `spend_logs` and `wallet_budgets`. Every feature that reads or writes `organizations`, `teams`, `org_members`, `team_wallets`, `api_keys`, `audit_logs`, `escrow_claim_queue`, `team_budgets`, `hourly_limit_usdc` on `wallet_budgets`, or `updated_at` on `wallet_budgets` either 500s immediately or silently fails a fire-and-forget write that never hits the DB.

### Schema drift on the live solvela-db

Because `MIGRATION_SQL` ran during the gateway's startup after the DB rename (2026-04-17), the fresh `solvela-db` cluster now has these rows already:

- `spend_logs` — exists with `request_id` + `session_id` columns (matches MIGRATION_SQL, differs from file `001` which doesn't have them — file `003` would add them).
- `wallet_budgets` — exists **without** `updated_at` (MIGRATION_SQL omits it, file `001` includes it) and **without** `hourly_limit_usdc` (file `007` would add it).
- No triggers, no functions, no other tables.
- Both tables have 0 rows.

If we simply bolt `sqlx::migrate!` on top of this state, migration 001's `CREATE TABLE IF NOT EXISTS wallet_budgets (..., updated_at ...)` is a no-op (table exists), the trigger it creates fires `NEW.updated_at = NOW()` against a column that doesn't exist, and every UPDATE on `wallet_budgets` breaks.

The fix: drop the two legacy tables on `solvela-db` once, then let `sqlx::migrate!` rebuild them from files `001`–`007`. Zero data loss (0 rows) and zero ambiguity about final schema.

### Scope of this plan
1. Replace the migration runner with `sqlx::migrate!`.
2. Remove the inline `MIGRATION_SQL` constant and the tests that reference it.
3. Add an integration test that applies all seven migrations to a live local Postgres and verifies the expected schema.
4. Run a one-shot `DROP TABLE` on `solvela-db` to clear the drifted legacy tables.
5. Deploy to `solvela-gateway`; on first startup, the new runner applies all seven migrations cleanly.
6. Verify the new state end-to-end: tables exist, one org endpoint returns a 200/4xx (not 500).
7. Update `HANDOFF.md` and `crates/gateway/src/AGENTS.md` to reflect the fix.

### Non-goals (explicit)
- **No changes to migration files 001–007.** They're all idempotent (`CREATE TABLE IF NOT EXISTS`, `ADD COLUMN IF NOT EXISTS`, `DROP TRIGGER IF EXISTS ... CREATE TRIGGER`). They apply cleanly to a fresh DB in order.
- **No changes to the org/team/api_key handler code.** Only the schema-plumbing bug is in scope. Whether `/v1/orgs/...` endpoints are feature-complete at the handler layer is a separate initiative.
- **No backfill.** The live DB has 0 user rows across both existing tables. Any future data migration concerns are out of scope because there is no data to migrate.

---

## What I need from you

- **Fly CLI access** with permissions on `solvela-gateway` + `solvela-db` (same token already used for the rename migration works; or a fresh org-level access token).
- **~3 minutes of rolling-restart downtime** on `solvela-gateway`. During the restart, one machine serves while the other restarts with the new binary and the migration runs on first connect.
- **Local Postgres available** for the integration test: `docker compose up -d` gives you `postgres://postgres:postgres@localhost:5432/solvela` per `docker-compose.yml`.
- **Rust toolchain** with `cargo` for the code changes + tests (you already have this).
- **Approval for one-shot DDL on solvela-db** — specifically `DROP TABLE IF EXISTS spend_logs, wallet_budgets CASCADE`. This is destructive, but targets a DB with 0 user rows and will be immediately rebuilt from migration files on the next gateway startup.

---

## File Structure

### Files to create

| Path | Purpose |
|---|---|
| `crates/gateway/tests/migrations.rs` | Integration test that runs `sqlx::migrate!("../../migrations")` against a live Postgres and asserts all expected tables + key columns exist |
| `docs/superpowers/plans/2026-04-17-fix-migration-runner.md` | This plan (already written) |

### Files to modify

| Path | Change |
|---|---|
| `crates/gateway/src/main.rs` (lines 685–695) | Replace body of `run_migrations()` with `sqlx::migrate!` |
| `crates/gateway/src/usage.rs` (lines 889–997) | Remove `pub const MIGRATION_SQL` and the three unit tests that reference it (`test_migration_sql_not_empty`, etc.) |
| `Cargo.toml` (workspace root, dev-dependencies section of `crates/gateway/Cargo.toml`) | Ensure `sqlx` dev-dep is present for the integration test (already is) |
| `HANDOFF.md` (lines under "Post-migration cleanup" section) | Replace the "Migration runner (deferred)" bullet with a "Migration runner — fixed 2026-04-17" note once deploy verified |
| `crates/gateway/src/AGENTS.md` | One-line mention that `run_migrations()` now uses `sqlx::migrate!` |

### Files NOT to touch

- `migrations/001_initial_schema.sql` through `migrations/007_hourly_spend_limits.sql` — all idempotent, all ready.
- Any crate other than `gateway`.

### One-shot operations (not in a file, run once during deployment)

1. `flyctl ssh console --app solvela-db -C "bash -lc '...DROP TABLE spend_logs, wallet_budgets CASCADE...'"`
2. `flyctl deploy --app solvela-gateway`
3. `curl https://api.solvela.ai/v1/orgs` — expect 401 (auth required), NOT 500 (table missing)

---

## Task 1: Add integration test that runs all migrations against live Postgres

**Files:**
- Create: `crates/gateway/tests/migrations.rs`

- [ ] **Step 1: Confirm local Postgres is running**

Run:
```bash
docker compose up -d
sleep 2
docker compose ps
```

Expected output: a `postgres` service row with `State=running (healthy)` and port 5432 mapped.

- [ ] **Step 2: Create an empty test DB to isolate from dev**

Run:
```bash
PGPASSWORD=postgres psql -h localhost -U postgres -d postgres -c "DROP DATABASE IF EXISTS solvela_migrate_test;"
PGPASSWORD=postgres psql -h localhost -U postgres -d postgres -c "CREATE DATABASE solvela_migrate_test;"
```

Expected: `DROP DATABASE` (or NOTICE: does not exist) and `CREATE DATABASE`.

- [ ] **Step 3: Write the failing integration test**

Create `crates/gateway/tests/migrations.rs` with exactly this content:

```rust
//! Integration test: all migration files in `migrations/` apply cleanly to a
//! fresh Postgres database and produce the expected schema.
//!
//! Requires a running Postgres. Opt-in via the `TEST_DATABASE_URL` env var:
//!
//! ```sh
//! docker compose up -d
//! PGPASSWORD=postgres psql -h localhost -U postgres -d postgres \
//!     -c "DROP DATABASE IF EXISTS solvela_migrate_test; CREATE DATABASE solvela_migrate_test;"
//! TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/solvela_migrate_test \
//!     cargo test -p gateway --test migrations -- --ignored
//! ```

use sqlx::postgres::PgPoolOptions;
use sqlx::Row;

/// Tables that all seven migration files, applied in order, must produce.
const EXPECTED_TABLES: &[&str] = &[
    "spend_logs",
    "wallet_budgets",
    "escrow_claim_queue",
    "organizations",
    "teams",
    "org_members",
    "team_wallets",
    "api_keys",
    "audit_logs",
    "team_budgets",
];

/// Columns whose presence proves the right ALTER TABLE migrations ran.
const EXPECTED_COLUMNS: &[(&str, &str)] = &[
    ("spend_logs", "request_id"),          // migration 003
    ("spend_logs", "session_id"),          // migration 003
    ("escrow_claim_queue", "next_retry_at"), // migration 004
    ("wallet_budgets", "updated_at"),      // migration 001
    ("wallet_budgets", "hourly_limit_usdc"), // migration 007
];

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing at a throwaway Postgres"]
async fn all_migrations_apply_cleanly() {
    let url = std::env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must be set to a fresh Postgres database");

    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .expect("failed to connect to TEST_DATABASE_URL");

    sqlx::migrate!("../../migrations")
        .run(&pool)
        .await
        .expect("migrations should apply cleanly to a fresh database");

    for table in EXPECTED_TABLES {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM information_schema.tables
                WHERE table_schema = 'public' AND table_name = $1
            )",
        )
        .bind(table)
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|e| panic!("failed to query for table {table}: {e}"));

        assert!(exists, "expected table `{table}` not found in public schema");
    }

    for (table, column) in EXPECTED_COLUMNS {
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (
                SELECT 1 FROM information_schema.columns
                WHERE table_schema = 'public'
                  AND table_name = $1
                  AND column_name = $2
            )",
        )
        .bind(table)
        .bind(column)
        .fetch_one(&pool)
        .await
        .unwrap_or_else(|e| panic!("failed to query for column {table}.{column}: {e}"));

        assert!(
            exists,
            "expected column `{table}.{column}` not found — migration likely missing"
        );
    }

    let migrations_run: i64 = sqlx::query(
        "SELECT COUNT(*)::BIGINT AS n FROM _sqlx_migrations WHERE success = TRUE",
    )
    .fetch_one(&pool)
    .await
    .expect("failed to query _sqlx_migrations")
    .get("n");

    assert_eq!(migrations_run, 7, "expected 7 applied migrations, got {migrations_run}");
}
```

- [ ] **Step 4: Run the test to verify it fails (no `sqlx::migrate!` call wired yet, but the test should still run — it uses the macro directly)**

Run:
```bash
TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/solvela_migrate_test \
  cargo test -p gateway --test migrations -- --ignored
```

Expected: **PASS** — the test uses `sqlx::migrate!` directly against an empty DB, so it will already work once the path resolves. If the test fails, the most likely causes are:
- Path mismatch: `sqlx::migrate!("../../migrations")` can't find the directory from `crates/gateway/`. Fix: confirm the path is relative to `CARGO_MANIFEST_DIR` which is `crates/gateway/`; `../../migrations` resolves to the workspace root `migrations/` directory. `ls ../../migrations` from inside `crates/gateway/` should show the seven `.sql` files.
- `DATABASE_URL` not reachable: verify `docker compose ps` shows the postgres container.

- [ ] **Step 5: Commit**

```bash
git add crates/gateway/tests/migrations.rs
git commit -m "test(gateway): add integration test verifying all 7 migrations apply"
```

---

## Task 2: Replace `run_migrations()` body with `sqlx::migrate!`

**Files:**
- Modify: `crates/gateway/src/main.rs:685-695`

- [ ] **Step 1: Locate the current implementation**

Run:
```bash
sed -n '685,695p' crates/gateway/src/main.rs
```

Expected output:
```rust
/// Apply all migrations from `migrations/001_initial_schema.sql`.
///
/// Uses `CREATE TABLE IF NOT EXISTS` and `CREATE INDEX IF NOT EXISTS` throughout,
/// so running this multiple times is safe (idempotent).
async fn run_migrations(pool: &sqlx::PgPool) -> anyhow::Result<()> {
    sqlx::raw_sql(gateway::usage::MIGRATION_SQL)
        .execute(pool)
        .await?;
    info!("database migrations applied");
    Ok(())
}
```

- [ ] **Step 2: Replace the body with `sqlx::migrate!`**

Use this exact Edit tool replacement:

Old:
```rust
/// Apply all migrations from `migrations/001_initial_schema.sql`.
///
/// Uses `CREATE TABLE IF NOT EXISTS` and `CREATE INDEX IF NOT EXISTS` throughout,
/// so running this multiple times is safe (idempotent).
async fn run_migrations(pool: &sqlx::PgPool) -> anyhow::Result<()> {
    sqlx::raw_sql(gateway::usage::MIGRATION_SQL)
        .execute(pool)
        .await?;
    info!("database migrations applied");
    Ok(())
}
```

New:
```rust
/// Apply every migration file in `../../migrations/` in filename order.
///
/// Uses `sqlx::migrate!`, which embeds the migration SQL at compile time and
/// tracks applied versions in a `_sqlx_migrations` table. Safe to run on every
/// startup — sqlx skips migrations that have already been applied.
async fn run_migrations(pool: &sqlx::PgPool) -> anyhow::Result<()> {
    sqlx::migrate!("../../migrations").run(pool).await?;
    info!("database migrations applied");
    Ok(())
}
```

- [ ] **Step 3: Verify the file compiles**

Run:
```bash
cargo check -p gateway
```

Expected: a clean compile (possibly with unused-import warnings about `gateway::usage::MIGRATION_SQL` — these will disappear after Task 3).

- [ ] **Step 4: Commit**

```bash
git add crates/gateway/src/main.rs
git commit -m "fix(gateway): swap run_migrations() to sqlx::migrate! so all 7 files apply"
```

---

## Task 3: Remove the dead `MIGRATION_SQL` constant and its tests

**Files:**
- Modify: `crates/gateway/src/usage.rs:889-997`

- [ ] **Step 1: Show current state of the constant and tests**

Run:
```bash
sed -n '889,1000p' crates/gateway/src/usage.rs
```

Expected: a `pub const MIGRATION_SQL: &str = r#"…"#;` block and three unit tests (`test_migration_sql_not_empty`, and two others that assert substrings like `"spend_logs"` and `"wallet_budgets"` appear in `MIGRATION_SQL`).

- [ ] **Step 2: Delete the constant and its three tests**

Use the Edit tool to remove the block starting with `pub const MIGRATION_SQL: &str = r#"` and ending at the closing `"#;`. Also remove the three unit tests inside the `#[cfg(test)] mod tests { ... }` block that reference `MIGRATION_SQL`. Leave all other constants, structs (`SpendLog`, `WalletBudget`), functions, and tests in place.

After the edit, search for lingering references:

Run:
```bash
grep -rn "MIGRATION_SQL" crates/gateway/src/
```

Expected: **no output** (empty result).

- [ ] **Step 3: Confirm the crate still compiles**

Run:
```bash
cargo check -p gateway
```

Expected: clean compile, no unused-import warnings.

- [ ] **Step 4: Run the full gateway unit test suite to confirm nothing else depended on MIGRATION_SQL**

Run:
```bash
cargo test -p gateway --lib
```

Expected: all tests pass (the three MIGRATION_SQL tests no longer exist; everything else still passes).

- [ ] **Step 5: Commit**

```bash
git add crates/gateway/src/usage.rs
git commit -m "refactor(gateway): remove inline MIGRATION_SQL — sqlx::migrate! supersedes it"
```

---

## Task 4: Format, lint, test once more

- [ ] **Step 1: Format**

Run:
```bash
cargo fmt --all
```

Expected: no output (already formatted) or a silent rewrite. If any file changed, `git diff` to review.

- [ ] **Step 2: Clippy**

Run:
```bash
cargo clippy --all-targets --all-features -- -D warnings
```

Expected: clean pass. If warnings fire on `sqlx::migrate!` about compile-time-unknown types, they're harmless clippy noise; in that case, add `#[allow(clippy::unnecessary_wraps)]` or similar targeted fix only for the specific warning (do not wholesale silence clippy).

- [ ] **Step 3: Full workspace test run**

Run:
```bash
cargo test
```

Expected: all tests pass. The ignored integration test in `crates/gateway/tests/migrations.rs` does **not** run here (it's gated by `#[ignore]`), which is intentional.

- [ ] **Step 4: Run the opt-in integration test one more time**

Run:
```bash
PGPASSWORD=postgres psql -h localhost -U postgres -d postgres \
  -c "DROP DATABASE IF EXISTS solvela_migrate_test; CREATE DATABASE solvela_migrate_test;"

TEST_DATABASE_URL=postgres://postgres:postgres@localhost:5432/solvela_migrate_test \
  cargo test -p gateway --test migrations -- --ignored
```

Expected: the `all_migrations_apply_cleanly` test passes. The test confirms all 7 migrations ran, all 10 expected tables exist, and all 5 expected columns exist.

- [ ] **Step 5: Commit if anything changed during fmt/clippy**

```bash
git status
# If changes, add + commit. Otherwise skip.
git add -u && git commit -m "style: cargo fmt after migration runner swap"
```

---

## Task 5: Drop legacy tables on solvela-db (one-shot ops)

This step clears the drifted `spend_logs` and `wallet_budgets` tables on the production Fly Postgres cluster so the new migration runner can rebuild them from migration files on the next gateway startup.

**Safety checks before running:**
- Row count on both tables must be 0 (confirmed 2026-04-17, but re-verify below).
- Both tables must be the only user-created tables in `public` schema (confirmed 2026-04-17, but re-verify below).

- [ ] **Step 1: Re-verify zero data on solvela-db**

Run:
```bash
flyctl ssh console --app solvela-db \
  -C 'bash -lc "PGPASSWORD=\"\$OPERATOR_PASSWORD\" psql -h localhost -U postgres -d solvela_gateway -c \"SELECT relname, n_live_tup FROM pg_stat_user_tables ORDER BY relname\""'
```

Expected output:
```
    relname     | n_live_tup 
----------------+------------
 spend_logs     |          0
 wallet_budgets |          0
(2 rows)
```

If any `n_live_tup` is not 0, **stop this task and investigate** before dropping.

- [ ] **Step 2: Drop the two legacy tables with CASCADE**

Build a base64-encoded SQL blob so nested shell quoting doesn't mangle it:

```bash
SQL_B64=$(printf 'DROP TABLE IF EXISTS spend_logs CASCADE;\nDROP TABLE IF EXISTS wallet_budgets CASCADE;\nDROP TABLE IF EXISTS _sqlx_migrations CASCADE;\n' | base64 -w0)

flyctl ssh console --app solvela-db \
  -C "bash -lc 'echo $SQL_B64 | base64 -d | PGPASSWORD=\"\$OPERATOR_PASSWORD\" psql -h localhost -U postgres -d solvela_gateway'"
```

Expected output:
```
DROP TABLE
DROP TABLE
DROP TABLE
```
(The third `DROP TABLE` may be `NOTICE:  table "_sqlx_migrations" does not exist, skipping` — that's fine, we include it for idempotency.)

- [ ] **Step 3: Verify the public schema is empty**

Run:
```bash
flyctl ssh console --app solvela-db \
  -C 'bash -lc "PGPASSWORD=\"\$OPERATOR_PASSWORD\" psql -h localhost -U postgres -d solvela_gateway -c \"\\dt\""'
```

Expected output: `Did not find any relations.`

The DB is now ready for a fresh `sqlx::migrate!` pass.

---

## Task 6: Deploy the fixed gateway and verify migrations ran

- [ ] **Step 1: Deploy**

Run:
```bash
flyctl deploy --app solvela-gateway
```

Expected: build succeeds (cargo-chef cache makes this fast), two machines update with rolling strategy, both healthy. Total time ~2 minutes.

- [ ] **Step 2: Tail logs for migration output**

Run:
```bash
flyctl logs --app solvela-gateway --no-tail | grep -iE "migrat|applied|sqlx" | tail -20
```

Expected: a `database migrations applied` log line from one of the new machines. No `ERROR` lines referencing schema mismatches or migration failures.

- [ ] **Step 3: Confirm all 10 tables now exist in solvela-db**

Run:
```bash
flyctl ssh console --app solvela-db \
  -C 'bash -lc "PGPASSWORD=\"\$OPERATOR_PASSWORD\" psql -h localhost -U postgres -d solvela_gateway -c \"\\dt\""'
```

Expected: a listing containing at minimum the following relations — `api_keys`, `audit_logs`, `escrow_claim_queue`, `org_members`, `organizations`, `spend_logs`, `team_budgets`, `team_wallets`, `teams`, `wallet_budgets`, `_sqlx_migrations`.

- [ ] **Step 4: Confirm `_sqlx_migrations` tracks all 7 versions**

Run:
```bash
flyctl ssh console --app solvela-db \
  -C 'bash -lc "PGPASSWORD=\"\$OPERATOR_PASSWORD\" psql -h localhost -U postgres -d solvela_gateway -c \"SELECT version, description, success FROM _sqlx_migrations ORDER BY version\""'
```

Expected: 7 rows, all with `success = t` (true), covering the seven migration filenames from `001_initial_schema` through `007_hourly_spend_limits`.

- [ ] **Step 5: Hit a previously-broken endpoint**

Run:
```bash
curl -sS -w "\nHTTP %{http_code}\n" https://api.solvela.ai/v1/orgs
```

Expected: **HTTP 401** (unauthorized — auth middleware fires), NOT HTTP 500 (table missing). The 401 proves the route reaches the handler and the handler's DB lookup doesn't blow up with `relation "organizations" does not exist`.

- [ ] **Step 6: Hit health + 402 paths to confirm no regression**

Run:
```bash
curl -sS -w "\nHTTP %{http_code}\n" https://api.solvela.ai/health

curl -sS -w "\nHTTP %{http_code}\n" -X POST https://api.solvela.ai/v1/chat/completions \
  -H 'content-type: application/json' \
  -d '{"model":"auto","messages":[{"role":"user","content":"hi"}]}'
```

Expected: health `200` with `{"status":"ok"}`; chat `402` with an x402 payment quote body. Same as pre-migration behavior — no regression on the working paths.

---

## Task 7: Update docs

**Files:**
- Modify: `HANDOFF.md` (the "Post-migration cleanup" section)
- Modify: `crates/gateway/src/AGENTS.md`

- [ ] **Step 1: Update the Post-migration cleanup section in HANDOFF.md**

Find this bullet:

```markdown
- **Migration runner:** `run_migrations()` only applies the inline `MIGRATION_SQL` (spend_logs + wallet_budgets). Migrations 002–007 in `migrations/` are NEVER applied by the gateway — so orgs/teams/api_keys/audit_logs/escrow_claim_queue/hourly_spend_limits tables don't exist in any prod DB. Wire up `sqlx::migrate!("./migrations")` before shipping any org-authenticated traffic. (Deferred — separate writeup.)
```

Replace with:

```markdown
- **Migration runner:** Fixed 2026-04-17 (see `docs/superpowers/plans/2026-04-17-fix-migration-runner.md`). `run_migrations()` now uses `sqlx::migrate!("../../migrations")` which embeds all 7 migration files and tracks applied versions in `_sqlx_migrations`. Verified: 10 tables present in solvela-db, `_sqlx_migrations` shows 7/7 applied.
```

Update the "Last verified" line at the top to mention the migration fix.

- [ ] **Step 2: Add a one-liner to crates/gateway/src/AGENTS.md**

Find the line describing `main.rs`:

```markdown
| `main.rs` | Binary entry — loads config, connects PG/Redis, runs migrations, starts server |
```

Replace with:

```markdown
| `main.rs` | Binary entry — loads config, connects PG/Redis, applies all 7 migration files via `sqlx::migrate!("../../migrations")`, starts server |
```

- [ ] **Step 3: Commit docs**

```bash
git add HANDOFF.md crates/gateway/src/AGENTS.md
git commit -m "docs: note migration runner fix is deployed"
```

---

## Task 8: Final verification

- [ ] **Step 1: Confirm git log shows 4–5 tight commits**

Run:
```bash
git log --oneline -10
```

Expected: commits roughly in this order (most recent first):
1. `docs: note migration runner fix is deployed`
2. `style: cargo fmt after migration runner swap` (if any)
3. `refactor(gateway): remove inline MIGRATION_SQL — sqlx::migrate! supersedes it`
4. `fix(gateway): swap run_migrations() to sqlx::migrate! so all 7 files apply`
5. `test(gateway): add integration test verifying all 7 migrations apply`

- [ ] **Step 2: Run the full lint + test suite one more time**

Run:
```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

Expected: all green.

- [ ] **Step 3: Consider opening a PR**

This is a behavior-change commit (schema now matches what the code has always claimed). If the repo has a PR workflow, push the branch and open one. Otherwise merge to main per usual practice.

---

## Rollback plan

If Task 6 verification fails (e.g., `cargo deploy` succeeds but startup errors prevent the gateway from coming up), the recovery steps are:

1. **Immediate:** Revert the gateway binary by redeploying the previous commit:
   ```bash
   git revert HEAD~4..HEAD --no-edit   # roll back the 4 code commits
   flyctl deploy --app solvela-gateway
   ```
2. **Then:** The `solvela-db` cluster will still have an empty public schema (the DROP TABLE ran in Task 5). To restore the "old" shape, re-run the old MIGRATION_SQL once manually via flyctl ssh + psql before re-deploying the old gateway. The prior SQL is recoverable from any commit older than the "remove inline MIGRATION_SQL" commit.
3. **Root cause analysis:** Look for migration file bugs. The most likely failure modes at Task 6 are:
   - Foreign key ordering issues if migration files were ever reordered (they shouldn't be — the plan forbids modifying 001–007).
   - `sqlx::migrate!` path not resolving at compile time — the error surfaces during `cargo build`, not at runtime, so would have been caught by Task 2 Step 3. If the deploy build failed with a path error, the macro argument needs adjustment (try `"./migrations"` or an absolute path).

No user data is at risk at any point in this plan — the live DB has 0 user rows, and the rollback path leaves the DB in at worst an empty-schema state.

---

## Post-plan: things to reconsider separately

These are surfaced so they aren't forgotten, but they're **out of scope** for this plan:

- **The `/v1/orgs/...` handlers and org middleware** — now that the tables exist, actually exercise these endpoints end-to-end (create org, add member, mint API key, make a chat request with that API key). The handlers have never been touched by real traffic in prod. Expect bugs.
- **The `hourly_limit_usdc` wiring on `wallet_budgets`** — migration 007 adds the column. Does `WalletBudget` in `crates/gateway/src/usage.rs` read it? Does any endpoint set it? If not, this is a schema-only ship, and the feature is still incomplete at the code layer.
- **Automated migration tests in CI** — the integration test added in Task 1 is gated by `--ignored` so it doesn't run in `cargo test`. CI should explicitly run it after starting docker-compose. A `.github/workflows/*.yml` change is warranted but not included here.
- **A `sqlx::migrate!` verify-only mode** — if you want startup to refuse to boot when a migration has drifted on disk vs what's applied, add `sqlx::migrate!("../../migrations").run_with_verify(pool).await?` or similar. Current plan uses plain `.run()` which is forgiving.
