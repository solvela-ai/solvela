-- Organizations: top-level billing entity
CREATE TABLE IF NOT EXISTS organizations (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    name            TEXT        NOT NULL,
    slug            TEXT        NOT NULL UNIQUE,
    owner_wallet    TEXT        NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_org_owner ON organizations(owner_wallet);
CREATE INDEX IF NOT EXISTS idx_org_slug ON organizations(slug);

-- Teams within an organization
CREATE TABLE IF NOT EXISTS teams (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id          UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    name            TEXT        NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(org_id, name)
);

CREATE INDEX IF NOT EXISTS idx_team_org ON teams(org_id);

-- Organization members (wallet -> org mapping with role)
CREATE TABLE IF NOT EXISTS org_members (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id          UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    wallet_address  TEXT        NOT NULL,
    role            TEXT        NOT NULL DEFAULT 'member' CHECK (role IN ('owner', 'admin', 'member')),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(org_id, wallet_address)
);

CREATE INDEX IF NOT EXISTS idx_org_member_wallet ON org_members(wallet_address);
CREATE INDEX IF NOT EXISTS idx_org_member_org ON org_members(org_id);

-- Team wallet assignments
CREATE TABLE IF NOT EXISTS team_wallets (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    team_id         UUID        NOT NULL REFERENCES teams(id) ON DELETE CASCADE,
    wallet_address  TEXT        NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(team_id, wallet_address)
);

CREATE INDEX IF NOT EXISTS idx_team_wallet_team ON team_wallets(team_id);
CREATE INDEX IF NOT EXISTS idx_team_wallet_wallet ON team_wallets(wallet_address);

-- API keys (org-scoped)
CREATE TABLE IF NOT EXISTS api_keys (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    org_id          UUID        NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    key_hash        TEXT        NOT NULL UNIQUE,
    key_prefix      TEXT        NOT NULL,
    name            TEXT        NOT NULL,
    role            TEXT        NOT NULL DEFAULT 'member' CHECK (role IN ('owner', 'admin', 'member')),
    last_used_at    TIMESTAMPTZ,
    expires_at      TIMESTAMPTZ,
    revoked_at      TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_api_key_org ON api_keys(org_id);
CREATE INDEX IF NOT EXISTS idx_api_key_hash ON api_keys(key_hash);

-- Generic updated_at trigger function (replaces migration 001's version)
CREATE OR REPLACE FUNCTION update_updated_at_column()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trg_organizations_updated_at ON organizations;
CREATE TRIGGER trg_organizations_updated_at
    BEFORE UPDATE ON organizations
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

DROP TRIGGER IF EXISTS trg_teams_updated_at ON teams;
CREATE TRIGGER trg_teams_updated_at
    BEFORE UPDATE ON teams
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();

-- Repoint wallet_budgets trigger to use the generic function
DROP TRIGGER IF EXISTS trg_wallet_budgets_updated_at ON wallet_budgets;
CREATE TRIGGER trg_wallet_budgets_updated_at
    BEFORE UPDATE ON wallet_budgets
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();
