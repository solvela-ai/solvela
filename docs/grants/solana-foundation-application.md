# Solana Foundation Grants Program — Application Draft

> Working draft. Submit at https://solana.org/grants. Cross-reference numbers
> against `STATUS.md` and the live `https://api.solvela.ai/health` /
> `/v1/escrow/config` endpoints before submitting — grant evaluators do this
> check themselves and stale numbers are an instant filter.

---

## Project name

**Solvela** — Solana-native AI agent payment gateway

## One-line description

The reference x402 payment gateway for Solana, with a deployed Anchor escrow program and a proxy that routes paid LLM requests across 5 providers in USDC-SPL.

## Track

**Public Goods / Ecosystem Infrastructure.** Solvela is open-source (gateway BUSL-1.1 transitioning to MIT; libraries MIT; SDKs MIT). It targets a chain-agnostic standard (x402) but ships Solana as the first-class implementation, which closes a real gap: Coinbase's reference x402 implementation is EVM-first, and there has been no production-quality Solana counterpart.

## Funding amount requested

**USD $50,000** (or equivalent SOL/USDC). Use of funds detailed below.

## Team

- **Kenneth Dixon** — sole maintainer, full-stack engineer. Background: [add 2–3 sentences from your real bio — prior employers, open-source projects, anything that signals "ships things"]. Located in the United States.
- Solo project. No token, no DAO, no anonymous co-founders. Contact: `kd@sky64.io`.

## What is Solvela?

Solvela is a payment-required HTTP gateway in front of LLM provider APIs. Clients send standard OpenAI-compatible chat-completion requests; the gateway responds `402 Payment Required` with a USDC-SPL price quote; the client signs a Solana `TransferChecked` transaction (or, optionally, deposits into an Anchor escrow PDA); the gateway verifies the payment on-chain and proxies the request to the optimal provider. Settlement is non-custodial: USDC moves from the client's wallet to the operator's wallet (or the escrow PDA) in a single Solana transaction, and the gateway never holds funds.

The implementation follows the [x402 specification](https://www.x402.org/) — the open HTTP-payment standard originally proposed by Coinbase — and adds:

- Two payment schemes: direct `TransferChecked` (one-shot) and trustless Anchor escrow (deposit / claim / refund with PDA vault) for use cases that need atomic chargeback.
- A 15-dimension rule-based smart router that classifies requests and selects the cheapest model meeting the requester's quality bar, microsecond-scale, no LLM-in-the-loop.
- Multi-provider adapters: OpenAI, Anthropic, Google, xAI, DeepSeek behind one OpenAI-compatible endpoint.
- A service marketplace: any third-party x402-enabled service (any chain, any provider) can register and be proxied through the gateway with a 5% platform fee.

## Why this matters for Solana

Three concrete reasons.

1. **Stablecoin utility.** USDC-SPL volume is one of Solana's strongest narratives. AI-agent payments are projected by Coinbase / a16z / Solana Foundation public posts to be a significant share of stablecoin transaction count by 2027. The current default rail for those payments is EVM (Coinbase x402 reference, Stripe Agent Toolkit on cards, AP2 on cards). Solvela puts a working, mainnet-deployed alternative on Solana.
2. **Reference implementation.** Anyone on Solana who wants to charge for an HTTP service in USDC currently has to invent the wire format, the verification logic, the replay protection, the ATA derivation, and the escrow flow themselves. We've packaged all of that into MIT-licensed crates (`solvela-x402`, `solvela-protocol`) and an Anchor program. A second Solana team building agent payments shouldn't have to re-derive `TransferChecked` discriminators from scratch.
3. **RPC and program economic activity.** Every gateway request triggers a Solana RPC call (verify) and, in the escrow path, a program invocation (claim/refund). Adoption directly drives Solana network usage. Helius, Triton, and QuickNode benefit; the validator set benefits.

## Traction (please verify against live endpoints)

- **Mainnet escrow program deployed:** `9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU`. View on Solscan or [solana.fm](https://solana.fm/address/9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU).
- **Production gateway:** `https://api.solvela.ai/health` — Fly.io ord region. P99 latency under 300 ms at 400 RPS sustained per `loadtest/`.
- **Crates.io publishes:** `solvela-protocol`, `solvela-x402`, `solvela-router`, `solvela-cli` at v0.2.x.
- **5 LLM providers integrated** in production: OpenAI, Anthropic, Google, xAI, DeepSeek. 26 models behind a unified pricing endpoint.
- **SDK matrix:** Python, TypeScript, Go, Rust, MCP server (Claude / Cursor / Claude Desktop / OpenClaw), Vercel AI SDK provider.
- **Test suite:** 400+ Rust tests; 4 required CI checks gate every merge.
- **Regulatory posture:** documented at [`docs/product/regulatory-position.md`](../product/regulatory-position.md). Non-custodial, no fiat conversion, no MSB triggers — vetted before mainnet deployment. This is unusual to ship at this stage and removes a meaningful category of risk for downstream Solana ecosystem adopters.

## What we'll do with the grant

| Bucket | Amount | Outcome |
|---|---|---|
| **Independent third-party security audit of the Anchor escrow program** (target: Neodyme, OtterSec, or Halborn) | $25,000 | Public audit report. Lifts the program from "deployed but un-audited" to "audited and published," which removes the largest deployment blocker for downstream Solana protocols who'd otherwise have to commission their own audit before integrating. |
| **6 months of runtime infrastructure** (Solana mainnet RPC at production tier with Helius or Triton, Fly.io gateway hosting, Upstash Redis, monitoring) | $9,000 | Keeps `api.solvela.ai` available to non-paying users at sub-300ms p99 across the grant period, regardless of fee revenue. |
| **Reference integrations** with three Solana-ecosystem agent frameworks (e.g., `ai16z/eliza`, `solana-agent-kit`, one DePIN consumer such as Nosana) | $8,000 | Open-source PRs with Solvela payment hooks. Each one becomes a tutorial and a permanent reference for downstream builders. |
| **Public Solana payment dashboard** showing real-time on-chain USDC volume routed through the gateway, with on-chain transaction links | $4,000 | Verifiable, no-trust-required adoption metric for ecosystem stakeholders. |
| **Conference / Breakpoint / Accelerate talk preparation and travel** | $4,000 | One technical talk on x402-on-Solana at a Solana ecosystem event in the grant period. |
| Total | **$50,000** | |

Funds will be received in USDC to a dedicated multisig wallet (or the Solana Foundation's preferred disbursement mechanism) and tracked in a public ledger.

## Milestones

| Month | Deliverable | Verifiable by |
|---|---|---|
| 1 | Audit firm engaged, scope signed | Public engagement letter |
| 2 | First reference integration (`solana-agent-kit`) merged | Upstream PR link |
| 3 | Audit fieldwork complete; first findings shared privately with maintainer | Audit firm communication |
| 4 | Audit report published; public dashboard live at `metrics.solvela.ai` | Live URL |
| 5 | Second reference integration shipped | Upstream PR link |
| 6 | Third reference integration shipped; ecosystem talk delivered | Upstream PR link, talk recording |

If a milestone slips, we report it in a public update post within two weeks of the original target. We do not consider funds spent until the corresponding milestone is verifiable on-chain or in public PRs.

## Open-source commitment

- All code under `crates/protocol`, `crates/x402`, `crates/router`, `crates/cli`, and `programs/escrow` is **MIT**. The gateway is BUSL-1.1 with a generous Additional Use Grant (free under $1M annual revenue derived from the gateway, free for internal first-party production, free for any non-production use) and converts to MIT four years after each release. SDKs are MIT.
- Contributions require DCO sign-off (`git commit -s`); no separate CLA. Enforced in CI.
- The grant does not change the license terms. There is no token. There will not be a token created as a result of this grant.

## Why this is a good fit for Solana Foundation

- **Public good.** MIT-licensed protocol crates and a deployed-on-mainnet Anchor program that anyone can integrate against without commercial entanglement.
- **Cross-cutting.** Drives RPC volume (validators, RPC providers), USDC volume (Circle, ecosystem stablecoin metrics), agent ecosystem (Eliza/AgentKit), and DePIN payment use cases (compute providers).
- **Real, not narrative.** Mainnet program, live gateway, real provider integrations, real load-test numbers — most of the stack already exists. The grant accelerates audit + adoption, not greenfield development.
- **Aligned regulatory posture.** Non-custodial, no MSB triggers. Solana Foundation's risk surface is smaller granting to Solvela than to most agent-payment plays.

## Public artifacts

- Repo: https://github.com/solvela-ai/solvela
- Status: https://github.com/solvela-ai/solvela/blob/main/STATUS.md
- Regulatory position: https://github.com/solvela-ai/solvela/blob/main/docs/product/regulatory-position.md
- Mainnet program: `9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU`
- Live gateway: https://api.solvela.ai
- Docs: https://docs.solvela.ai
- License explanation: https://github.com/solvela-ai/solvela#licensing
- Crates: https://crates.io/crates/solvela-protocol, /solvela-x402, /solvela-router, /solvela-cli

## Out of scope (explicit)

To save the evaluator a question:

- We will not launch a token as part of or after this grant.
- We will not custody user funds. The gateway never holds USDC except as the recipient of completed payments.
- We will not add fiat conversion. If a downstream user wants fiat, that's a separate stack we will not build.
- We will not request additional grants from the Solana Foundation in the same calendar year unless explicitly invited.

---

## Submission checklist (delete before sending)

- [ ] Replace bio placeholder in "Team" section with real bio
- [ ] Verify all numbers in "Traction" against current STATUS.md
- [ ] Verify mainnet program ID has not changed
- [ ] Confirm Fly.io / Vercel deploy is green via /health
- [ ] Confirm a quote from at least one of Neodyme / OtterSec / Halborn for the audit before quoting $25k as a hard number
- [ ] Decide whether to disclose any prior funding (none currently — say so explicitly if asked)
- [ ] Loom demo: 2 min, agent makes a real USDC payment end-to-end. Link in submission cover letter.
- [ ] Cover letter framing: "x402 reference for Solana, with deployed mainnet program and live gateway. Funding accelerates audit + ecosystem integration; we are not asking for greenfield R&D."
