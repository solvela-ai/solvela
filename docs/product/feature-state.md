# Feature state — live vs. dormant

> What's actually **on** in production, what's **built but off** (the dormant surface we
> keep so we can turn it on when the moment comes), and what's **not built**. Every dormant
> feature lists the exact switch to flip it. See [`STATUS.md`](../../STATUS.md) for the
> chronological deploy log.
>
> Last reviewed: 2026-07-14 (Fly v457).

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

## Dormant — built, off by default

Fully built and merged, shipped **disabled**. This is deliberate optionality — the switch is
already there; flipping it is a config change, not a build. Do **not** enable on mainnet
without proving the money path on devnet first.

| Feature | Off switch (default) | How to turn on | Before enabling on mainnet |
|---|---|---|---|
| **Escrow payment scheme** (trustless USDC-SPL escrow program) | `solana.escrow_program_id` unset → 402 advertises `exact` only | Set `SOLVELA_SOLANA__ESCROW_PROGRAM_ID=<program id>` | Prove deposit→claim→refund on devnet. Note: escrow program upgrade authority is a warm single-sig ([#175](https://github.com/solvela-ai/solvela/issues/175), open — Squads multisig is the fast-follow). |
| **Payment-channel voucher settlement** (spend-down channel) | `channel.enabled = false` | `SOLVELA_CHANNEL__ENABLED=true` | Prove open→draw→refund on devnet; refund worker must be running. |
| **Gas-drip faucet** (USDC-only onboarding) | `faucet.enabled = false` (also inert without a source key) | `SOLVELA_FAUCET__ENABLED=true` **and** `SOLVELA_FAUCET__SOURCE_KEY=<key>` | Finish abuse hardening (Tier-3 F8–F21) + live devnet drip; cap the SOL float and rate-limit. |

## Not built / parked

Intentionally not implemented. The seams exist; the work doesn't.

| Feature | State |
|---|---|
| **Base / EVM path** | Parked. `PaymentVerifier` is chain-agnostic by design, but there is no EVM implementation. Revisit on real funding-friction demand. |
| **Multi-currency stablecoins** (EURC/AUDD/FX) | Parked. No customer ask; not worth the FX/mint surface yet. |
