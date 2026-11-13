-- Nano (XNO) receipt redemption store (#813).
--
-- A Nano send block is final on-chain and its hash never expires, so a
-- redeeming receipt must be recorded permanently. The `block_hash` primary key
-- is the exactly-once gate: the route verifies the receipt FIRST and only then
-- runs INSERT ... ON CONFLICT DO NOTHING, so a fresh hash records a row
-- (rows_affected = 1) and a replay records nothing (rows_affected = 0).
--
-- This table is deliberately independent of the 120 s transaction replay cache
-- in Redis: that TTL is right for a Solana recent-blockhash signature (which
-- expires on-chain) but useless for a hash that never expires.
--
-- The amounts are stored as text because Nano raw values are u128 (up to 10^38)
-- and do not fit a bigint; they are an audit record only — the spend ledger
-- stays micro-USD denominated.
CREATE TABLE IF NOT EXISTS nano_receipts (
    block_hash TEXT PRIMARY KEY,
    payer_account TEXT NOT NULL,
    amount_raw TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
