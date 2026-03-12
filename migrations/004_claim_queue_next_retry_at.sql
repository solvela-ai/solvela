-- Add next_retry_at column for exponential backoff on claim retries.
-- Claims won't be picked up before their backoff period expires.

ALTER TABLE escrow_claim_queue
    ADD COLUMN IF NOT EXISTS next_retry_at TIMESTAMPTZ;

-- Composite index for the pending-claims query: filter by status + next_retry_at
CREATE INDEX IF NOT EXISTS idx_claim_queue_pending_retry
    ON escrow_claim_queue (status, next_retry_at)
    WHERE status = 'pending';
