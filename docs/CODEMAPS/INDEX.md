<!-- Generated: 2026-05-27 | Files scanned: 155 | Token estimate: ~350 -->

# Solvela Codemap Index

**Last Updated:** 2026-05-27  
**Total Scanned Files:** 155  
**Total Codemaps:** 5 files (~3500 tokens)

---

## Quick Navigation

| Codemap | Focus | Best For |
|---------|-------|----------|
| **[architecture.md](architecture.md)** | System overview, crate boundaries, request flows | Understanding the big picture, A2A protocol, Solana integration |
| **[backend.md](backend.md)** | Route tree, middleware chain, handlers | Building API endpoints, adding routes, debugging requests |
| **[frontend.md](frontend.md)** | Page tree, components, state management | Dashboard development, UI/UX, forms |
| **[data.md](data.md)** | Database schema, tables, migrations | Adding database features, understanding data models, querying |
| **[dependencies.md](dependencies.md)** | Provider integrations, external services | Integrating new LLM provider, Solana RPC, configuring APIs |

---

## Codebase Structure at a Glance

```
solvela/
├── crates/
│   ├── gateway/          (HTTP server, 20+ route handlers, middleware)
│   ├── x402/             (Solana payment protocol, escrow, nonce pool)
│   ├── router/           (Smart request scorer, routing profiles)
│   ├── protocol/         (Wire types, payment types, constants)
│   └── cli/              (solvela CLI binary)
│
├── programs/
│   └── escrow/           (Anchor program, USDC-SPL escrow)
│
├── dashboard/            (Next.js 16 frontend)
├── sdks/                 (typescript, python, go, rust, ai-sdk-provider, openclaw-provider, signer-core, mcp, cli-npm)
├── config/               (models.toml, services.toml, default.toml)
├── migrations/           (PostgreSQL schema, 9 files)
└── docs/CODEMAPS/        (This documentation)
```

---

## Key Entry Points

### Binary Entrypoints
- **Gateway** — `crates/gateway/src/main.rs` (HTTP server on port 8402)
- **CLI** — `crates/cli/src/main.rs` (Command-line wallet interface)

### Library Entrypoints
- **x402 Protocol** — `crates/x402/src/lib.rs` (Solana payment verification, escrow)
- **Router** — `crates/router/src/lib.rs` (Smart request scoring)
- **Protocol Types** — `crates/protocol/src/lib.rs` (OpenAI-compatible types)
- **Gateway** — `crates/gateway/src/lib.rs` (AppState, build_router)

### Frontend
- **Dashboard** — `dashboard/src/app/` (Next.js 16 pages)

---

## Request Flow Summary

### Chat Completions (POST /v1/chat/completions)
1. Parse & validate → Resolve model (alias/profile/direct)
2. Cost estimation (5% fee)
3. Payment check → 402 if missing PAYMENT-SIGNATURE
4. Verify signature via Facilitator (Solana ed25519)
5. Proxy to LLM provider
6. Cache response (Redis)
7. Log spend (PostgreSQL, fire-and-forget)
8. Claim escrow (if configured)
9. Return JSON or SSE stream

### A2A Protocol (POST /a2a)
1. Agent discovers gateway via GET /.well-known/agent-card.json (alias: /.well-known/agent.json)
2. Send message/send JSON-RPC → Gateway returns Task (input-required) + cost
3. Agent signs USDC-SPL transaction → sends with payment payload
4. Gateway verifies payment → proxies to LLM → returns Task (completed)

---

## Architecture Highlights

### Multi-Tenant Organization Model
```
Organization
├── Teams
│   └── Wallets (agents)
├── Members (role-based: owner/admin/member)
├── API Keys (for programmatic access)
└── Budgets (daily/monthly spend limits)
```

### Payment Model
- **5% platform fee** on all requests (included in cost breakdown)
- **USDC-SPL on Solana** — transparent, no hidden fees
- **Escrow-based** — agents deposit, gateway claims on request completion
- **Exact** — pay-per-request via signed transaction (future: immediate settlement)

### Smart Routing
- **15-dimension scorer** (code density, reasoning markers, technical terms, etc.)
- Routes requests to optimal model based on:
  - Complexity tier (Simple/Medium/Complex/Reasoning)
  - Routing profile (eco/auto/premium/free)
  - Provider health (latency, error rates)

### Graceful Degradation
- **No PostgreSQL?** → Fire-and-forget logging, no org/budget features
- **No Redis?** → In-memory LRU cache fallback (10k entries, 120s TTL)
- **No Escrow?** → Chat endpoint works, claims must be manual

---

## Common Tasks

### Add a New Chat Endpoint
1. Create route handler in `crates/gateway/src/routes/`
2. Add to router in `lib.rs` (`build_router()`)
3. Implement middleware (rate limit, auth, payment)
4. Write integration test in `tests/`
See [backend.md](backend.md)

### Add a New LLM Provider
1. Create adapter in `crates/gateway/src/providers/new_provider.rs`
2. Implement `async fn chat(...)` (translate to OpenAI format)
3. Add model entries to `config/models.toml`
4. Add API key env var to `.env.example`
5. Register in `ProviderRegistry`
See [dependencies.md](dependencies.md)

### Add Database Schema
1. Create migration SQL file in `migrations/`
2. Use idempotent `CREATE TABLE IF NOT EXISTS`
3. (sqlx is used in runtime-checked mode — no offline `sqlx prepare` step is required)
4. Add queries to `crates/gateway/src/` (sqlx checked)
See [data.md](data.md)

### Add Dashboard Page
1. Create page in `dashboard/src/app/[section]/page.tsx`
2. Use `Recharts` for charts, `Tailwind` for layout
3. Call API via `lib/api.ts`
4. Add to navigation in `dashboard/layout.tsx`
See [frontend.md](frontend.md)

### Debug a Payment Issue
1. Check PAYMENT-SIGNATURE header decoding (base64 or JSON)
2. Verify Solana signature via Facilitator (`x402/facilitator.rs`)
3. Check blockhash age (<2 mins from slot time)
4. Verify transaction signature matches message
5. Check escrow PDA derivation: `[b"escrow", agent, service_id]`
See [architecture.md](architecture.md) + [dependencies.md](dependencies.md)

---

## Test Coverage

| Crate | Unit Tests | Integration Tests | E2E |
|-------|-----------|------------------|-----|
| gateway | Yes (routes, middleware, utils) | Yes (tower::ServiceExt) | No (run locally) |
| x402 | Yes (sig verification, escrow) | Yes (Solana RPC mocks) | Optional (live mainnet) |
| router | Yes (scorer, profiles) | Yes (routing decisions) | N/A |
| protocol | Yes (serialization, types) | Yes (wire format) | N/A |
| cli | Yes (commands, parsing) | Yes (integration) | No |
| dashboard | Yes (Vitest) | N/A | No |

Run tests:
```bash
cargo test                              # All Rust tests
npm --prefix dashboard test             # Frontend unit tests (vitest)
```

The dashboard has no `e2e` npm script — Playwright is pulled in only as a transitive `@vitest/browser-playwright` dep. End-to-end testing is not yet wired up.

---

## Configuration & Environment

### TOML Files
- `config/default.toml` — Server defaults, Solana RPC
- `config/models.toml` — Model registry (44+ models)
- `config/services.toml` — Service marketplace

### Environment Variables
- Provider API keys: `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, etc.
- Solana: `SOLVELA_SOLANA__RPC_URL`, `SOLVELA_SOLANA__RECIPIENT_WALLET` (Fly.io double-underscore is canonical; legacy single-underscore form is also accepted as a fallback)
- Database (optional): `DATABASE_URL`, `REDIS_URL`
- Admin: `SOLVELA_ADMIN_TOKEN`, `SOLVELA_DEV_BYPASS_PAYMENT`

See [dependencies.md](dependencies.md) for full env var list.

---

## Performance Characteristics

### Request Latencies
- **Health check** — <5ms (in-memory)
- **Chat cost estimation** — ~10ms (model lookup, token estimation)
- **Payment verification** — ~50ms (Solana sig verification)
- **LLM proxy** — 500ms–30s (depends on model, streaming)
- **Response cache hit** — ~5ms (Redis)

### Throughput
- **Concurrent requests** — Limited by `ConcurrencyLimitLayer` (default: 100)
- **Rate limiting** — Per-wallet sliding window (default: 60 req/min)
- **Database** — Connection pool (default: 10 connections)

### Resource Usage
- **In-memory replay protection** — 10k entries max (120s TTL, LRU eviction)
- **Response cache** — 10-minute TTL per response (default; see `cache::ResponseCacheConfig::default_ttl_secs = 600`)
- **Binary size** — ~15MB (release build, 2-stage Docker)

---

## Security Model

### Authentication
- **Wallet-based:** Solana ed25519 signatures (x402 protocol)
- **API Key-based:** SHA256 hashed, stored in PostgreSQL
- **Session tokens:** HMAC-signed (for dashboard)

### Authorization
- **Route-level:** `RequireOrg`, `RequireOrgAdmin` extractors
- **Role-based:** owner > admin > member
- **Org-scoped:** Data access limited to org members

### Secrets Management
- **No hardcoded secrets** — All from env vars
- **Secret redaction** — Custom Debug impls hide API keys in logs
- **Key storage** — Hashed in PostgreSQL (never plaintext)

---

## License & Attribution

| Component | License | Scope |
|-----------|---------|-------|
| Gateway (server) | BUSL-1.1 | `crates/gateway/` (commercial use restricted) |
| x402, router, protocol, cli | Apache-2.0 | `crates/{x402,router,protocol,cli}/` (reusable) |
| Escrow program | Apache-2.0 | `programs/escrow/` (on-chain, reusable) |
| SDKs | Apache-2.0 | `sdks/` (client libraries) |
| Dashboard | Apache-2.0 | `dashboard/` (front-end) |
| Change date | 2030-05-02 | BUSL-1.1 (gateway) becomes MIT after this date |

---

## Release History

**Current Version:** 0.2.0 (Cargo.toml)

Recent launches (see [STATUS.md](../../STATUS.md) + [CHANGELOG.md](../../CHANGELOG.md)):
- Multi-tenant orgs, teams, API keys
- A2A protocol (agent-to-agent message passing)
- Smart router (15-dimension scorer)
- Escrow integration (durable claims)
- Dashboard (Next.js 16 frontend)

---

## Related Documentation

- **[README.md](../../README.md)** — Project overview, quick start
- **[STATUS.md](../../STATUS.md)** — Live shipping status
- **[CHANGELOG.md](../../CHANGELOG.md)** — Chronological history
- **[CLAUDE.md](../../CLAUDE.md)** — Development guidelines, architecture rules
- **[SECURITY.md](../../SECURITY.md)** — Security policy, vulnerability disclosure
- **[CONTRIBUTING.md](../../CONTRIBUTING.md)** — Contribution guidelines

---

## Support & Feedback

- **Issues** — GitHub issues (bugs, feature requests)
- **Discussions** — GitHub discussions (questions, ideas)
- **Security** — See [SECURITY.md](../../SECURITY.md) for disclosure process

---

**Generated by documentation specialist — 2026-05-27**
