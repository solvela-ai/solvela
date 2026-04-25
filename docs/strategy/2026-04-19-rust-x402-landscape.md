# Rust x402 Crate Landscape — Competitive Deep Dive

> **Date:** 2026-04-19
> **Data source:** crates.io JSON API (search: `x402`, 98 total matches) + github.com/x402-rs/x402-rs README.
> **Why this doc exists:** You asked what else is on crates.io. Answer: a lot, and it changes your publish plan and exit positioning.

---

## 0 — TL;DR

1. **`x402` the crate name is taken.** Squatter from Feb 2025 holds `x402` v0.1.0, no repo, no homepage, 776 total downloads. You can't publish your internal `x402` crate under that name.
2. **`x402-rs` is the 800-pound gorilla of Rust x402.** 257 GitHub stars, 20k+ downloads across 8+ modular crates, actively shipping (last release 2026-04-14, 5 days ago).
3. **`x402-chain-solana` already exists.** Not from Coinbase, not from you — from the `x402-rs` org. Updated 2026-04-14 (5 days old). Solana-on-Rust-x402 is not a greenfield opportunity.
4. **Your Solvela crate names are all available** (`solvela`, `solvela-cli`, `solvela-router`, `solvela-protocol`, `solvela-sdk`, `solvela-gateway`, `solvela-x402`). You can publish; you just need to rename the internal `x402` crate.
5. **Your moat narrows but holds.** The x402 protocol layer is commoditized in Rust. What's NOT commoditized: a deployed, operating LLM gateway with mainnet escrow, smart routing, 5 providers, and two commercial customers. Sell the product, not the protocol.
6. **Exit valuation impact:** −10–15% on the headline. Buyers will see `x402-rs` + `x402-chain-solana` and ask "what makes you different from a weekend integration of those?" You must have a sharper answer than "Solana-native."

---

## 1 — The dominant player: `x402-rs` ecosystem

Maintained by `github.com/x402-rs/x402-rs` org (257 stars, 800+ commits). Position: "production-proven, compatible with the Coinbase reference SDK." Powers a hosted facilitator service called **FareSide**.

### What FareSide actually is (critical context, added 2026-04-19)

FareSide (`fareside.com`) is the **commercial arm** of x402-rs — same team, open-source library + hosted service (Supabase/Postgres pattern). But crucially:

- **FareSide is a hosted x402 facilitator, NOT an LLM gateway.** Different layer of the stack. A facilitator verifies payment payloads + settles on-chain. A gateway (Solvela) routes LLM requests and charges a platform fee on top.
- **FareSide is in closed beta** (confirmed by user on 2026-04-19). No public pricing, no public signup. They've briefly handled 77% of x402 traffic during ecosystem surges.
- **Multi-chain:** Base, Polygon, Avalanche, Sei, XDC, Solana, Aptos.
- **Funding / team:** undisclosed publicly.

**This is a layer-confusion rescue for Solvela's positioning.** Solvela is NOT competing with FareSide. Solvela *could use* FareSide (or Coinbase's facilitator, or self-hosted x402-rs) as its backend facilitator while Solvela handles LLM routing, escrow, provider fallback, and the 5% platform fee. That's a healthy stack relationship, not a competitive one.

### The facilitator market (separate from the gateway market)

Worth knowing because buyers will ask:

- **Coinbase Facilitator** — reference implementation. Starting **2026-01-01**, charges `$0.001/settlement` after 1,000 free/month. Free tier is generous enough that hobby projects never pay.
- **FareSide** — closed beta, no public pricing, 77% traffic handling during spikes.
- **PayAI, Dexter, x402rs** — other facilitators per the Sei Labs fee-transparency proposal.
- **Self-hosted `x402-rs`** — free, run your own facilitator binary.

Solvela's architecture already treats the facilitator as swappable (you have your own verification path). That's a feature — sell "bring-your-own-facilitator" as optionality, not a weakness.

### Published crates under this ecosystem (all versioned to 1.4.x, released 2026-04-14)

| Crate | v | Downloads (recent) | Role |
|---|---|---|---|
| `x402-axum` | 1.4.6 | 19.6k (13.2k recent) | **Axum middleware for enforcing x402 on routes** — direct competitor to your middleware |
| `x402-rs` | 0.12.5 | 20.1k (7.4k recent) | Runnable facilitator binary |
| `x402-types` | 1.4.6 | 10.6k (10.6k recent) | Core types / facilitator traits |
| `x402-chain-eip155` | 1.4.6 | 6.8k (6.8k recent) | EVM chain support |
| `x402-reqwest` | 1.4.6 | 4.2k (1.1k recent) | Client-side reqwest wrapper |
| `x402-chain-solana` | 1.4.6 | 2.8k (2.8k recent) | **Solana chain support** — released Feb 2026, shipping every ~2 weeks |
| `x402-facilitator-local` | 1.4.6 | 2.5k (2.5k recent) | Local facilitator implementation |

**Bottom line:** A Rust developer who needs "add x402 payments to my Axum service, settling on Solana with USDC" can do it **today**, with zero code written by them, using `x402-axum` + `x402-chain-solana`. This is the landscape you're publishing into.

---

## 2 — The other active players

### `r402` family — github.com/qntx/r402

Clean-room parallel implementation. Renamed to avoid x402-rs namespace.

| Crate | v | Downloads | Role |
|---|---|---|---|
| `r402` | 0.13.0 | 1.2k | Core types |
| `r402-http` | 0.13.0 | 678 | HTTP transport |
| `r402-evm` | 0.13.0 | 896 | EVM chain |
| `r402-svm` | 0.13.0 | 739 | **Solana VM chain** |

Created Feb 2026, already at v0.13.0 — aggressive cadence. Lower adoption than x402-rs but actively shipping. Effectively a second Rust stack doing the same thing x402-rs does.

### `tempo-x402` family — github.com/compusophy/tempo-x402

Full agentic stack on "Tempo blockchain" (not Solana, but the shape is strategically adjacent). All at v9.3.0, last updated 2026-04-12.

| Crate | Role |
|---|---|
| `tempo-x402` | Core protocol library (EIP-712, TIP-20) |
| `tempo-x402-gateway` | **API gateway with embedded facilitator** — direct shape match to Solvela |
| `tempo-x402-identity` | ERC-8004 on-chain agent identity |
| `tempo-x402-soul` | **Agentic loop powered by Gemini** — observe-think-record |
| `tempo-x402-node` | Self-deploying gateway + identity + orchestration |
| `tempo-x402-model` | Sequence model for autonomous planning |
| `tempo-x402-cartridge` | WASM sandbox for x402 app execution |

This is the most sophisticated x402-adjacent effort by architecture. Different chain (Tempo), but the pattern — gateway + identity + agentic loop + sandbox — is exactly the direction the category is heading. If you ever position against "our gateway is agentic-native," `tempo-x402-soul` is what you're compared to.

### `x402-kit` / AIMOverse family — github.com/AIMOverse/x402-kit

"V2 Supported" branded SDK. Suggests x402 V2 migration is happening in the ecosystem.

| Crate | v | Downloads | Role |
|---|---|---|---|
| `x402-core` | 2.5.0 | 2.2k | Modular SDK core |
| `x402-networks` | 2.5.0 | 1.7k | Chain types |
| `x402-signer` | 2.5.0 | 1.6k | Buyer-side signing SDK |
| `x402-paywall` | 2.5.0 | 430 | Paywall helper |
| `x402-kit` | 2.5.0 | 659 | SDK umbrella |

Lower downloads than x402-rs but complete stack, V2-ready.

### Miscellaneous

- `x402` (the bare name) — squatter from Feb 2025, 776 downloads, no repo. Held but effectively dead.
- `x402-facilitator` (second-state) — December 2025, 40 downloads. Basically inactive.
- `nginx-x402` — Nginx module. Different niche.
- `cargo-x402` — scaffold/template tool. Different niche.
- `rust-x402` — older (v0.3.0, Dec 2025), 1.4k downloads. Appears superseded.
- `pipegate` — "payment authentication middleware with stablecoins," v0.6.0, not x402-specific but adjacent.

---

## 3 — Name availability (confirmed via API)

| Name | Status |
|---|---|
| `solvela` | ✅ AVAILABLE |
| `solvela-cli` | ✅ AVAILABLE |
| `solvela-sdk` | ✅ AVAILABLE |
| `solvela-router` | ✅ AVAILABLE |
| `solvela-protocol` | ✅ AVAILABLE |
| `solvela-gateway` | ✅ AVAILABLE |
| `solvela-x402` | ✅ AVAILABLE |
| `solvela-core` | ✅ AVAILABLE |
| `x402-solana` | ✅ AVAILABLE |
| `x402-rs-solana` | ✅ AVAILABLE |
| `x402` | ❌ TAKEN (squatter) |
| `x402-chain-solana` | ❌ TAKEN (x402-rs owns it) |

---

## 4 — What this means for the publish plan

### Must-change before publishing Rust crates

Your internal `x402` crate cannot publish under that name. Options:

**Option A — Rename to `solvela-x402` (recommended).** Clean, branded, no namespace conflict. Your gateway imports it as `solvela_x402` (underscore in code). Keeps your implementation. Signals it's your flavor of x402, not the canonical.

**Option B — Adopt `x402-rs` crates, delete your impl.** Replace your `crates/x402/` with `x402-axum` + `x402-chain-solana` + `x402-types`. Dramatically cuts your code surface (and your future maintenance burden). Loses your wire-format control. Gains compatibility with the dominant Rust ecosystem instantly. **Major decision — don't make this at 11pm before a launch.**

**Option C — Publish `solvela-x402` but depend on `x402-types` for protocol types.** Middle path: you keep your verification logic and gateway integration, but interop with the x402-rs world via shared types. Best of both if done well, worst if types diverge.

My recommendation: **Option A for this release.** Ship what you have, rename only. Revisit Option B/C in a post-launch retrospective once you know real users.

### Skip publishing what nobody will consume

You probably don't need to publish `solvela-router` or `solvela-protocol` to crates.io. Their consumers are internal to the Solvela gateway. Publishing them creates a support burden (API stability obligations) with zero user demand.

**Publish only `solvela-cli`.** Vendor or depend-path-only on internal crates within the gateway repo. This collapses the Rust publish chain from 4 crates to 1. Much less to go wrong.

### Updated Rust publish chain (post-finding)

1. **Decision:** Option A → rename `x402` to `solvela-x402` in workspace Cargo.toml + every `use` site. Run `cargo check` to catch refs.
2. **Publish only `solvela-cli`.** If Cargo complains about unpublished path deps, EITHER also publish `solvela-x402` (fine) OR bundle its sources into `solvela-cli` (more work, less ecosystem value).
3. Skip publishing `solvela-protocol`, `solvela-router` until a real external consumer asks.

---

## 5 — Positioning: what to say now

You cannot credibly lead with:
- ❌ "Rust x402 for Solana" — `x402-chain-solana` already owns this lane
- ❌ "First Rust implementation of x402" — `x402-rs` had it a year ago
- ❌ "The Solana-native x402 library" — same reason
- ❌ "We built x402 verification in Rust" — table stakes now

You CAN credibly lead with:
- ✅ "A deployed, operating x402 LLM gateway — not a library, a running service"
- ✅ "Mainnet Anchor escrow — the only x402 gateway with trustless refunds enforced on-chain"
- ✅ "Smart routing across 5 providers with per-request cost breakdown"
- ✅ "Two commercial products running on it in production" (Telsi + RustyClaw)
- ✅ "Agent-native: MCP plugin, A2A protocol, AP2 compatibility"

The shift is subtle but critical: **you are not a library vendor, you are an infrastructure operator**. Libraries are commodity; the operating product, plus its customers and escrow, is not.

### Rewritten positioning line

Old:
> "Open-source x402 gateway for Solana."

New:
> "The Solana LLM gateway — production x402 service with mainnet escrow, running two commercial customers, open SDK suite. Built on x402, differentiated by escrow and operations."

That quietly cedes the library layer to x402-rs (wise — don't fight a war you don't need) and stakes a claim higher in the stack where you actually have advantage.

---

## 6 — Impact on exit valuation

From the exit playbook (2026-04-19):

> Plan for $1–2M. Ready for $500k. Don't anticipate $5M.

Revised in light of today's findings:

> **Plan for $900k–$1.8M. Ready for $500k. Don't anticipate $4M.**

(Narrower downward revision than the pre-FareSide-context read, because the gateway-vs-facilitator layer distinction means you're not fighting x402-rs head-on.)

Reason: a knowledgeable acquirer's first question during diligence will be "how do you differ from `x402-rs` + `x402-chain-solana` + a weekend of Axum work?" Your answer has to distinguish the **library layer** (commoditized — x402-rs owns it) from the **facilitator layer** (FareSide commercial + Coinbase public + self-host) from the **gateway + escrow + routing + product layer** (where Solvela actually lives and nobody else has shipped at scale on Solana). That positioning is defensible but must be articulated clearly — it doesn't read itself off the code.

Movers unchanged:
- +30–50% for published SDKs (still true)
- +20–40% for visible MRR from Telsi/RustyClaw
- +50–100% for a second bidder (most valuable)
- +20% for audited escrow
- **NEW: +15%** if you can produce a "Solvela vs x402-rs" comparison doc with actual benchmarks (latency, p99, escrow coverage, time-to-production for a customer). This neutralizes the #1 diligence question.

---

## 7 — What to tell yourself

You did not waste time. Your gateway's internal `x402` verification, escrow program, router, provider adapters, middleware stack, and test suite **are still your work**. What changed is that the *protocol layer* (verify a signature, check a replay, talk to a facilitator) is no longer a moat.

The category has matured. The competition is real. You still have a product to sell — just don't sell it as "we built x402 in Rust for Solana." Sell it as "we built and operate the LLM gateway on top of x402 for Solana, with the escrow nobody else bothered with."

That story is true, defensible, and worth money.

---

## 8 — Concrete next actions

1. **Rename internal `x402` crate → `solvela-x402`.** Cargo workspace edit + grep-replace in imports. One afternoon.
2. **Decide publish scope.** Recommend: `solvela-cli` only. Re-evaluate `solvela-x402` public publish at T+30.
3. **Write `docs/comparison-x402-rs.md`** — honest, technical, side-by-side of `x402-rs/x402-axum + x402-chain-solana` vs Solvela. 800 words. Strengthens your exit pitch and is useful launch content.
4. **Update SDK publish runbook** (`docs/runbooks/2026-04-19-sdk-publish.md`) §2 with the Rust rename. Already flagged as a gotcha; now it's a confirmed blocker.
5. **Update `2026-04-17-competitive-analysis.md`** — it focuses on gateway-level competitors (BlockRun, Skyfire) and missed the Rust library layer entirely. Add a §3.5 for the Rust crate ecosystem with x402-rs as the primary player.

---

## 9 — Sources

- crates.io JSON API: `/api/v1/crates?q=x402` (98 total matches, 2026-04-19)
- `github.com/x402-rs/x402-rs` (257 stars, 800+ commits)
- `x402.rs` — project homepage (mentions FareSide, Coinbase SDK compatibility)
- `github.com/qntx/r402` (r402 family)
- `github.com/compusophy/tempo-x402` (tempo-x402 family)
- `github.com/AIMOverse/x402-kit` (x402-kit family)
