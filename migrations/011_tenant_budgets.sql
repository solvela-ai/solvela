-- Per-tenant budget ENFORCEMENT (PR2). Builds on migration 010's attribution-only
-- `spend_logs.tenant` column. A `tenant_budgets` row sets hourly/daily/monthly
-- caps on a `(wallet_address, tenant)` bucket; the gateway enforces it atomically
-- the same way it enforces wallet/team caps.
--
-- IMPORTANT (security boundary): the `x-tenant` header is UNAUTHENTICATED and
-- FORGEABLE. These budgets are cooperative accounting under ONE trusted
-- single-wallet proxy metering its own downstream customers — they are NOT
-- isolation between mutually-distrusting tenants. Anyone who controls the
-- wallet's traffic can set any tenant tag.
--
-- Additive + idempotent. Mirrors the 005/007/008 trigger pattern.
CREATE TABLE IF NOT EXISTS tenant_budgets (
    wallet_address      TEXT        NOT NULL,
    tenant              TEXT        NOT NULL,
    hourly_limit_usdc   DECIMAL(18, 6),
    daily_limit_usdc    DECIMAL(18, 6),
    monthly_limit_usdc  DECIMAL(18, 6),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (wallet_address, tenant)
);

DROP TRIGGER IF EXISTS trg_tenant_budgets_updated_at ON tenant_budgets;
CREATE TRIGGER trg_tenant_budgets_updated_at
    BEFORE UPDATE ON tenant_budgets
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

-- Opt-in fail-closed flag: when TRUE, the wallet may only spend under a tagged,
-- provisioned tenant (untagged or unknown-tenant requests are rejected). Default
-- FALSE so every existing wallet's behavior is unchanged.
ALTER TABLE wallet_budgets ADD COLUMN IF NOT EXISTS require_tenant BOOLEAN NOT NULL DEFAULT FALSE;
