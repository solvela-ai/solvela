-- Per-tenant attribution: tag spend rows with an optional, free-form tenant
-- identifier supplied by a trusted upstream proxy (x-tenant header). Attribution
-- only in this migration — no budget/enforcement table or flag (those land in a
-- later migration). Additive, mirrors migration 003's ALTER + partial-index
-- pattern.
ALTER TABLE spend_logs ADD COLUMN IF NOT EXISTS tenant TEXT;
CREATE INDEX IF NOT EXISTS idx_spend_tenant ON spend_logs(wallet_address, tenant) WHERE tenant IS NOT NULL;
