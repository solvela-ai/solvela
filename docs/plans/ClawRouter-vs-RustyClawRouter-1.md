 ClawRouter vs Solvela — Competitive Analysis

  Where ClawRouter Falls Short (and you already do better)

  1. Architecture — Monolith vs Modular Workspace

  ClawRouter is a single src/ directory with ~41 TypeScript files in a flat structure. proxy.ts
  alone is ~2,000 lines handling routing, payment, streaming, caching, and fallback all in one file.

  Solvela has a clean workspace with separated concerns:
  - x402 (protocol-only, no HTTP coupling)
  - router (pure scoring, no I/O)
  - gateway (HTTP layer)
  - common (shared types)

  This gives you testability ClawRouter can't match — your x402 crate can be used independently of
  Axum.

  2. Security — Significant Gaps in ClawRouter

  ┌───────────────┬──────────────────────────────┬──────────────────────────────────────────────┐
  │     Area      │          ClawRouter          │               Solvela                │
  ├───────────────┼──────────────────────────────┼──────────────────────────────────────────────┤
  │ Key storage   │ Wallet private key stored on │ Keys from env vars only, never persisted     │
  │               │  disk via BIP39 mnemonic     │                                              │
  ├───────────────┼──────────────────────────────┼──────────────────────────────────────────────┤
  │ Secret        │ No custom Debug impls        │ Custom Debug on FeePayerWallet, AppConfig —  │
  │ redaction     │                              │ all secrets [REDACTED]                       │
  ├───────────────┼──────────────────────────────┼──────────────────────────────────────────────┤
  │ Key zeroing   │ No memory zeroing            │ Drop impl zeros keypair bytes                │
  ├───────────────┼──────────────────────────────┼──────────────────────────────────────────────┤
  │ Replay        │ 30s dedup cache (dedup.ts)   │ Redis-backed check_and_record_tx —           │
  │ protection    │                              │ persistent across restarts                   │
  ├───────────────┼──────────────────────────────┼──────────────────────────────────────────────┤
  │ Security      │ None                         │ X-Content-Type-Options, X-Frame-Options,     │
  │ headers       │                              │ Referrer-Policy on every response            │
  ├───────────────┼──────────────────────────────┼──────────────────────────────────────────────┤
  │ Prompt guard  │ None                         │ Injection, jailbreak, PII detection          │
  │               │                              │ middleware                                   │
  ├───────────────┼──────────────────────────────┼──────────────────────────────────────────────┤
  │ Body size     │ None visible                 │ 10 MB RequestBodyLimitLayer                  │
  │ limit         │                              │                                              │
  ├───────────────┼──────────────────────────────┼──────────────────────────────────────────────┤
  │ CORS          │ Not visible                  │ Explicit allowlist with env var override     │
  └───────────────┴──────────────────────────────┴──────────────────────────────────────────────┘

  3. Payment Verification — ClawRouter Delegates Everything

  ClawRouter acts as a proxy to BlockRun's API — it doesn't verify payments itself. The flow is:
  Client → ClawRouter (local proxy) → BlockRun API (does actual x402 signing + verification)

  Solvela does direct on-chain verification via Facilitator + settlement, with:
  - Dual-scheme support (exact + escrow)
  - On-chain escrow program with PDA-based deposit/claim/refund
  - Fee payer pool with round-robin rotation + cooldown failover
  - Durable nonce pool
  - Balance monitoring

  ClawRouter has no escrow system at all.

  4. Scoring — 14 vs 15 Dimensions, Missing Rigor

  Both use rule-based scoring, but:

  ┌────────────────────┬───────────────────────┬───────────────────────────────────────────────┐
  │       Aspect       │      ClawRouter       │                Solvela                │
  ├────────────────────┼───────────────────────┼───────────────────────────────────────────────┤
  │ Dimensions         │ 14                    │ 15 (adds tool usage)                          │
  ├────────────────────┼───────────────────────┼───────────────────────────────────────────────┤
  │ Confidence         │ Sigmoid fallback      │ Direct tier mapping                           │
  │ calibration        │                       │                                               │
  ├────────────────────┼───────────────────────┼───────────────────────────────────────────────┤
  │ LLM fallback       │ Yes                   │ No (pure rules, sub-µs)                       │
  │ classifier         │ (llm-classifier.ts)   │                                               │
  ├────────────────────┼───────────────────────┼───────────────────────────────────────────────┤
  │ Weight validation  │ No test               │ test_weights_sum_to_one() — guarantees        │
  │                    │                       │ weights = 1.0                                 │
  ├────────────────────┼───────────────────────┼───────────────────────────────────────────────┤
  │ System prompt      │ Yes (issue #50 fix)   │ Filters to user messages only in              │
  │ filtering          │                       │ concatenate_user_content                      │
  └────────────────────┴───────────────────────┴───────────────────────────────────────────────┘

  ClawRouter's LLM classifier fallback is interesting but adds latency + cost + external dependency
  for edge cases.

  5. Error Handling — Type-Safe vs String-Based

  ClawRouter: 3 error classes (InsufficientFundsError, EmptyWalletError, RpcError) with string
  discriminators.

  Solvela: thiserror enums throughout (GatewayError, FeePayerError, ProviderError) with
  exhaustive match — the compiler enforces every error is handled.

  6. Testing — Orders of Magnitude More Rigorous

  ClawRouter: scattered .test.ts files, Vitest, test count unclear.

  Solvela: 139 tests across all crates, including:
  - Financial calculation tests with 100% coverage markers
  - Serde roundtrip tests for all payment types
  - Integration tests via tower::ServiceExt::oneshot (no live server)
  - TDD-written fee payer pool with 9 test scenarios

  ---
  Where ClawRouter Has Features You're Missing

  1. Session Persistence / Model Sticking
  ClawRouter tracks sessions so the same agent gets the same model across a conversation.
  Three-strike escalation detects repetitive failures and auto-upgrades the tier. You don't have
  this — each request is independently routed.

  2. Auto-Compression
  ClawRouter compresses large requests (>180KB) before sending to the provider, saving 15-40% on
  token costs. You don't compress.

  3. SSE Heartbeat
  ClawRouter sends heartbeat events during streaming to prevent client timeouts (OpenClaw's 10-15s
  timeout). Your SSE implementation doesn't have this.

  4. Degraded Response Detection
  ClawRouter detects when providers return garbage (repetitive loops, overloaded placeholders) and
  retries with a different model. Your fallback only triggers on hard errors.

  5. Free Tier Fallback
  When the wallet is empty, ClawRouter falls back to nvidia/gpt-oss-120b (free model). Your gateway
  just returns 402.

  6. Message Normalization
  ClawRouter handles provider-specific quirks:
  - Converts developer role → system
  - Sanitizes tool IDs for Anthropic's character restrictions
  - Handles Gemini's "first message must be user" requirement
  - Strips thinking tokens from responses
  - Truncates to 200 messages preserving system context

  Your provider adapters handle format translation but these specific edge cases may not be covered.

  7. Debug/Diagnostics Command
  /debug returns routing diagnostics — tier breakdown, scoring dimensions, session info, cost
  estimates. Your CLI has doctor but not in-band diagnostics.

  ---
  How You Could Do It Better

  ┌───────────────────────┬─────────────────────────────────────────────┬───────────────────────┐
  │    Feature to Add     │           Why It's Better in Rust           │       Priority        │
  ├───────────────────────┼─────────────────────────────────────────────┼───────────────────────┤
  │ Session sticking      │ DashMap<SessionId, (ModelId, Tier,          │ High — agents need    │
  │                       │ Instant)> — lock-free, sub-µs lookup        │ consistency           │
  ├───────────────────────┼─────────────────────────────────────────────┼───────────────────────┤
  │ Degraded response     │ Pattern matching on response content,       │ Medium — prevents     │
  │ detection             │ zero-cost at compile time                   │ wasted payments       │
  ├───────────────────────┼─────────────────────────────────────────────┼───────────────────────┤
  │ SSE heartbeat         │ tokio::select! with interval timer —        │ Medium — prevents     │
  │                       │ trivial in async Rust                       │ client timeouts       │
  ├───────────────────────┼─────────────────────────────────────────────┼───────────────────────┤
  │ Auto-compression      │ Could use tiktoken-rs for accurate token    │ Low — most agent      │
  │                       │ counting before deciding                    │ requests are small    │
  ├───────────────────────┼─────────────────────────────────────────────┼───────────────────────┤
  │ Free tier fallback    │ Simple config: free_model = "...", skip     │ Low — nice UX         │
  │                       │ payment when wallet signals empty           │                       │
  ├───────────────────────┼─────────────────────────────────────────────┼───────────────────────┤
  │ In-band /debug        │ Return scoring dimensions in x-debug-*      │ Low — useful for      │
  │                       │ response headers                            │ integration           │
  └───────────────────────┴─────────────────────────────────────────────┴───────────────────────┘

  Your Unique Advantages ClawRouter Can't Match

  1. Direct on-chain verification — ClawRouter is a proxy to BlockRun's centralized API. You verify
  directly. This is trustless vs trust-dependent.
  2. Escrow program — Anchor-based on-chain escrow with deposit/claim/refund. ClawRouter has nothing
   like this.
  3. Fee payer pool — 8 rotating wallets with automatic failover. ClawRouter uses a single wallet.
  4. Nonce pool — Durable nonces for reliable transaction submission. ClawRouter doesn't handle
  nonces.
  5. Performance — Rust + Axum vs Node.js. Your scorer runs in <1µs. ClawRouter's runs in <1ms
  (1000x slower).
  6. Memory safety — Keypair zeroing on drop, no GC-delayed secret exposure.
  7. Type-safe protocol — PayloadData::Escrow | Direct with compile-time exhaustive matching vs
  runtime string checking.
  8. Multi-SDK — Python, TypeScript, Go, MCP server. ClawRouter only has the npm package.

  Bottom Line

  ClawRouter is a Node.js proxy that delegates to BlockRun's centralized API. Solvela is a
  standalone, self-sovereign payment gateway that does everything on-chain. You're building the
  infrastructure layer they depend on. The features you're missing (session sticking, heartbeats,
  degraded detection) are UX polish — straightforward to add. Their fundamental architecture
  limitation (centralized payment verification) can't be fixed without rewriting.