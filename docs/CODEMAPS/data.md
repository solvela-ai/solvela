<!-- Generated: 2026-05-27 | Files scanned: 7 | Token estimate: ~650 -->

# Database Schema & Data Codemap

**Last Updated:** 2026-05-27  
**Provider:** PostgreSQL 16 (optional; gateway degrades gracefully without it)  
**Migrations:** `migrations/` (9 files, idempotent, auto-run on startup)  
**ORM:** sqlx (runtime-checked SQL via `sqlx::query`/`query_as`; no compile-time `query!` macros)

## Schema Overview

### Payment & Usage

#### spend_logs
One row per completed LLM request. Written asynchronously (fire-and-forget via `tokio::spawn`).

| Column | Type | Constraints | Notes |
|--------|------|-----------|-------|
| id | UUID | PK, default gen_random_uuid() | Unique request identifier |
| wallet_address | TEXT | NOT NULL | Solana wallet address (agent) |
| model | TEXT | NOT NULL | Model ID (e.g., "gpt-4o") |
| provider | TEXT | NOT NULL | Provider name (e.g., "openai") |
| input_tokens | INTEGER | NOT NULL, CHECK >= 0 | Prompt tokens counted |
| output_tokens | INTEGER | NOT NULL, CHECK >= 0 | Completion tokens |
| cost_usdc | DECIMAL(18, 6) | NOT NULL, CHECK >= 0 | Total cost including 5% fee |
| tx_signature | TEXT | NULLABLE | Solana transaction signature (nullable for free/no-payment) |
| created_at | TIMESTAMPTZ | NOT NULL, DEFAULT NOW() | Request timestamp |

**Indexes:**
- `idx_spend_wallet` — Fast wallet analytics, budget enforcement
- `idx_spend_created` — Fast time-range queries (daily/monthly aggregation)
- `idx_spend_wallet_created` — Combined for wallet + date range queries

**Usage:**
- Per-wallet analytics (total spend, request count)
- Daily/monthly budget enforcement
- Provider/model usage breakdown
- Audit trail of all requests

---

#### wallet_budgets
Optional per-wallet spending limits. Absence = unlimited.

| Column | Type | Constraints | Notes |
|--------|------|-----------|-------|
| wallet_address | TEXT | PK | Solana wallet address |
| hourly_limit_usdc | DECIMAL(18, 6) | NULLABLE | Hourly spend cap (added in migration 007) |
| daily_limit_usdc | DECIMAL(18, 6) | NULLABLE | Daily spend cap (NULL = unlimited) |
| monthly_limit_usdc | DECIMAL(18, 6) | NULLABLE | Monthly spend cap (NULL = unlimited) |
| total_spent_usdc | DECIMAL(18, 6) | NOT NULL, DEFAULT 0, CHECK >= 0 | Cumulative spend (metadata) |
| created_at | TIMESTAMPTZ | NOT NULL, DEFAULT NOW() | When limit created |
| updated_at | TIMESTAMPTZ | NOT NULL, DEFAULT NOW() | Last modified |

**Triggers:**
- `trg_wallet_budgets_updated_at` — Auto-update `updated_at` on any row change

**Usage:**
- Check remaining daily quota before request
- Check remaining monthly quota
- Reset daily limit at midnight UTC

---

### Organization & Multi-Tenancy

#### organizations
Top-level billing entity. Multiple teams within each org.

| Column | Type | Constraints | Notes |
|--------|------|-----------|-------|
| id | UUID | PK, default gen_random_uuid() | Org identifier |
| name | TEXT | NOT NULL | Display name (e.g., "Acme AI Corp") |
| slug | TEXT | NOT NULL UNIQUE | URL-safe identifier (e.g., "acme-ai") |
| owner_wallet | TEXT | NOT NULL | Owner's Solana wallet |
| created_at | TIMESTAMPTZ | NOT NULL, DEFAULT NOW() | Org creation date |
| updated_at | TIMESTAMPTZ | NOT NULL, DEFAULT NOW() | Last modified |

**Indexes:**
- `idx_org_owner` — Find orgs by owner wallet

**Triggers:**
- `trg_organizations_updated_at` — Auto-update `updated_at`

**Usage:**
- Billing boundary (single invoice per org/month)
- Namespace for teams, API keys, members
- Root of RBAC hierarchy

---

#### teams
Sub-division within an org for cost center/project organization.

| Column | Type | Constraints | Notes |
|--------|------|-----------|-------|
| id | UUID | PK, default gen_random_uuid() | Team identifier |
| org_id | UUID | FK → organizations(id) ON DELETE CASCADE | Parent org |
| name | TEXT | NOT NULL | Team name (e.g., "Data Science") |
| created_at | TIMESTAMPTZ | NOT NULL, DEFAULT NOW() | Creation date |
| updated_at | TIMESTAMPTZ | NOT NULL, DEFAULT NOW() | Last modified |

**Unique:** (org_id, name) — Team names unique per org

**Indexes:**
- `idx_team_org` — List teams in org

**Usage:**
- Cost center allocation
- Team-scoped budgets (optional future)
- Organize agents/wallets by team

---

#### org_members
Wallet → Org mapping with role-based access control.

| Column | Type | Constraints | Notes |
|--------|------|-----------|-------|
| id | UUID | PK, default gen_random_uuid() | Member record ID |
| org_id | UUID | FK → organizations(id) ON DELETE CASCADE | Parent org |
| wallet_address | TEXT | NOT NULL | Solana wallet address |
| role | TEXT | NOT NULL, DEFAULT 'member' | 'owner', 'admin', 'member' |
| created_at | TIMESTAMPTZ | NOT NULL, DEFAULT NOW() | Added date |
| updated_at | TIMESTAMPTZ | NOT NULL, DEFAULT NOW() | Last modified |

**Unique:** (org_id, wallet_address) — One role per wallet per org

**Indexes:**
- `idx_org_member_wallet` — Find orgs by member wallet

**RBAC Roles:**
- `owner` — Full org control (create teams, add members, set budgets)
- `admin` — Manage members, settings, API keys; cannot delete org
- `member` — Use chat, view analytics; no admin actions

**Usage:**
- Determine permissions (RequireOrgAdmin extractor checks role)
- Access control to org endpoints
- Audit who made changes

---

#### team_wallets
Team → Wallet assignment for team-scoped operations.

| Column | Type | Constraints | Notes |
|--------|------|-----------|-------|
| id | UUID | PK, default gen_random_uuid() | Mapping ID |
| team_id | UUID | FK → teams(id) ON DELETE CASCADE | Parent team |
| wallet_address | TEXT | NOT NULL | Solana wallet address |
| created_at | TIMESTAMPTZ | NOT NULL, DEFAULT NOW() | Added date |

**Unique:** (team_id, wallet_address) — One assignment per wallet per team

**Indexes:**
- `idx_team_wallet_team` — Find wallets in team
- `idx_team_wallet_wallet` — Find teams for wallet

**Usage:**
- Map agent wallets to teams
- Aggregate team spend from wallet spend logs
- Team-scoped budget enforcement (future)

---

### Authentication & Authorization

#### api_keys
Org-scoped API credentials for programmatic access.

| Column | Type | Constraints | Notes |
|--------|------|-----------|-------|
| id | UUID | PK, default gen_random_uuid() | Key record ID |
| org_id | UUID | FK → organizations(id) ON DELETE CASCADE | Parent org |
| key_hash | TEXT | NOT NULL UNIQUE | SHA256(secret_key); hashed for storage |
| key_prefix | TEXT | NOT NULL | First 12 chars of key (shown in UI) |
| name | TEXT | NOT NULL | Display name (e.g., "Dashboard API Key") |
| role | TEXT | NOT NULL, DEFAULT 'member' | 'owner', 'admin', 'member' (same as org_members) |
| last_used_at | TIMESTAMPTZ | NULLABLE | Timestamp of last use (for monitoring) |
| expires_at | TIMESTAMPTZ | NULLABLE | Expiration date (NULL = never expires) |
| revoked_at | TIMESTAMPTZ | NULLABLE | Revocation date (soft delete) |
| created_at | TIMESTAMPTZ | NOT NULL, DEFAULT NOW() | Creation date |

**Indexes:**
- `idx_api_key_org` — Find keys for org

**Usage:**
- Authenticate requests with `Authorization: Bearer solv_...` header
- Never store plaintext key in DB; compare hashes only
- Support key expiration (check `expires_at`)
- Support revocation (check `revoked_at IS NULL`)
- Monitor usage via `last_used_at`

**Key Format:** `solvela_k_[random]` — the canonical prefix used by `extract_api_key` (the legacy `rcr_k_` prefix is still accepted for back-compat).

---

### Audit & Compliance

#### audit_logs (migration 006, hardened in 009)
Action tracking for compliance and debugging.

| Column | Type | Constraints | Notes |
|--------|------|-----------|-------|
| id | UUID | PK, default gen_random_uuid() | Log entry ID |
| org_id | UUID | FK → organizations(id) ON DELETE SET NULL | Org affected |
| actor_wallet | TEXT | NULLABLE | Actor wallet (NULL when admin token acted) |
| actor_api_key | UUID | FK → api_keys(id) ON DELETE SET NULL | Actor API key id |
| action | TEXT | NOT NULL | Action type (e.g., "team_created", "member_added") |
| resource_type | TEXT | NOT NULL | Resource kind (org, team, member, …) |
| resource_id | TEXT | NULLABLE | Resource identifier |
| details | JSONB | NULLABLE | Action details (team name, role, etc.) |
| ip_address | TEXT | NULLABLE | Originating IP for the admin action |
| created_at | TIMESTAMPTZ | NOT NULL, DEFAULT NOW() | Action timestamp |

**Indexes:**
- `idx_audit_org` — Find logs for org
- `idx_audit_action` — Find logs by action
- `idx_audit_created` — Find logs by timestamp
- `idx_audit_actor` — Find logs by actor wallet (partial index, WHERE actor_wallet IS NOT NULL)

**Usage:**
- Compliance audit trail (SOC 2 Type II)
- Investigate unauthorized changes
- Retrieve action history for UI

**Logged Actions:**
- `org_created`, `org_updated`, `org_deleted`
- `team_created`, `team_updated`, `team_deleted`
- `member_added`, `member_updated`, `member_removed`
- `api_key_created`, `api_key_revoked`
- `budget_set`, `budget_updated`
- `request_made` (optional; may be logged in spend_logs instead)

---

#### escrow_claim_queue (migration 002, extended in 004 & 008)
Pending USDC-SPL escrow claims (async processing).

| Column | Type | Constraints | Notes |
|--------|------|-----------|-------|
| id | UUID | PK, default gen_random_uuid() | Queue entry ID |
| service_id | BYTEA | NOT NULL | 32-byte service ID (used in PDA seed) |
| agent_pubkey | TEXT | NOT NULL | Base58 agent wallet (claimer) |
| claim_amount | BIGINT | NOT NULL | Amount to claim in **atomic USDC units** (6 decimals) |
| deposited_amount | BIGINT | NULLABLE | Verified deposit ceiling (atomic units) |
| status | TEXT | NOT NULL, DEFAULT 'pending' | 'pending', 'in_progress', 'completed', 'failed' |
| attempts | INTEGER | NOT NULL, DEFAULT 0 | Number of retry attempts |
| last_attempt_at | TIMESTAMPTZ | NULLABLE | Timestamp of last attempt |
| next_retry_at | TIMESTAMPTZ | NULLABLE (added in migration 004) | When to retry next (exponential backoff) |
| tx_signature | TEXT | NULLABLE | Solana transaction signature (filled on success) |
| error_message | TEXT | NULLABLE | Last error message (if failed) |
| created_at | TIMESTAMPTZ | NOT NULL, DEFAULT NOW() | Claim queued date |
| completed_at | TIMESTAMPTZ | NULLABLE | When claim finished |
| updated_at | TIMESTAMPTZ | NOT NULL, DEFAULT NOW() | Added in migration 008 (drives stale-claim recovery) |

**Indexes:**
- `idx_claim_queue_status` — Find pending claims by status
- `idx_claim_queue_created` — Find claims by submission time

**Usage:**
- Background processor (`escrow/claim_processor.rs`) polls pending claims
- Retries failed claims with exponential backoff
- Tracks claim lifecycle from submission to confirmation
- Stale `in_progress` claims older than 5 minutes (by `updated_at`) are reclaimed by `fetch_pending_claims`

---

#### team_budgets (migration 007)
Per-team spend caps. (Migration 007 is named `007_hourly_spend_limits.sql` but actually creates `team_budgets` and adds `hourly_limit_usdc` to `wallet_budgets`; there is no `hourly_spend_limits` table.)

| Column | Type | Constraints | Notes |
|--------|------|-----------|-------|
| team_id | UUID | PK, FK → teams(id) ON DELETE CASCADE | Team |
| hourly_limit_usdc | DECIMAL(18, 6) | NULLABLE | Hourly spend cap |
| daily_limit_usdc | DECIMAL(18, 6) | NULLABLE | Daily spend cap |
| monthly_limit_usdc | DECIMAL(18, 6) | NULLABLE | Monthly spend cap |
| created_at | TIMESTAMPTZ | NOT NULL, DEFAULT NOW() | Record created |
| updated_at | TIMESTAMPTZ | NOT NULL, DEFAULT NOW() | Last modified (auto-updated by trigger) |

**Triggers:**
- `trg_team_budgets_updated_at` — Auto-update `updated_at` on any row change

**Usage:**
- Enforce per-team spend caps
- Rate limit expensive requests per team per period

---

## Migration Timeline

| # | File | Tables Created/Modified | Purpose |
|---|------|--------|---------|
| 1 | `001_initial_schema.sql` | spend_logs, wallet_budgets | Initial payment tracking |
| 2 | `002_escrow_claim_queue.sql` | escrow_claim_queue | Async escrow claim processing |
| 3 | `003_phase_g_request_session_ids.sql` | spend_logs.request_id, spend_logs.session_id | Add request/session tracking |
| 4 | `004_claim_queue_next_retry_at.sql` | escrow_claim_queue.next_retry_at | Backoff scheduling |
| 5 | `005_organizations.sql` | organizations, teams, org_members, team_wallets, api_keys | Multi-tenant RBAC |
| 6 | `006_audit_logs.sql` | audit_logs | Compliance audit trail |
| 7 | `007_hourly_spend_limits.sql` | wallet_budgets.hourly_limit_usdc, team_budgets | Team budgets + hourly caps |
| 8 | `008_escrow_claim_queue_updated_at.sql` | escrow_claim_queue.updated_at | Stale-claim recovery |
| 9 | `009_audit_actor_admin.sql` | audit_logs admin-actor backfill | Track admin-token actions |

All migrations are **idempotent** (use `CREATE TABLE IF NOT EXISTS`). Applied automatically on gateway startup via `run_migrations()` (from `main.rs`). If migration fails and `DATABASE_URL` is set, gateway exits with non-zero status (fatal error).

---

## Data Access Patterns

### Spend Tracking (usage.rs module)
```rust
// Log a request (fire-and-forget)
tokio::spawn(async move {
    sqlx::query("INSERT INTO spend_logs (...) VALUES (...)")
        .execute(&db_pool)
        .await
        .ok();  // Ignore errors to avoid blocking hot path
});

// Check budget before request
let spent_today = sqlx::query_scalar(
    "SELECT COALESCE(SUM(cost_usdc), 0) FROM spend_logs 
     WHERE wallet_address = $1 AND created_at::date = CURRENT_DATE"
).bind(wallet).fetch_one(&db_pool).await?;
```

### Org Context (orgs/queries.rs)
```rust
// Find org by ID
let org = sqlx::query_as::<_, Organization>(
    "SELECT * FROM organizations WHERE id = $1"
).bind(org_id).fetch_one(&db_pool).await?;

// List members
let members = sqlx::query_as::<_, OrgMember>(
    "SELECT * FROM org_members WHERE org_id = $1"
).bind(org_id).fetch_all(&db_pool).await?;
```

### API Key Verification (security.rs)
```rust
// Verify key
let key_hash = sha256(provided_key);
let record = sqlx::query_as::<_, ApiKeyRecord>(
    "SELECT * FROM api_keys WHERE key_hash = $1 
     AND revoked_at IS NULL 
     AND (expires_at IS NULL OR expires_at > NOW())"
).bind(key_hash).fetch_optional(&db_pool).await?;
```

---

## Optional: Redis Cache (response cache)

If `REDIS_URL` is set, gateway caches chat responses:

**Key:** SHA-256(model ‖ serialised messages ‖ temperature)  
**Value:** JSON response  
**TTL:** 600 seconds (10 minutes) default; configurable via `ResponseCacheConfig::default_ttl_secs`

Enables:
- Wallet-agnostic response cache (two agents with identical prompts share cached response)
- Reduced LLM API calls
- Faster response times for common queries

Falls back to no caching if Redis unavailable.

---

## Graceful Degradation

### No PostgreSQL (DATABASE_URL unset)
- spend_logs not written (no analytics, no budget enforcement)
- org/team/api_key features disabled
- Gateway still serves chat/models
- Perfect for standalone agent use case

### No Redis (REDIS_URL unset)
- Response cache disabled
- Replay protection falls back to in-memory LRU (10k entries, 120s TTL)
- chat endpoint works, but cache misses on every request
- May cause duplicate LLM calls for identical prompts

### No Escrow Database
- Escrow deposits accepted, but claims not tracked
- Claim processor disabled
- Manual claim resolution required

---

## Related

- [Architecture Overview](architecture.md)
- [Backend Routes](backend.md)
- [Migrations directory](../../migrations/)
