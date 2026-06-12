-- Settlement-platform P2: client-facing payment receipts.
--
-- One row per PAID, delivered + settled request — written fire-and-forget
-- (tokio::spawn, never awaited on the hot path) at the same points where
-- spend logging fires: the chat path (streaming AND non-streaming — the #541
-- bug class) and the services proxy (vendor services at settlement time,
-- plain services post-upstream). Free-tier ($0) requests produce no payment
-- and no receipt.
--
-- The row id is the capability: an unguessable UUIDv4 returned to the payer
-- in the `x-solvela-receipt` response header and fetched via
-- GET /v1/receipts/{receipt_id}. There is deliberately no listing endpoint,
-- so the primary-key index is the only lookup path needed.
--
-- All amounts are atomic USDC (6 decimals, integer math — solvela-fintech);
-- the decimal strings in the wire shape are derived at read time.

CREATE TABLE IF NOT EXISTS receipts (
    id UUID PRIMARY KEY,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- Model id (chat path) or marketplace service id (proxy path) — same
    -- convention as spend_logs.model.
    model TEXT NOT NULL,
    -- x402 scheme that settled the payment ('exact' / 'escrow'). TEXT with a
    -- length cap rather than an enum CHECK so a future scheme stays
    -- recordable without a migration.
    payment_scheme TEXT NOT NULL CHECK (char_length(payment_scheme) <= 16),
    -- Payment transaction reference as recorded on the spend ledger (the
    -- signed transaction carried in the payment payload).
    tx_signature TEXT,
    payer_wallet TEXT NOT NULL,
    -- What the agent was actually billed (mirrors the spend ledger; equals
    -- total_atomic except when an escrow semantic-cache discount realised).
    amount_paid_atomic BIGINT NOT NULL CHECK (amount_paid_atomic >= 0),
    -- The cost breakdown that produced the bill (rule #5: provider cost +
    -- platform fee = total, truthful to what the agent pays).
    provider_cost_atomic BIGINT NOT NULL CHECK (provider_cost_atomic >= 0),
    platform_fee_atomic BIGINT NOT NULL CHECK (platform_fee_atomic >= 0),
    total_atomic BIGINT NOT NULL CHECK (total_atomic >= 0),
    -- Vendor-settled marketplace requests only (settlement-platform P1): the
    -- on-chain settlement leg plus Solvela's off-chain 5% fee receivable.
    -- All three are NULL on the chat path and for plain (gateway-recipient)
    -- services. Base58 32-byte Solana pubkeys are at most 44 characters.
    vendor_wallet TEXT CHECK (char_length(vendor_wallet) <= 44),
    vendor_settled_atomic BIGINT
        CHECK (vendor_settled_atomic IS NULL OR vendor_settled_atomic >= 0),
    vendor_fee_receivable_atomic BIGINT
        CHECK (vendor_fee_receivable_atomic IS NULL OR vendor_fee_receivable_atomic >= 0)
);
