<!-- Generated: 2026-05-27 | Files scanned: 26 | Token estimate: ~750 -->

# External Dependencies & Integrations Codemap

**Last Updated:** 2026-05-27  
**Primary Integrations:** 5 LLM providers, Solana blockchain, PostgreSQL, Redis

## LLM Providers (API Adapters)

All providers implement OpenAI-compatible chat/completion format. Gateway translates between OpenAI format (internal) and provider-specific APIs.

### OpenAI (providers/openai.rs)
**Endpoint:** https://api.openai.com/v1/chat/completions  
**Auth:** `Authorization: Bearer {OPENAI_API_KEY}`  
**Models:** GPT-5.2, GPT-4o, GPT-4o Mini, o3, GPT-OSS 120B (from config/models.toml)  
**Features:** Streaming, vision, tools, function calling  
**Cost:** Tracked per token; 5% platform fee added by gateway

**Adapter:** Translates OpenAI request → OpenAI request (identity), returns response stream.

```rust
// providers/openai.rs
pub async fn chat(
    client: &reqwest::Client,
    api_key: &str,
    req: &ChatRequest,
) -> Result<ChatResponse> {
    let response = client
        .post("https://api.openai.com/v1/chat/completions")
        .bearer_auth(api_key)
        .json(req)
        .send()
        .await?;
    response.json().await
}
```

---

### Anthropic (providers/anthropic.rs)
**Endpoint:** https://api.anthropic.com/v1/messages  
**Auth:** `x-api-key: {ANTHROPIC_API_KEY}`  
**Models:** Claude Opus 4.6, Claude Sonnet 4.6, Claude Haiku 4.5  
**Features:** Streaming, vision, tools, extended thinking (native support)  
**Cost:** Per-token pricing (input/output may differ)

**Adapter:** Translates OpenAI → Anthropic native format (messages API v1).
- OpenAI `messages[]` → Anthropic `messages[]` (same structure)
- Anthropic `stop_reason` → OpenAI `finish_reason`
- Streaming deltas → OpenAI-compatible chunks

```rust
// providers/anthropic.rs
pub async fn chat(
    client: &reqwest::Client,
    api_key: &str,
    req: &ChatRequest,
) -> Result<ChatResponse> {
    let anthropic_req = convert_to_anthropic(req);
    let response = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", api_key)
        .json(&anthropic_req)
        .send()
        .await?;
    let anthropic_resp = response.json::<AnthropicResponse>()?;
    Ok(convert_from_anthropic(&anthropic_resp))
}
```

---

### Google Gemini (providers/google.rs)
**Endpoint:** https://generativelanguage.googleapis.com/v1/models/{model}/generateContent  
**Auth:** Query param `key={GOOGLE_API_KEY}`  
**Models:** Gemini 3.1 Pro, Gemini 2.5 Flash, Gemini 2.5 Flash Lite  
**Features:** Streaming, vision, tools, reasoning (Pro)  
**Cost:** Per-token pricing; supports million-token batches  
**Context Window:** Up to 1M tokens (Gemini models)

**Adapter:** Translates OpenAI → Google Generative AI format.
- Converts `messages[]` to Google's `content[]` + `parts[]` structure
- Maps role: `user` → `user`, `assistant` → `model`, `system` → inline instructions
- Handles vision (image URLs → content parts)

---

### DeepSeek (providers/deepseek.rs)
**Endpoint:** https://api.deepseek.com/v1/chat/completions  
**Auth:** `Authorization: Bearer {DEEPSEEK_API_KEY}`  
**Models:** DeepSeek V3.2 Chat, DeepSeek V3.2 Reasoner  
**Features:** Streaming, reasoning (native `think_tokens`)  
**Cost:** Lower input/output cost vs. OpenAI; reasoning cheaper than o1

**Adapter:** Near-identical to OpenAI (both use OpenAI-compatible format).

---

### xAI Grok (providers/xai.rs)
**Endpoint:** https://api.x.ai/v1/chat/completions  
**Auth:** `Authorization: Bearer {XAI_API_KEY}`  
**Models:** Grok-4 Fast Reasoning  
**Features:** Streaming, reasoning, real-time knowledge  
**Cost:** Experimental pricing

**Adapter:** OpenAI-compatible format (same as DeepSeek).

---

## Provider Health Monitoring (providers/health.rs)

Background task that probes each provider every 60s:

```rust
pub struct ProviderHealthTracker {
    openai: ProviderHealth,
    anthropic: ProviderHealth,
    google: ProviderHealth,
    deepseek: ProviderHealth,
    xai: ProviderHealth,
}

pub struct ProviderHealth {
    last_check_at: Instant,
    is_healthy: bool,
    latency_ms: u64,
    error_count: u64,
}
```

Provider health is reported indirectly through `GET /health` and provider-specific metrics on `/metrics`. There is no dedicated `/v1/health` endpoint; the public liveness endpoint is `/health`.

Used by:
- **Router scorer** — Deprioritize slow providers
- **Fallback logic** — Skip unhealthy providers
- **Dashboard metrics** — Display provider status

---

## Solana Integration (x402 Crate)

### Solana RPC (solana_rpc.rs)
**Endpoint:** From `SOLVELA_SOLANA__RPC_URL` (or the legacy single-underscore `SOLVELA_SOLANA_RPC_URL`). `config/default.toml` ships with `https://api.devnet.solana.com` — production deployments must override to a mainnet-beta endpoint.  
**Client:** `reqwest::Client`

**Calls made:**
- `getLatestBlockhash()` — For transaction signing (blockhash lifetime ~2 mins)
- `getSlot()` — Current slot (for escrow/cache endpoints)
- `getSignatureStatuses([tx_sig])` — Check tx confirmation status

```rust
// solana_rpc.rs
pub async fn get_latest_blockhash(
    client: &reqwest::Client,
    rpc_url: &str,
) -> Result<Blockhash> {
    let response = client.post(rpc_url)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getLatestBlockhash",
            "params": [{ "commitment": "finalized" }]
        }))
        .send()
        .await?;
    // Parse JSON-RPC response
}
```

---

### USDC-SPL Token (spl_transfer.rs)
**Mint Address:** `EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v` (mainnet; hard-coded in `crates/protocol/src/constants.rs::USDC_MINT`)  
**Decimals:** 6 (1 USDC = 1,000,000 atomic units)

**Operations:**
- **Transfer** — SPL token transfer instruction (agent wallet → Solvela recipient)
- **Verification** — Parse transaction, verify recipient + amount

```rust
// spl_transfer.rs
pub fn verify_spl_transfer(
    tx: &Transaction,
    expected_mint: &Pubkey,
    expected_recipient: &Pubkey,
    expected_amount: u64,
) -> Result<()> {
    // Parse tx instructions
    // Verify SPL token program instruction
    // Check recipient (token account) and amount
}
```

---

### Escrow Program (programs/escrow/ - Anchor)
**Program ID:** From `SOLVELA_SOLANA__ESCROW_PROGRAM_ID` (legacy `SOLVELA_SOLANA_ESCROW_PROGRAM_ID` also accepted). Mainnet deployment: `9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU`.  
**Chain:** Mainnet-Beta (Solana)  
**Language:** Anchor (Solana Rust framework)

**Instructions:**
1. **Deposit** — Agent deposits USDC to escrow (PDA-based)
2. **Claim** — Service claims USDC from escrow (Solvela trigger)
3. **Refund** — Agent refunds unclaimed USDC

**PDA Seed:** `[b"escrow", agent.key().as_ref(), &service_id]`  
**Owner:** Escrow program

**Interaction:**
- Gateway calls `escrow_claimer.submit_claim()` → constructs instruction
- Client signs and submits transaction
- Escrow program executes (no signature needed from gateway)

```rust
// x402/escrow/mod.rs
pub struct EscrowClaimer {
    program_id: Pubkey,
    fee_payer: Pubkey,
    recipient: Pubkey,
    usdc_mint: Pubkey,
}

pub async fn submit_claim(&self, agent: &Pubkey, service_id: &[u8], amount: u64) -> Result<Tx> {
    // Construct: SPL transfer from PDA to recipient
    // PDA derived from [b"escrow", agent, service_id]
}
```

---

### Solana Signature Verification (solana.rs)
**Crypto:** ed25519-dalek (native Solana signing curve)

**Flow:**
1. Client signs message or transaction with private key
2. Gateway receives signature + public key
3. Gateway calls `Facilitator::verify()` → ed25519 verification

```rust
// x402/solana.rs
pub fn verify_signature(
    public_key: &Pubkey,
    message: &[u8],
    signature: &Signature,
) -> Result<()> {
    let public_key = VerifyingKey::from_bytes(public_key.as_ref())?;
    public_key.verify_strict(message, signature)
}
```

---

### Fee Payer Pool (fee_payer.rs)
**Purpose:** Rotate hot wallets for Solana fee payment (to avoid nonce conflicts).

**State:**
- Load private keys from env (`SOLVELA_SOLANA__FEE_PAYER_KEY` plus `_2`…`_8` for the rotation pool)
- Maintain pool of `Fee Payer` structs (wallet address + balance)
- Round-robin selection on each claim

```rust
// x402/fee_payer.rs
pub struct FeePayerPool {
    payers: Vec<FeePayer>,
    current_index: AtomicUsize,
}

pub fn next_payer(&self) -> &FeePayer {
    let idx = self.current_index.fetch_add(1, Ordering::Relaxed);
    &self.payers[idx % self.payers.len()]
}
```

---

### Nonce Pool (nonce_pool.rs)
**Purpose:** Manage durable nonce accounts for ordered transaction submission.

**State:**
- Multiple nonce accounts (to parallelize claims)
- Each nonce account has reserved account address
- Gateway increments nonce on each use (ensures ordering)

```rust
// x402/nonce_pool.rs
pub struct NoncePool {
    accounts: Vec<NonceAccount>,
}

pub async fn acquire_nonce(&self) -> NonceAccountGuard {
    // Get least-used nonce account
    // Reserve it (prevent double-use)
    // Return guard (releases on drop)
}
```

---

## Data Storage

### PostgreSQL (sqlx)
**Version:** PostgreSQL 16 (optional; gateway works without it)  
**Connection:** `DATABASE_URL` env var (e.g., `postgres://user:pass@localhost:5432/solvela`)  
**Driver:** `sqlx` (compile-time SQL verification, runtime-checked queries)

**Usage:**
- Spend logs (fire-and-forget writes, analytics reads)
- Organizations, teams, members, API keys
- Audit logs
- Escrow claim queue

**Connection Pooling:** Tokio-based pool (default 10 connections).

```toml
# Cargo.toml
sqlx = { version = "0.8", features = ["runtime-tokio", "postgres", "chrono", "uuid"] }
```

---

### Redis (redis crate)
**Version:** Redis 7 (optional; gateway works without it)  
**Connection:** `REDIS_URL` env var (e.g., `redis://localhost:6379`)  
**Driver:** `redis` crate with tokio-comp feature

**Usage:**
- **Response Cache** — Cache chat completions by (model, messages, temperature) hash
- **Replay Protection** — Track `payment-signature` nonces to prevent replay attacks

**Cache TTL:**
- Response cache: 600 seconds / 10 minutes (default; see `cache::ResponseCacheConfig::default_ttl_secs`)
- Replay protection: ~120 seconds (matches Solana blockhash lifetime)

```rust
// cache.rs
pub struct ResponseCache {
    client: redis::aio::Connection,
}

pub async fn get(&self, key: &str) -> Result<Option<String>> {
    redis::cmd("GET").arg(key).query_async(&mut self.client).await
}

pub async fn set(&self, key: &str, value: &str, ttl_secs: usize) -> Result<()> {
    redis::cmd("SETEX").arg(key).arg(ttl_secs).arg(value)
        .query_async(&mut self.client).await
}
```

---

## Third-Party Crates (Key Dependencies)

| Crate | Version | Purpose | Used By |
|-------|---------|---------|---------|
| **axum** | 0.8 | Web framework | gateway (routes, middleware) |
| **tokio** | 1 (full) | Async runtime | All binaries |
| **tower** | 0.5 | Tower service traits | gateway |
| **tower-http** | 0.6 | HTTP middleware (cors, trace, timeout, limit, set-header, catch-panic) | gateway |
| **reqwest** | 0.12 | HTTP client | Provider adapters, Solana RPC |
| **serde** + **serde_json** | 1 | Serialization | All crates |
| **sqlx** | 0.8 | PostgreSQL driver | gateway (optional) |
| **redis** | 1.2 | Redis client | gateway (optional) |
| **ed25519-dalek** + **curve25519-dalek** | 2/4 | Solana sig verification | x402 |
| **bs58** | 0.5 | Base58 encoding (Solana addrs) | x402 |
| **sha2** + **hmac** | 0.11/0.13 | Hashing, HMAC | gateway (session tokens, cache keys) |
| **base64** | 0.22 | Base64 encoding | Payment header decoding |
| **tracing** + **tracing-subscriber** | 0.1 / 0.3 | Structured logging | All crates |
| **metrics** + **metrics-exporter-prometheus** | 0.24 / 0.18 | Prometheus metrics | gateway |
| **clap** | 4 | CLI parsing | cli (derive macros) |
| **thiserror** | 2 | Error macros | Libraries |
| **anyhow** | 1 | Error context | Binaries |
| **uuid** + **chrono** | 1 / 0.4 | UUIDs, timestamps | All crates |
| **toml** | 1.1 | Config parsing | gateway |
| **dotenvy** | 0.15 | .env file loading | gateway |
| **zeroize** | 1 | Secure key cleanup | x402 (secret material) |
| **lru** | 0.18 | LRU cache | gateway (replay protection fallback) |

---

## Configuration Files Integration

### config/default.toml
Loaded on startup; env vars override. The shipped defaults point at devnet — production deploys override via env.

```toml
[server]
host = "0.0.0.0"
port = 8402

[solana]
rpc_url = "https://api.devnet.solana.com"
recipient_wallet = ""
usdc_mint = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"

[monitor]
warn_threshold_sol = 0.1
critical_threshold_sol = 0.02
check_interval_secs = 300

[cache.semantic]
enabled = false
threshold = 0.85
hit_price_percent = 30
ttl_secs = 600

[providers]
# Provider API keys come from env: OPENAI_API_KEY, ANTHROPIC_API_KEY, GOOGLE_API_KEY, XAI_API_KEY, DEEPSEEK_API_KEY.
```

There are no `[logging]` or `[rate_limit]` sections in `config/default.toml`; log level is set via `RUST_LOG` and rate-limit defaults live in `crates/gateway/src/middleware/rate_limit.rs` (60 req / 60 s per wallet, hard-coded).

### config/models.toml
Model registry; 26+ models with per-token pricing.

```toml
[models.openai-gpt-4o]
provider = "openai"
model_id = "gpt-4o"
display_name = "GPT-4o"
input_cost_per_million = 2.50
output_cost_per_million = 10.00
context_window = 128000
supports_streaming = true
supports_tools = true
supports_vision = true
```

### config/services.toml
Service marketplace registry (external LLM service metadata).

---

## SDK Integrations

All SDKs live in-tree under `sdks/` (see `docs/runbooks/sdk-consolidation.md` for the consolidation history). The illustrative snippets below show the shape of each SDK's published API — consult each SDK's README for exact import paths and constructor signatures.

- **TypeScript** (`sdks/typescript/`) — published as `@solvela/sdk` on npm
- **Python** (`sdks/python/`) — published as `solvela-sdk` on PyPI
- **Go** (`sdks/go/`) — module `github.com/solvela-ai/solvela/sdks/go`
- **Rust** (`sdks/rust/`) — `solvela-client` family on crates.io (non-workspace member, like `programs/escrow/`)
- **Vercel AI SDK provider** (`sdks/ai-sdk-provider/`)
- **OpenClaw provider** (`sdks/openclaw-provider/`)
- **Signer core** (`sdks/signer-core/`)
- **MCP server** (`sdks/mcp/`) — Claude Desktop / MCP host integration
- **CLI distribution shim** (`sdks/cli-npm/`)

---

## Environment Variable Reference

**LLM Provider Keys:**
- `OPENAI_API_KEY` — OpenAI secret key
- `ANTHROPIC_API_KEY` — Anthropic API key
- `GOOGLE_API_KEY` — Google Gemini API key
- `DEEPSEEK_API_KEY` — DeepSeek API key
- `XAI_API_KEY` — xAI API key

**Solana Configuration** (double-underscore is canonical; single-underscore form accepted as fallback):
- `SOLVELA_SOLANA__RPC_URL` — Solana RPC endpoint
- `SOLVELA_SOLANA__RECIPIENT_WALLET` — USDC recipient address
- `SOLVELA_SOLANA__USDC_MINT` — USDC mint (default mainnet)
- `SOLVELA_SOLANA__ESCROW_PROGRAM_ID` — Escrow program ID
- `SOLVELA_SOLANA__FEE_PAYER_KEY` (plus `_2`…`_8`) — Fee payer private key(s) (base64 or JSON byte array)

**Data Storage:**
- `DATABASE_URL` — PostgreSQL connection string (optional)
- `REDIS_URL` — Redis connection string (optional)

**Server & Admin:**
- `SOLVELA_HOST` — Listen address (default 0.0.0.0)
- `SOLVELA_PORT` — Listen port (default 8402)
- `SOLVELA_ADMIN_TOKEN` — Admin endpoint access token (optional)

---

## Related

- [Architecture Overview](architecture.md)
- [Backend Routes](backend.md)
- [Data Schema](data.md)
