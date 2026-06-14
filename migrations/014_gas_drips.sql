-- Gas-drip faucet idempotency + daily-cap ledger.
--
-- The gateway drips a dust of native SOL to USDC-funded agent wallets so the
-- agent (the on-chain fee payer for `exact` / `escrow` payments) can pay its
-- own network gas. The user funds USDC only.
--
-- The PRIMARY KEY on `wallet_address` is the once-per-wallet-ever idempotency
-- guard: the faucet route does an `INSERT ... ON CONFLICT (wallet_address) DO
-- NOTHING` as a *reservation* BEFORE sending the transfer. A conflict (0 rows
-- inserted) means the wallet was already handled (or is being handled
-- concurrently) — the PK serializes concurrent requests so we never
-- double-drip. `tx_signature` is filled in AFTER a successful send; a row with
-- a NULL `tx_signature` (a crash between reserve and send-success) correctly
-- continues to block a re-drip — the safe failure direction (under-drip, not
-- double-drip).
CREATE TABLE IF NOT EXISTS gas_drips (
    wallet_address TEXT PRIMARY KEY,
    lamports       BIGINT NOT NULL CHECK (lamports >= 0),
    tx_signature   TEXT,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- The global daily-cap query filters on a sargable half-open UTC range
-- (`created_at >= start_of_utc_day AND created_at < start_of_utc_day + 1 day`),
-- which lets the planner use a plain btree on `created_at`. A functional index
-- on `(created_at::date)` is NOT usable here (and is itself invalid: casting a
-- `timestamptz` to `date` is STABLE, not IMMUTABLE — Postgres rejects it with
-- 42P17, aborting the migration). Index the raw column instead.
CREATE INDEX IF NOT EXISTS idx_gas_drips_created_at
    ON gas_drips (created_at);
