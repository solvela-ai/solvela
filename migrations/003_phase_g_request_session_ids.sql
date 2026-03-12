-- Phase G: Add request_id and session_id tracking to spend_logs
ALTER TABLE spend_logs ADD COLUMN IF NOT EXISTS request_id TEXT DEFAULT NULL;
ALTER TABLE spend_logs ADD COLUMN IF NOT EXISTS session_id TEXT DEFAULT NULL;
CREATE INDEX IF NOT EXISTS idx_spend_session ON spend_logs(session_id) WHERE session_id IS NOT NULL;
