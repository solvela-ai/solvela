-- Routing telemetry: persist the smart-router classification on each spend
-- row so the real-world tier distribution is auditable (today the tier/score
-- are computed per request and discarded — the only sink is a debug header).
--
-- Attribution/observability only — these columns never gate or change
-- billing. Both are NULL on paths that never run the router (service proxy,
-- search, A2A) and on rows written before this migration. Additive, mirrors
-- migration 010's ALTER pattern.
ALTER TABLE spend_logs ADD COLUMN IF NOT EXISTS routing_tier TEXT;
ALTER TABLE spend_logs ADD COLUMN IF NOT EXISTS routing_score DOUBLE PRECISION;
