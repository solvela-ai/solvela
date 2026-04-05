-- Add hourly spend limit to wallet_budgets
ALTER TABLE wallet_budgets ADD COLUMN IF NOT EXISTS hourly_limit_usdc DECIMAL(18, 6);

-- Team-level budget limits
CREATE TABLE IF NOT EXISTS team_budgets (
    team_id             UUID        PRIMARY KEY REFERENCES teams(id) ON DELETE CASCADE,
    hourly_limit_usdc   DECIMAL(18, 6),
    daily_limit_usdc    DECIMAL(18, 6),
    monthly_limit_usdc  DECIMAL(18, 6),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

DROP TRIGGER IF EXISTS trg_team_budgets_updated_at ON team_budgets;
CREATE TRIGGER trg_team_budgets_updated_at
    BEFORE UPDATE ON team_budgets
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();
