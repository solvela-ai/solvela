-- Routing telemetry: persist the smart-router classification on each spend
-- row so the real-world tier distribution is auditable (today the tier/score
-- are computed per request and discarded — the only sink is a debug header).
--
-- Attribution/observability only — these columns never gate or change
-- billing. Both are NULL whenever the smart router produced no classification:
-- paths that never invoke it (service proxy, search, A2A), chat requests that
-- bypass it via a direct model ID or an alias, and rows written before this
-- migration. A non-NULL value therefore always denotes a real classification —
-- including a genuine score of 0.0. (The "N/A"/0.0 debug-header sentinel is
-- mapped to NULL before it reaches this table; see routes::chat::
-- routing_telemetry.) Additive, mirrors migration 010's ALTER pattern.
ALTER TABLE spend_logs ADD COLUMN IF NOT EXISTS routing_tier TEXT;
ALTER TABLE spend_logs ADD COLUMN IF NOT EXISTS routing_score DOUBLE PRECISION;
