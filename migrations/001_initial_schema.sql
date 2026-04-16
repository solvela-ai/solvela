-- Solvela initial schema
-- Run: psql $DATABASE_URL -f migrations/001_initial_schema.sql
-- Or: applied automatically by docker-compose on first start.

-- ─── Spend logs ───────────────────────────────────────────────────────────────
-- One row per completed LLM request. Written asynchronously (fire-and-forget).

CREATE TABLE IF NOT EXISTS spend_logs (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    wallet_address   TEXT        NOT NULL,
    model            TEXT        NOT NULL,
    provider         TEXT        NOT NULL,
    input_tokens     INTEGER     NOT NULL CHECK (input_tokens >= 0),
    output_tokens    INTEGER     NOT NULL CHECK (output_tokens >= 0),
    cost_usdc        DECIMAL(18, 6) NOT NULL CHECK (cost_usdc >= 0),
    tx_signature     TEXT,                          -- Solana tx signature (nullable for free-tier)
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Fast lookups by wallet (per-wallet analytics, budget checks)
CREATE INDEX IF NOT EXISTS idx_spend_wallet
    ON spend_logs (wallet_address);

-- Fast lookups by time (daily/monthly aggregations)
CREATE INDEX IF NOT EXISTS idx_spend_created
    ON spend_logs (created_at);

-- Combined index for wallet + time range queries
CREATE INDEX IF NOT EXISTS idx_spend_wallet_created
    ON spend_logs (wallet_address, created_at);

-- ─── Wallet budgets ───────────────────────────────────────────────────────────
-- Optional per-wallet spending limits. Absence means unlimited.

CREATE TABLE IF NOT EXISTS wallet_budgets (
    wallet_address      TEXT        PRIMARY KEY,
    daily_limit_usdc    DECIMAL(18, 6),              -- NULL = unlimited
    monthly_limit_usdc  DECIMAL(18, 6),              -- NULL = unlimited
    total_spent_usdc    DECIMAL(18, 6) NOT NULL DEFAULT 0 CHECK (total_spent_usdc >= 0),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Auto-update updated_at on any row change
CREATE OR REPLACE FUNCTION update_wallet_budgets_updated_at()
RETURNS TRIGGER AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trg_wallet_budgets_updated_at ON wallet_budgets;
CREATE TRIGGER trg_wallet_budgets_updated_at
    BEFORE UPDATE ON wallet_budgets
    FOR EACH ROW EXECUTE FUNCTION update_wallet_budgets_updated_at();
