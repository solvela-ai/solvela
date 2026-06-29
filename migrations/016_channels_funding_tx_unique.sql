-- v0 channel funding-replay defense (PR1, increment 2).
--
-- A channel's deposited_atomic is credited from an ON-CHAIN-VERIFIED funding
-- transfer. The same funding transaction must never credit two channels, or the
-- custodial gateway would owe 2x while holding 1x (a custodial-double-credit /
-- funds-loss hole). The open path settles (broadcasts) the funding tx, so
-- Solana's transaction dedup already rejects a re-presented funding tx at the
-- chain layer; this partial unique index is the belt-and-suspenders LEDGER guard
-- so a duplicate `funding_tx_sig` can never be inserted even if the chain-level
-- dedup window is somehow bypassed. `create_channel` uses
-- `ON CONFLICT (funding_tx_sig) DO NOTHING` against it and treats 0 rows as
-- "funding tx already used".
--
-- Partial (WHERE funding_tx_sig IS NOT NULL): a row with no funding reference
-- yet (NULL) is not constrained — Postgres treats NULLs as distinct under a
-- plain UNIQUE anyway, but the predicate makes the intent explicit and keeps the
-- index from indexing NULLs.
CREATE UNIQUE INDEX IF NOT EXISTS channels_funding_tx_sig_key
    ON channels (funding_tx_sig)
    WHERE funding_tx_sig IS NOT NULL;
