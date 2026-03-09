-- Durable escrow claim queue with retry tracking
-- One row per pending escrow claim. Processed by the background claim worker.

CREATE TABLE IF NOT EXISTS escrow_claim_queue (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    service_id       BYTEA       NOT NULL,                -- 32-byte escrow service ID
    agent_pubkey     TEXT        NOT NULL,                -- base58 agent wallet
    claim_amount     BIGINT      NOT NULL,                -- atomic USDC units
    deposited_amount BIGINT,                              -- verified deposit (cap)
    status           TEXT        NOT NULL DEFAULT 'pending',  -- pending | in_progress | completed | failed
    attempts         INTEGER     NOT NULL DEFAULT 0,
    last_attempt_at  TIMESTAMPTZ,
    tx_signature     TEXT,                                -- filled on success
    error_message    TEXT,                                -- last error
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at     TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_claim_queue_status
    ON escrow_claim_queue (status);

CREATE INDEX IF NOT EXISTS idx_claim_queue_created
    ON escrow_claim_queue (created_at);
