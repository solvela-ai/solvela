-- Nano receipt reconciliation columns (#813 review, 2026-11-17).
--
-- `request_id` ties a redeemed block hash to the chat request that consumed
-- it, and `outcome` records whether that call was served. The row is written
-- with outcome = 'consumed' when the receipt is verified and reserved (before
-- the provider call); after the provider responds it is updated to 'served'.
-- If the provider call fails, the row is left at 'provider_failed': the XNO
-- has already moved and the receipt cannot be replayed, so an operator can
-- find these rows and issue a manual credit/refund.
--
-- Forward-only and idempotent, per migrations/AGENTS.md.
ALTER TABLE nano_receipts
    ADD COLUMN IF NOT EXISTS request_id TEXT;

ALTER TABLE nano_receipts
    ADD COLUMN IF NOT EXISTS outcome TEXT NOT NULL DEFAULT 'consumed';

-- Operators query the paid-but-undelivered set for refunds; keep that cheap.
CREATE INDEX IF NOT EXISTS nano_receipts_outcome_idx ON nano_receipts (outcome);
