-- Per-project budget caps
--
-- Adds an org-scoped `budget_projects` table. Each project carries a hard
-- cap (`budget_usd_atomic`) and a running counter (`spent_usd_atomic`),
-- both expressed in atomic USDC units (6 decimals). The CHECK constraint
-- prevents negative budgets at the schema level.
--
-- A `try_charge(project_id, amount)` SQL pattern is used to atomically
-- increment `spent_usd_atomic` only when there is remaining budget — see
-- `crates/gateway/src/orgs/budget_projects.rs::BudgetProjectRepo::try_charge`.
-- Pattern (atomic conditional update) ported from Franklin's
-- `src/content/library.ts:122-145` ContentLibrary.addAsset.
CREATE TABLE IF NOT EXISTS budget_projects (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id              UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    name                TEXT        NOT NULL,
    budget_usd_atomic   BIGINT      NOT NULL CHECK (budget_usd_atomic >= 0),
    spent_usd_atomic    BIGINT      NOT NULL DEFAULT 0,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (org_id, name)
);

CREATE INDEX IF NOT EXISTS budget_projects_org_idx ON budget_projects(org_id);

-- Reuse the generic updated_at trigger function defined in 005_organizations.sql.
DROP TRIGGER IF EXISTS trg_budget_projects_updated_at ON budget_projects;
CREATE TRIGGER trg_budget_projects_updated_at
    BEFORE UPDATE ON budget_projects
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();
