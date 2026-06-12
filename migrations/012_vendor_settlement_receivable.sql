-- Settlement-platform P1: per-service vendor_wallet (record + invoice).
--
-- "Vendor-Settlement Fee Mechanics" RFC (2026-06-12, mechanism C, vendor
-- absorbs): for marketplace services with a vendor_wallet, the agent's single
-- transfer settles directly to the vendor, and Solvela's 5% platform fee is
-- RECORDED here as an off-chain receivable (atomic USDC, integer) to be
-- invoiced to the vendor — never charged to the agent on-chain.
--
-- All three columns are NULL for non-vendor rows (chat path, plain proxy
-- services, free tier).

-- Base58 32-byte Solana pubkeys are at most 44 characters; anything longer
-- can never be a payable wallet, so the column refuses it outright.
ALTER TABLE spend_logs ADD COLUMN IF NOT EXISTS vendor_wallet TEXT
    CHECK (char_length(vendor_wallet) <= 44);
ALTER TABLE spend_logs ADD COLUMN IF NOT EXISTS vendor_settled_atomic BIGINT
    CHECK (vendor_settled_atomic IS NULL OR vendor_settled_atomic >= 0);
ALTER TABLE spend_logs ADD COLUMN IF NOT EXISTS vendor_fee_receivable_atomic BIGINT
    CHECK (vendor_fee_receivable_atomic IS NULL OR vendor_fee_receivable_atomic >= 0);

-- Invoice aggregation: receivables are summed per vendor wallet.
CREATE INDEX IF NOT EXISTS idx_spend_vendor_wallet
    ON spend_logs (vendor_wallet)
    WHERE vendor_wallet IS NOT NULL;
