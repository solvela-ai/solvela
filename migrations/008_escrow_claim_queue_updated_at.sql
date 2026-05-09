-- Add updated_at to escrow_claim_queue.
--
-- claim_queue::fetch_pending_claims references this column to recover stale
-- in_progress claims that have been stuck for more than 5 minutes (likely
-- abandoned due to a worker crash or SIGTERM). The column was missing from
-- the original migration 002, which made fetch_pending_claims fail at
-- runtime once the claim processor was enabled.
--
-- Reuses the generic update_updated_at_column() trigger function defined
-- in migration 005 so every UPDATE to a row refreshes updated_at.

ALTER TABLE escrow_claim_queue
    ADD COLUMN IF NOT EXISTS updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW();

DROP TRIGGER IF EXISTS trg_escrow_claim_queue_updated_at ON escrow_claim_queue;
CREATE TRIGGER trg_escrow_claim_queue_updated_at
    BEFORE UPDATE ON escrow_claim_queue
    FOR EACH ROW EXECUTE FUNCTION update_updated_at_column();
