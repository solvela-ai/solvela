-- 009_audit_actor_admin.sql
--
-- G5 H6: previously, audit entries written through the global admin token
-- carried `actor_wallet = NULL, actor_api_key = NULL`, so audit entries for
-- admin-token operations had no actor identity. If the admin token were
-- compromised, the audit log would record what changed but not who did it.
--
-- Add `actor_admin BOOLEAN NOT NULL DEFAULT FALSE`. The `log_audit` writer
-- sets it to TRUE for entries authenticated via the admin path. Combined
-- with `details->>'admin_token_hash_prefix'` (an 8-char SHA-256 prefix of
-- the admin bearer token), operators get an actor differentiator per
-- admin token without storing the plaintext.

ALTER TABLE audit_logs
    ADD COLUMN IF NOT EXISTS actor_admin BOOLEAN NOT NULL DEFAULT FALSE;

-- Partial index on admin entries so audit queries that filter on the
-- admin path stay fast as the table grows.
CREATE INDEX IF NOT EXISTS idx_audit_actor_admin
    ON audit_logs(created_at DESC)
    WHERE actor_admin = TRUE;
