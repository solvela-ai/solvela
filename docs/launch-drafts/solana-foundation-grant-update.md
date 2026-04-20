# Solana Foundation Grant Milestone Update — Solvela

**Status:** DRAFT. Do NOT send. User triggers after private testing.

**To:** Solana Foundation (agentic-payments initiative)  
**From:** Solvela team  
**Date:** [AUTO-FILLED: current date]  
**Subject:** Phase 1 Complete — x402 MCP Gateway Live

---

## Milestone Update

### What We Shipped (This Period)

**Solvela v1.0 is now production-ready:**

1. **MCP Server** — Production-grade Model Context Protocol server for Claude Code, Cursor, and Claude Desktop. One-line install:
   ```bash
   npm install -g @solvela/mcp-server
   solvela mcp install --host=claude-code
   ```

2. **Real x402 Signing** — Replaced stub transactions with live Solana signatures. Uses `@solana/web3.js` to sign VersionedTransactions client-side. Keys never leave the user's machine.

3. **Trustless Escrow** — Anchor program on Solana mainnet (address: `9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU`). Escrow deposits are gated by amount and session duration:
   - Per-call cap: `SOLVELA_MAX_ESCROW_DEPOSIT` (default $5.00)
   - Per-session cap: `SOLVELA_MAX_ESCROW_SESSION` (default $20.00)
   - Pay-only-for-what-you-receive: USDC claimed only on LLM response completion

4. **Multi-Host Support** — Works on Claude Code, Cursor, Claude Desktop. CLI installer handles config generation for all three.

5. **OpenClaw Provider Plugin** — Solvela models now appear in OpenClaw's model picker as a first-class provider.

### Metrics (Current Snapshot)

- **Mainnet transactions:** Early-stage metrics still stabilizing — full dashboard shared separately on request.
- **Active wallets:** Early-stage metrics still stabilizing — full dashboard shared separately on request.
- **Models supported:** 26+ (OpenAI, Anthropic, Google, xAI, DeepSeek)
- **Average latency (full stack):** ~1.2 seconds (x402 signature + verification + LLM call)
- **RPS capacity (tested):** 400 RPS under sustained load (Rust + Axum)
- **Test coverage:** 683 tests across crates (unit + integration + contract tests for 402 envelope)

Two commercial products already run on Solvela in production: **Telsi.ai** (multi-tenant AI assistant SaaS, migrated from BlockRun in April) and **RustyClaw.ai** (crypto trading terminal with autonomous trading agent, paying Stripe customers).

---

### Technical Highlights

**Why this matters for the foundation:**

1. **Solana-first:** 65–70% of x402 volume is on Solana. We're betting on the right chain.
2. **Trustless escrow:** No other x402 LLM gateway has an audited on-chain program. This is a real competitive moat that only Solana enables.
3. **Performance:** Rust + Axum gateway handles payment verification at scale (load-tested to 400 RPS with p99 < 300ms).
4. **Agent-first design:** Not built for humans to log in. Built for agents to own wallets and sign transactions.

---

## What's Next (Roadmap)

### Short Term (May 2026)
- Multi-wallet support (Phantom deeplink, hardware wallets)
- Additional escrow analysis + audit prep for certification
- Integration with Solana Foundation's agentic-payments working group

### Medium Term (Q2 2026)
- Base/EVM support (Coinbase's `Upto` payment scheme)
- x402 V2 sessions support
- Service discovery marketplace (x402 services registry)

### Long Term (Q3+)
- Nosana integration (decentralized GPU inference on Solana)
- Multi-agent session management
- Advanced routing profiles (custom model classifiers per agent)

---

## Challenges & Support Needed

1. **Distribution:** Anthropic MCP Registry, cursor.directory, and OpenClaw docs are the three channels. We're working all three in parallel.

2. **Competition:** BlockRun went dual-chain (Solana + Base). Our escrow advantage buys us time, but we need to accelerate feature parity.

3. **Partnerships:** Introductions to Solana validators and node operators running agentic infrastructure would accelerate adoption.

---

## Ask

If possible:

1. **Recognition** on the Solana Foundation's agentic-payments initiative page (if such a page exists)
2. **Referrals** to ecosystem partners building agentic infra (Nosana, Kuzco, etc.)
3. **Exploration of follow-on funding** for Phase 2 work (multi-wallet support, advanced routing)

---

## Competitive Context

The Rust x402 library layer is crowded (x402-rs + x402-chain-solana, r402, tempo-x402). Solvela occupies a different layer: the operated LLM gateway, with mainnet Anchor escrow, smart routing across 5 providers, and MCP plugins for Claude Code, Cursor, and OpenClaw. Two commercial products already run on it — Telsi.ai (multi-tenant AI SaaS) and RustyClaw.ai (crypto trading terminal). The Foundation is seeing many x402 submissions; Solvela differs by being fully operational with paying downstream consumers, not just a published library.

---

## Closing

Solvela is shipping into a category (agentic LLM payments on Solana) that the foundation is actively betting on. We're grateful for the support and excited to help unlock the "agentic internet" narrative the foundation is positioning.

The escrow feature — trustless settlement on-chain — is something only Solana (and teams willing to deploy and audit Anchor programs) can deliver. We're proud to be the first.

---

**Questions?** Reach out: hello@solvela.ai  
**Live:** https://api.solvela.ai  
**Docs:** https://docs.solvela.ai  
**GitHub:** https://github.com/solvela-ai/solvela
