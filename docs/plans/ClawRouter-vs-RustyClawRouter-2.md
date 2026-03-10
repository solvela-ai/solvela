 BlockRun ClawRouter vs RustyClawRouter

  We win on fundamentals:
  - Server-side gateway (no client-side key custody)
  - Anchor escrow (trustless settlement — they have nothing)
  - Redis-backed replay protection & persistence (they lose everything on restart)
  - Rate limiting (they have none)
  - Modular crate architecture (their proxy.ts is 3500 lines)
  - Multi-tenancy capability (they're single-user only)

  They have features we should build:
  - Session tracking — conversation-level cost attribution + stuck-loop detection (3-strike
  escalation)
  - Time-windowed spend limits — hourly/daily/monthly caps (theirs is client-side, ours should be
  server-side/enforced)
  - Model capability flags — tool_calling, vision, agentic, reasoning for smart filtering
  - Request deduplication — coalesce identical in-flight requests to prevent duplicate provider
  calls
  - 41 models vs our 16 (easy to expand via models.toml)
  - Context compression (7-layer, 15-40% token reduction) — nice-to-have, not critical

  What NOT to copy:
  - LLM fallback classifier (adds 200-400ms latency in routing path)
  - Client-side wallet storage
  - In-memory-only persistence
  - Monolith architecture

  Full analysis saved to docs/plans/2026-03-08-blockrun-competitive-analysis.md.


