-- v0 off-chain spend-down channel (PR1, increment 1): data model only.
--
-- "Fund once, draw down." A channel is one USDC-SPL deposit credited after the
-- funding tx is verified on-chain, then drawn down off-chain by a sequence of
-- monotonic cumulative vouchers (ed25519-signed by the agent's session key).
-- v0 is CUSTODIAL: the gateway holds the prepaid balance; the signed-voucher
-- audit trail bounds what it can claim. The on-chain `solvela-channel` program
-- (a later, gated PR) moves custody into a PDA without changing the voucher
-- format — only the enforcer changes. See the PR2 scope doc.
--
-- All amounts are atomic USDC (6 decimals, integer math — solvela-fintech §1):
-- BIGINT (i64), CHECK (>= 0). Realistic deposits sit far below i64 max
-- (~9.2e12 USDC). The only atomic->f64 conversion remains the single existing
-- ledger-boundary cast in usage.rs; never store f64 here.
--
-- No query code accompanies this migration (increment 1 is the isolated
-- financial core + schema); the open/draw/settle/close query layer lands later.

CREATE TABLE IF NOT EXISTS channels (
    -- v0: a uuid string. end-state: the Channel PDA base58 (so the two slices
    -- unify on one identifier). TEXT either way.
    channel_id              TEXT PRIMARY KEY,
    -- base58 depositor wallet.
    agent_wallet            TEXT NOT NULL,
    -- base58 ed25519 voucher-signing pubkey; = agent_wallet unless a delegate
    -- session key is issued. This is the key vouchers are verified against.
    session_key             TEXT NOT NULL,
    -- = the gateway recipient wallet (single on-chain payee).
    provider                TEXT NOT NULL,
    -- USDC mint (base58).
    mint                    TEXT NOT NULL,
    -- On-chain-verified deposited principal; the hard ceiling on every draw.
    deposited_atomic        BIGINT      NOT NULL CHECK (deposited_atomic >= 0),
    -- Cumulative value already settled (paid out / accounted). Monotonic.
    settled_atomic          BIGINT      NOT NULL DEFAULT 0 CHECK (settled_atomic >= 0),
    -- Highest accepted voucher cumulative. Monotonic non-decreasing; the
    -- authoritative running total of fee-inclusive call costs on this channel.
    last_voucher_cumulative_atomic BIGINT NOT NULL DEFAULT 0
        CHECK (last_voucher_cumulative_atomic >= 0),
    -- Channel/voucher validity horizon (Solana slot). Nullable until set.
    expiry_slot             BIGINT,
    status                  TEXT        NOT NULL CHECK (status IN ('open', 'closing', 'closed')),
    -- end-state only (unilateral-close challenge window). 0/NULL = open.
    challenge_deadline_slot BIGINT,
    -- On-chain deposit reference, verified on-chain before crediting
    -- deposited_atomic (never trust a client-asserted amount).
    funding_tx_sig          TEXT,
    created_at              TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at              TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- The financial invariant, enforced at the DB layer as well as in code:
    --   settled_atomic <= last_voucher_cumulative_atomic <= deposited_atomic
    -- A row that violates it is corruption that could over-pay or strand funds;
    -- reject it here. (A fresh channel is settled=0, last=0, deposited=N: holds.)
    CONSTRAINT channels_settled_le_cumulative
        CHECK (settled_atomic <= last_voucher_cumulative_atomic),
    CONSTRAINT channels_cumulative_le_deposited
        CHECK (last_voucher_cumulative_atomic <= deposited_atomic)
);

-- Append-only audit log + replay defense (the v0 dispute evidence). The HIGHEST
-- cumulative_atomic per channel is authoritative; rows are never updated. The
-- voucher MUST be persisted before the response is returned (RFC §5 T6: a crash
-- must not lose the right to settle), even though the channel-row update is
-- fire-and-forget.
CREATE TABLE IF NOT EXISTS channel_vouchers (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    channel_id          TEXT        NOT NULL REFERENCES channels(channel_id),
    -- Signed running total at this draw (atomic, fee-inclusive).
    cumulative_atomic   BIGINT      NOT NULL CHECK (cumulative_atomic >= 0),
    -- This call's cost = cumulative - prev_cumulative = compute_actual_atomic_cost
    -- for the call (the 5% fee is already baked in at the single fee site).
    call_cost_atomic    BIGINT      NOT NULL CHECK (call_cost_atomic >= 0),
    -- Mirrors the signed-message fields, retained for audit + dispute.
    expiry_slot         BIGINT      NOT NULL,
    nonce               BIGINT      NOT NULL,
    -- SHA-256 of the canonical request body; binds voucher <-> request.
    request_digest      BYTEA       NOT NULL,
    -- ed25519 signature over the canonical voucher message (DOMAIN_SEP || ...).
    signature           BYTEA       NOT NULL,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- A cumulative value is signed at most once per channel: replay defense and
    -- the index that serves the "highest cumulative" lookup.
    UNIQUE (channel_id, cumulative_atomic)
);
