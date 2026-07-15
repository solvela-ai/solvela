# Feature state — live vs. dormant

> What's actually **on** in production, what's **built but off** (the dormant surface we
> keep so we can turn it on when the moment comes), and what's **not built**. Every dormant
> feature lists the exact switch to flip it. See [`STATUS.md`](../../STATUS.md) for the
> chronological deploy log.
>
> Last reviewed: 2026-07-14 late (Fly v463).
>
> **Correction (2026-07-14):** the first revision of this page listed escrow and the payment
> channel as dormant. A prod secrets audit showed both had been **enabled since ~2026-07-05**
> (when the channel disbursement slice shipped). This page now reflects the audited state.

## Live in production

These are on right now and serve real traffic.

| Feature | Notes |
|---|---|
| **Exact x402 USDC-SPL payment** (mainnet) | The core path. `PAYMENT-SIGNATURE` → verify → proxy. This is what Telsi uses. |
| **5% platform fee + `cost_breakdown`** | On every request. |
| **OpenAI-compatible API** | `POST /v1/chat/completions`, `GET /v1/models` — 44 models across 6 providers. |
| **Native `/v1/messages` Anthropic relay** | Byte-passthrough for Anthropic-resolved models; preserves thinking `signature`s, `tool_use`, cache-token usage (#635/#647/#651). |
| **SSE streaming** | Token-by-token, including real Gemini/Google streaming (#723/#724). |
| **Smart router** | `eco`/`auto`/`premium`/`free` profiles + rule-based 15-dimension scorer. |
| **Free NVIDIA tier + fast-failover** | 15 free ($0/$0) models; a hung free upstream fails over in ~10s (#725). |
| **Response cache — Tier 1 (exact)** | Hashes `(model, messages, temperature)`; wallet-agnostic (Rule #16). |
| **Semantic cache — Tier 2** | **Enabled 2026-07-14.** `bge-small-en-v1.5` embeddings + RediSearch KNN; near-duplicate prompts hit the cache. |
| **A2A protocol adapter (v0.3)** | `/.well-known/agent-card.json`, `message/send` → x402 + chat pipeline. |
| **Agent toolbelt** | `POST /v1/search`, `POST /v1/solana/price`, discovery via `GET /v1/services` (#708/#712). |
| **`/metrics`** | Prometheus, gated by `SOLVELA_ADMIN_TOKEN` (Bearer). |
| **Org / team / API-key / audit / budget** | Enterprise hierarchy; active when `DATABASE_URL` is set. |
| **Escrow payment scheme** (mainnet) | `SOLVELA_SOLANA__ESCROW_PROGRAM_ID` set since ~2026-07-05; 402 advertises `exact` + `escrow`. Deposit→claim→expiry-refund proven end-to-end on devnet 2026-07-14. Caveat: upgrade authority is a warm single-sig ([#175](https://github.com/solvela-ai/solvela/issues/175) — Squads multisig is the fast-follow). |
| **Payment-channel voucher settlement** (mainnet) | `SOLVELA_CHANNEL__ENABLED=true` since ~2026-07-05, bounded: 100 USDC max deposit, 500 USDC/day refund cap. Open→draw→close→refund proven on devnet 2026-07-14. Refund phantom-confirm ([#743](https://github.com/solvela-ai/solvela/issues/743)) fixed in [#746](https://github.com/solvela-ai/solvela/pull/746) (v463): per-obligation memo + landed-tx verification before any row is stamped confirmed. |
| **Gas-drip faucet** (USDC-only onboarding, mainnet) | **Enabled 2026-07-14** (v462) after Tier-3 hardening ([#742](https://github.com/solvela-ai/solvela/pull/742)) and a full devnet drip proof. Small capped float: 0.01 SOL per drip, 1 SOL/day cap, once-per-wallet, requires ≥0.1 USDC in the wallet. |

## Dormant — built, off by default

**Currently empty.** Escrow, the payment channel, and the faucet all graduated to live
(see above). Their kill switches remain one env flip away — unset
`SOLVELA_SOLANA__ESCROW_PROGRAM_ID`, set `SOLVELA_CHANNEL__ENABLED=false`, or set
`SOLVELA_FAUCET__ENABLED=false` — and the gateway degrades to the exact-only core with no
redeploy. (The channel refund worker deliberately keeps draining already-frozen refund
obligations even when `channel.enabled=false`.)

## Not built / parked

Intentionally not implemented. The seams exist; the work doesn't.

| Feature | State |
|---|---|
| **Base / EVM path** | Parked. `PaymentVerifier` is chain-agnostic by design, but there is no EVM implementation. Revisit on real funding-friction demand. |
| **Multi-currency stablecoins** (EURC/AUDD/FX) | Parked. No customer ask; not worth the FX/mint surface yet. |
