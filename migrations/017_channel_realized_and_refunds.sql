-- v0 spend-down channel: synchronous `realized` counter + refund reservations
-- (PR-A of the channel close-disbursement plan).
--
-- `realized_atomic` = the sum of per-call ACTUAL fee-inclusive costs — the
-- agent's true obligation. It is advanced ONLY inside the draw's single
-- persist transaction (`persist_voucher_and_advance`), so it is SYNCHRONOUS
-- with `last_voucher_cumulative_atomic`. It is a NEW column, deliberately NOT
-- a rename/reuse of the async `settled_atomic` (which lags at ~0 in v0 —
-- keying a refund on it re-opens the A1 over-refund money loss). The
-- cooperative-close refund is `deposited - realized`:
--   deposited - settled  = A1 over-refund (hands back value already drawn)
--   deposited - last     = #600 strand (locks the quote-vs-actual gap forever)
--   deposited - realized = correct (returns exactly what was never consumed)
--
-- All amounts are atomic USDC (BIGINT, integer math — solvela-fintech §1).

-- Column add + backfill in ONE guarded block: the backfill runs ONLY when the
-- column is being created, i.e. only against pre-017 rows — every one a
-- /v1/search draw, where quote == actual, so the realized total IS the voucher
-- total (`realized := last`). Guarding on column-existence (not on a value
-- predicate like `realized = 0`) makes a raw disaster-recovery re-apply
-- semantically safe: post-017 a channel can legitimately sit at
-- realized = 0 with last > 0 (a fully-discounted chat draw), and a re-run
-- value-predicate backfill would silently inflate its obligation.
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_name = 'channels' AND column_name = 'realized_atomic'
    ) THEN
        ALTER TABLE channels
            ADD COLUMN realized_atomic BIGINT NOT NULL DEFAULT 0
                CHECK (realized_atomic >= 0);
        UPDATE channels
           SET realized_atomic = last_voucher_cumulative_atomic;
    END IF;
END $$;

-- CHECK chain, guarded (`ADD CONSTRAINT IF NOT EXISTS` is not Postgres SQL).
-- Combined with 015's `settled <= last <= deposited` this yields the full
-- invariant: settled <= realized <= last <= deposited.
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'channels_settled_le_realized'
          AND conrelid = 'channels'::regclass
    ) THEN
        ALTER TABLE channels
            ADD CONSTRAINT channels_settled_le_realized
                CHECK (settled_atomic <= realized_atomic);
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
        WHERE conname = 'channels_realized_le_cumulative'
          AND conrelid = 'channels'::regclass
    ) THEN
        ALTER TABLE channels
            ADD CONSTRAINT channels_realized_le_cumulative
                CHECK (realized_atomic <= last_voucher_cumulative_atomic);
    END IF;
END $$;

-- Refund reservations: one row per closed channel = the frozen refund
-- obligation. The FULL send tuple (amount, destination_wallet, mint) is frozen
-- inside the close transaction; the worker never recomputes ANY of it at send
-- time — a second read of a mutable ledger (or of config, for the asset) is a
-- TOCTOU on money. In particular the mint is the DEPOSIT's asset from
-- `channels.mint`, never a config read: a config mint change between close and
-- send must not move a different token.
--
-- Exactly-once discipline:
--  * `channel_id` PRIMARY KEY pins one OBLIGATION (one row, ever);
--  * the worker's status-predicated, payload-carrying claim CAS
--    (`UPDATE ... WHERE status = 'reserved'`, rows_affected-gated, committed
--    BEFORE first broadcast) pins one SIGNER;
--  * `signed_tx` holds the exact wire bytes so every retry rebroadcasts the
--    SAME transaction (the signature is the ledger-level dedupe);
--  * `last_valid_block_height` powers the conclusive-death check (which MUST
--    use the history-searching signature-status form);
--  * `first_broadcast_at` (set once, inside the claim CAS) anchors the
--    trailing-24h global daily-cap window.
--
-- Status machine (NO terminal-abandon state — a refund is owed money):
--   reserved  -> in_flight (claim CAS) | confirmed (amount == 0)
--   in_flight -> confirmed | in_flight (signature-predicated re-sign) | held
--   held      -> reserved (operator re-arm ONLY; entry fires an alert)
--   confirmed -> terminal (channel -> 'closed' in the same transaction)
CREATE TABLE IF NOT EXISTS channel_refunds (
    channel_id              TEXT        PRIMARY KEY REFERENCES channels(channel_id),
    -- Frozen at close: deposited_atomic - realized_atomic (checked in code;
    -- 0 is legal — a fully-drawn channel — and completes without a tx).
    amount_atomic           BIGINT      NOT NULL CHECK (amount_atomic >= 0),
    -- Frozen from channels.agent_wallet at close — never caller-supplied.
    destination_wallet      TEXT        NOT NULL,
    -- Frozen from channels.mint at close — the deposit's asset, never config.
    mint                    TEXT        NOT NULL,
    status                  TEXT        NOT NULL DEFAULT 'reserved'
        CHECK (status IN ('reserved', 'in_flight', 'confirmed', 'held')),
    -- base58 signature of the signed refund tx; persisted by the claim CAS
    -- BEFORE the first broadcast.
    tx_signature            TEXT,
    -- The exact signed wire bytes; rebroadcasts reuse them verbatim.
    signed_tx               BYTEA,
    last_valid_block_height BIGINT,
    attempts                INT         NOT NULL DEFAULT 0,
    first_broadcast_at      TIMESTAMPTZ,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- One-shot legacy backfill: v0 channels already `closing` (their close
-- recorded the obligation before this table existed) get their reservation
-- here, frozen from the same tuple the close transaction freezes. Their
-- `realized` was backfilled above (= last, correct for search-only history,
-- where quote == actual). Idempotent via ON CONFLICT; the subtraction cannot
-- go negative under the CHECK chain (realized <= last <= deposited).
INSERT INTO channel_refunds (channel_id, amount_atomic, destination_wallet, mint, status)
SELECT channel_id,
       deposited_atomic - realized_atomic,
       agent_wallet,
       mint,
       'reserved'
  FROM channels
 WHERE status = 'closing'
ON CONFLICT (channel_id) DO NOTHING;
