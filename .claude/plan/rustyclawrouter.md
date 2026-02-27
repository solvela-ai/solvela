# Implementation Plan: RustyClawRouter

> Solana-native AI agent payment infrastructure — a Rust-based alternative to BlockRun.AI.
> AI agents pay for LLM API calls with USDC-SPL on Solana. No API keys, no accounts, just wallets.

---

## Task Type
- [x] Backend (Rust API gateway + Solana program)
- [x] Frontend (Dashboard — Next.js)
- [x] Fullstack (SDKs, CLI, MCP integrations)

---

## Technical Solution

### Core Concept
Replace API keys with Solana wallet signatures. The x402 protocol embeds USDC-SPL micropayments directly into HTTP requests. An AI agent requests an LLM API → gets HTTP 402 with the price → signs a USDC-SPL transfer → retries with the signed payment → gets the LLM response.

### Architecture Overview

```
┌─────────────────────────────────────────────────────────────────────┐
│                         CLIENT LAYER                                │
│                                                                     │
│  Python SDK        TypeScript SDK       Rust SDK       Go SDK       │
│  rustyclawrouter   @rustyclawrouter/sdk rustyclawrouter rcr-go     │
│                                                                     │
│  MCP Server (Claude Code / OpenClaw integration)                    │
│  CLI Tool (wallet mgmt, stats, diagnostics)                         │
└──────────────────────────┬──────────────────────────────────────────┘
                           │ HTTPS + PAYMENT-SIGNATURE header (x402 v2)
                           ▼
┌─────────────────────────────────────────────────────────────────────┐
│              RUSTYCLAWROUTER API GATEWAY (Rust / Axum)              │
│                                                                     │
│  ┌──────────┐  ┌───────────┐  ┌──────────┐  ┌──────────────────┐  │
│  │ x402     │  │ Smart     │  │ Response │  │ Token            │  │
│  │ Payment  │  │ Router    │  │ Cache    │  │ Compression      │  │
│  │ Middleware│  │ (15-dim)  │  │ (Redis)  │  │ (LLMLingua svc)  │  │
│  └──────────┘  └───────────┘  └──────────┘  └──────────────────┘  │
│                                                                     │
│  ┌──────────────────┐  ┌─────────────────┐  ┌──────────────────┐  │
│  │ Provider Proxy    │  │ Usage Tracker   │  │ Budget Manager   │  │
│  │ (OpenAI, Claude,  │  │ (PostgreSQL)    │  │ (per-wallet)     │  │
│  │  Gemini, Grok...) │  │                 │  │                  │  │
│  └──────────────────┘  └─────────────────┘  └──────────────────┘  │
│                                                                     │
│  Endpoints:                                                         │
│   POST /v1/chat/completions    (OpenAI-compatible)                  │
│   POST /v1/images/generations  (Image gen)                          │
│   GET  /v1/models              (List models)                        │
│   GET  /v1/services            (x402 service discovery — Phase 6)   │
│   GET  /pricing                (Model pricing + fee breakdown)      │
│   GET  /health                 (Health check)                       │
└──────────────────────────┬──────────────────────────────────────────┘
                           │
              ┌────────────┼────────────────────┐
              ▼            ▼                    ▼
         OpenAI        Anthropic           Google/xAI/DeepSeek
                           │
                           ▼ (on-chain settlement)
┌─────────────────────────────────────────────────────────────────────┐
│                      SOLANA MAINNET                                 │
│                                                                     │
│  Phase 1: Direct USDC-SPL TransferChecked                          │
│           (pre-signed tx, facilitator settles)                      │
│                                                                     │
│  Phase 2: Anchor Escrow Program                                    │
│           PDA vault → make/take/refund                              │
│           Trustless, timeout-based refunds                          │
│                                                                     │
│  USDC Mint: EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v         │
│  Tx cost: ~$0.0008/tx (~5000 lamports base fee)                    │
└─────────────────────────────────────────────────────────────────────┘
```

### Key Differentiators vs BlockRun
| Aspect | BlockRun | RustyClawRouter |
|--------|----------|-----------------|
| Gateway language | Node.js/TypeScript | **Rust (Axum)** — sub-microsecond routing overhead |
| Primary chain | Base (EVM) | **Solana** — 3.75× cheaper fees, 5× faster finality |
| Payment primitive | EIP-3009 TransferWithAuthorization | **SPL TransferChecked** (pre-signed versioned tx) |
| On-chain program | None (direct transfers only) | **Anchor escrow** (Phase 2) — trustless settlement |
| Token | USDC ERC-20 | **USDC-SPL** |
| Fee payer | CDP Facilitator | **Self-hosted fee payer** (SOL hot wallet) |
| Multi-chain | Base only | **Solana** (future: Base/EVM compatibility) |
| Service scope | LLM inference only | **LLM + any x402 service** (marketplace, Phase 6) |

---

## Implementation Steps

### Phase 1: Core Gateway + x402 Payments (Weeks 1–4)

> **Goal**: Working Rust API gateway that accepts Solana x402 payments and proxies to one LLM provider.

#### Step 1.1: Project Scaffolding
- **Expected deliverable**: Cargo workspace with `gateway`, `x402`, `router`, `common` crates
- Initialize Cargo workspace:
  ```
  rustyclawrouter/
  ├── Cargo.toml                    (workspace root)
  ├── crates/
  │   ├── gateway/                  (Axum HTTP server)
  │   │   ├── Cargo.toml
  │   │   └── src/
  │   │       ├── main.rs
  │   │       ├── routes/
  │   │       │   ├── mod.rs
  │   │       │   ├── chat.rs       (/v1/chat/completions)
  │   │       │   ├── models.rs     (/v1/models)
  │   │       │   ├── images.rs     (/v1/images/generations)
  │   │       │   └── health.rs     (/health)
  │   │       ├── middleware/
  │   │       │   ├── mod.rs
  │   │       │   ├── x402.rs       (payment verification)
  │   │       │   └── rate_limit.rs
  │   │       ├── providers/
  │   │       │   ├── mod.rs
  │   │       │   ├── openai.rs
  │   │       │   ├── anthropic.rs
  │   │       │   ├── google.rs
  │   │       │   ├── xai.rs
  │   │       │   └── deepseek.rs
  │   │       ├── config.rs
  │   │       └── error.rs
  │   ├── x402/                     (x402 protocol implementation)
  │   │   ├── Cargo.toml
  │   │   └── src/
  │   │       ├── lib.rs
  │   │       ├── types.rs          (PaymentRequired, PaymentPayload, etc.)
│   │       ├── traits.rs         (PaymentVerifier trait — chain-agnostic, future multi-chain)
│   │       ├── solana.rs         (Solana-specific signing/verification)
│   │       ├── facilitator.rs    (settlement service)
│   │       └── middleware.rs     (Axum middleware layer)
  │   ├── router/                   (smart routing engine)
  │   │   ├── Cargo.toml
  │   │   └── src/
  │   │       ├── lib.rs
  │   │       ├── scorer.rs         (15-dimension weighted scorer)
  │   │       ├── profiles.rs       (eco/auto/premium/free)
  │   │       └── models.rs         (model registry + pricing)
  │   └── common/                   (shared types, utils)
  │       ├── Cargo.toml
  │       └── src/
  │           ├── lib.rs
  │           ├── types.rs          (ChatMessage, ChatResponse, etc.)
  │           └── error.rs
  ├── programs/                     (Phase 2: Anchor programs)
  │   └── escrow/
  ├── sdks/
  │   ├── python/
  │   ├── typescript/
  │   └── go/
  ├── config/
  │   ├── models.toml               (model registry + pricing)
  │   └── default.toml              (gateway config)
  └── docker-compose.yml            (Redis + PostgreSQL)
  ```

- Key Cargo.toml dependencies:
  ```toml
  [workspace.dependencies]
  axum = "0.8"
  tokio = { version = "1", features = ["full"] }
  serde = { version = "1", features = ["derive"] }
  serde_json = "1"
  reqwest = { version = "0.12", features = ["json", "stream"] }
  solana-sdk = "2.2"
  solana-client = "2.2"
  spl-token = "7"
  spl-associated-token-account = "5"
  base64 = "0.22"
  ed25519-dalek = "2"
  redis = { version = "0.27", features = ["tokio-comp"] }
  sqlx = { version = "0.8", features = ["runtime-tokio", "postgres", "chrono", "uuid"] }
  tower = "0.5"
  tower-http = { version = "0.6", features = ["cors", "trace", "timeout"] }
  tracing = "0.1"
  tracing-subscriber = "0.3"
  uuid = { version = "1", features = ["v4"] }
  chrono = { version = "0.4", features = ["serde"] }
  config = "0.14"
  thiserror = "2"
  anyhow = "1"
  ```

#### Step 1.2: x402 Protocol Implementation (`crates/x402`)
- **Expected deliverable**: Full x402 v2 Solana payment verification + settlement
- Implement core types:
  ```rust
  // PaymentRequired — returned in 402 response
  pub struct PaymentRequired {
      pub x402_version: u8,  // 2
      pub resource: Resource,
      pub accepts: Vec<PaymentAccept>,
      pub error: String,
  }
  
  pub struct PaymentAccept {
      pub scheme: String,                // "exact"
      pub network: String,               // "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp"
      pub amount: String,                // atomic USDC units (6 decimals)
      pub asset: String,                 // USDC mint pubkey
      pub pay_to: String,                // recipient wallet pubkey
      pub max_timeout_seconds: u64,      // 300
  }
  
  // PaymentPayload — sent in PAYMENT-SIGNATURE header
  pub struct PaymentPayload {
      pub x402_version: u8,
      pub resource: Resource,
      pub accepted: PaymentAccept,
      pub payload: SolanaPayload,
  }
  
  pub struct SolanaPayload {
      pub transaction: String,  // base64-encoded signed versioned tx
  }
  ```

- Implement Solana payment verification:
  1. Decode base64 transaction from `PAYMENT-SIGNATURE` header
  2. Deserialize as `VersionedTransaction`
  3. Introspect SPL Token transfer instruction:
     - Verify program ID = `TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA`
     - Verify instruction type byte = `12` (TransferChecked) or `3` (Transfer)
     - Verify destination ATA matches our recipient
     - Verify amount ≥ required price
  4. Simulate transaction via `simulateTransaction` RPC
  5. If valid: broadcast via `sendTransaction` RPC
  6. Confirm via `confirmTransaction` RPC
  7. Verify post-tx token balances

- Implement fee payer service:
  - Hot wallet with SOL for tx fees
  - Agent signs tx → gateway co-signs as fee payer → submit
  - Fee payer wallet: funded with ~1 SOL ($100-150, covers ~400K txns)
  - Monitor balance, alert at threshold

#### Step 1.3: API Gateway Core (`crates/gateway`)
- **Expected deliverable**: Axum server with OpenAI-compatible endpoints
- Implement `POST /v1/chat/completions`:
  - Request body: OpenAI-compatible (model, messages, max_tokens, temperature, etc.)
  - If no `PAYMENT-SIGNATURE` header → return HTTP 402 with `payment-required` header
  - If `PAYMENT-SIGNATURE` present → verify payment → proxy to provider → return response
  - Stream support via SSE (Server-Sent Events) for streaming completions

- Implement provider proxy layer:
  - Each provider in `providers/` module translates OpenAI format → provider format → back
  - Start with OpenAI (direct passthrough) + Anthropic (format translation)
  - Provider trait:
    ```rust
    #[async_trait]
    pub trait LLMProvider: Send + Sync {
        fn name(&self) -> &str;
        fn supported_models(&self) -> Vec<ModelInfo>;
        async fn chat_completion(&self, req: ChatRequest) -> Result<ChatResponse>;
        async fn chat_completion_stream(&self, req: ChatRequest) -> Result<impl Stream<Item = ChatChunk>>;
    }
    ```

- Implement `GET /v1/models` — return all supported models with pricing
- Implement `GET /health` — gateway health + Solana RPC status

#### Step 1.4: Model Registry + Pricing (`config/models.toml`)
- **Expected deliverable**: Configuration-driven model catalog with pricing
- TOML-based model registry:
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
  
  [models.anthropic-claude-sonnet-4]
  provider = "anthropic"
  model_id = "claude-sonnet-4-20250514"
  display_name = "Claude Sonnet 4"
  input_cost_per_million = 3.00
  output_cost_per_million = 15.00
  context_window = 200000
  supports_streaming = true
  supports_tools = true
  reasoning = true
  ```

- Pricing formula: `user_cost = provider_cost × 1.05` (5% platform fee)
- **What the 5% covers** (documented transparently on pricing page + in responses):
  - USDC settlement infrastructure (fee payer SOL costs, RPC nodes)
  - Smart routing engine (model selection, fallback, circuit breakers)
  - Response caching infrastructure (Redis)
  - Usage tracking and analytics
  - SDK and tooling maintenance
- 402 response includes exact cost breakdown in `PAYMENT-REQUIRED` header:
  ```json
  {
    "x402_version": 2,
    "accepts": [{ "...": "..." }],
    "cost_breakdown": {
      "provider_cost": "0.002500",
      "platform_fee": "0.000125",
      "total": "0.002625",
      "currency": "USDC",
      "fee_percent": 5
    }
  }
  ```
- `GET /pricing` endpoint returns all models with final user-facing prices + fee split
- SDKs expose `get_cost_estimate(model, input_tokens, output_tokens)` before making a paid call
- Dashboard analytics show provider cost vs platform fee split per request

#### Step 1.5: Docker Compose + Integration Tests
- **Expected deliverable**: One-command local dev environment
- `docker-compose.yml` with Redis + PostgreSQL + Solana test validator
- Integration tests:
  - Full x402 payment flow (402 → sign → verify → settle) on localnet
  - Provider proxy (mock upstream, verify format translation)
  - Streaming response tests

---

### Phase 2: Smart Router + Caching (Weeks 5–7)

> **Goal**: Intelligent request routing to cheapest capable model, with response caching.

#### Step 2.1: 15-Dimension Smart Router (`crates/router`)
- **Expected deliverable**: Rule-based classifier, <1µs per request, zero external calls
- Implement weighted scorer (adapted from BlockRun's `router.py`):

  | # | Dimension | Weight | Signal |
  |---|-----------|--------|--------|
  | 1 | Token count | 0.08 | Short (<50) → SIMPLE |
  | 2 | Code presence | 0.15 | `function`, `class`, backticks |
  | 3 | Reasoning markers | 0.18 | `prove`, `theorem`, `step by step` |
  | 4 | Technical terms | 0.10 | `algorithm`, `kubernetes` |
  | 5 | Creative markers | 0.05 | `story`, `poem`, `brainstorm` |
  | 6 | Simple indicators | 0.02 | `what is`, `define`, `translate` |
  | 7 | Multi-step patterns | 0.12 | `first...then`, numbered steps |
  | 8 | Question complexity | 0.05 | Count of `?` |
  | 9 | Agentic task | 0.04 | `read file`, `edit`, `deploy` |
  | 10 | Math/logic | 0.06 | Equations, formal notation |
  | 11 | Language complexity | 0.04 | Avg word length, vocabulary |
  | 12 | Conversation depth | 0.03 | Message count in context |
  | 13 | Tool usage | 0.04 | Function calling, tools requested |
  | 14 | Output format | 0.02 | JSON, structured output hints |
  | 15 | Domain specificity | 0.02 | Medical, legal, scientific terms |

- Tier boundaries:
  ```
  score < 0.0  → SIMPLE
  0.0 ≤ score < 0.3  → MEDIUM
  0.3 ≤ score < 0.5  → COMPLEX
  score ≥ 0.5  → REASONING
  ```

- Routing profiles:
  | Tier | ECO | AUTO | PREMIUM | FREE |
  |------|-----|------|---------|------|
  | SIMPLE | deepseek-chat | gemini-2.5-flash | gpt-4o | gpt-oss-120b |
  | MEDIUM | gemini-2.5-flash-lite | grok-code-fast | claude-sonnet-4 | gpt-oss-120b |
  | COMPLEX | deepseek-chat | gemini-3.1-pro | claude-opus-4.5 | gpt-oss-120b |
  | REASONING | deepseek-reasoner | grok-4-fast | o3 | gpt-oss-120b |

- Model aliases: `auto`, `eco`, `premium`, `free`, `gpt5`, `sonnet`, `grok-fast`

#### Step 2.2: Response Caching (Redis)
- **Expected deliverable**: Two-tier cache with exact + semantic matching
- **Tier 1: Exact match** — `SHA256(model + messages + temperature)` → Redis
  - TTL: 10min default, configurable per model
  - Expected hit rate: 15-30%
- **Tier 2: Semantic cache** (stretch goal) — embed query → vector similarity
  - Use Redis Vector Search (RediSearch)
  - Cosine similarity threshold: 0.95
  - Expected additional hit rate: up to 40%
- Cache middleware in Axum tower layer (check cache → miss → call provider → store)

#### Step 2.3: Provider Fallback + Circuit Breaker
- **Expected deliverable**: Automatic retry with provider fallback
- Per-provider health tracking:
  - EWMA latency (10s window)
  - Failure rate per minute
  - Rate limit tracking (429 responses)
- Circuit breaker: >50% failure rate → cooldown 30s → try next provider
- Ordered fallback list per model tier (e.g., GPT-4o fails → try Gemini Pro)

#### Step 2.4: Usage Tracking + Budget Management
- **Expected deliverable**: Per-wallet spend tracking with budget limits
- PostgreSQL schema:
  ```sql
  CREATE TABLE spend_logs (
      id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
      wallet_address TEXT NOT NULL,
      model TEXT NOT NULL,
      provider TEXT NOT NULL,
      input_tokens INTEGER NOT NULL,
      output_tokens INTEGER NOT NULL,
      cost_usdc DECIMAL(18, 6) NOT NULL,
      tx_signature TEXT,
      created_at TIMESTAMPTZ DEFAULT NOW()
  );
  
  CREATE TABLE wallet_budgets (
      wallet_address TEXT PRIMARY KEY,
      daily_limit_usdc DECIMAL(18, 6),
      monthly_limit_usdc DECIMAL(18, 6),
      total_spent_usdc DECIMAL(18, 6) DEFAULT 0,
      created_at TIMESTAMPTZ DEFAULT NOW()
  );
  
  CREATE INDEX idx_spend_wallet ON spend_logs(wallet_address);
  CREATE INDEX idx_spend_created ON spend_logs(created_at);
  ```

- Redis for hot-path rate limiting:
  ```
  rate_limit:{wallet}:{minute} → request count (TTL 60s)
  spend:{wallet}:{day} → daily spend (TTL 86400s)
  balance:{wallet} → cached USDC balance (TTL 30s)
  ```

- All DB writes are async (tokio::spawn) — never on request critical path

---

### Phase 3: SDKs + CLI (Weeks 8–11)

> **Goal**: Python + TypeScript + Go SDKs, CLI tool, MCP server.

#### Step 3.1: Python SDK (`sdks/python/`)
- **Expected deliverable**: `pip install rustyclawrouter`
- Package structure:
  ```
  sdks/python/
  ├── pyproject.toml
  ├── rustyclawrouter/
  │   ├── __init__.py
  │   ├── client.py          (LLMClient, AsyncLLMClient)
  │   ├── x402.py            (payment signing, Solana tx creation)
  │   ├── wallet.py          (key management, balance checking)
  │   ├── router.py          (client-side smart routing)
  │   ├── types.py           (ChatMessage, ChatResponse, etc.)
  │   └── config.py          (API URLs, model registry)
  └── tests/
  ```

- Key classes:
  ```python
  class LLMClient:
      def __init__(self, private_key=None, api_url=None, session_budget=None):
          # Key resolution: param → env var → ~/.rustyclawrouter/.session → auto-generate
      
      def chat(self, model: str, prompt: str, **kwargs) -> str:
          """Simple chat. Handles x402 payment transparently."""
      
      def chat_completion(self, model: str, messages: list, **kwargs) -> ChatResponse:
          """Full OpenAI-compatible completion."""
      
      def smart_chat(self, prompt: str, profile: str = "auto") -> SmartChatResponse:
          """Uses smart router to pick cheapest capable model."""
      
      def get_balance(self) -> float:
          """Check USDC-SPL balance on Solana."""
      
      def get_spending(self) -> SpendInfo:
          """Get spending stats."""
  ```

- Dependencies: `solders`, `solana-py`, `httpx`, `base64`

#### Step 3.2: TypeScript SDK (`sdks/typescript/`)
- **Expected deliverable**: `npm install @rustyclawrouter/sdk`
- Package structure:
  ```
  sdks/typescript/
  ├── package.json
  ├── tsconfig.json
  ├── src/
  │   ├── index.ts
  │   ├── client.ts           (LLMClient)
  │   ├── x402.ts             (payment signing)
  │   ├── wallet.ts           (key management)
  │   ├── router.ts           (client-side routing)
  │   ├── openai-compat.ts    (OpenAI drop-in replacement)
  │   └── types.ts
  └── tests/
  ```

- OpenAI drop-in compatibility:
  ```typescript
  import { OpenAI } from '@rustyclawrouter/sdk';
  
  const client = new OpenAI(); // Uses SOLANA_WALLET_KEY env var
  const response = await client.chat.completions.create({
    model: 'openai/gpt-4o',
    messages: [{ role: 'user', content: 'Hello!' }]
  });
  ```

- Dependencies: `@solana/web3.js`, `@solana/spl-token`, `bs58`

#### Step 3.3: CLI Tool
- **Expected deliverable**: `cargo install rustyclawrouter-cli`
- Commands:
  ```
  rcr wallet init          # Generate new Solana keypair
  rcr wallet status        # Show address + USDC balance
  rcr wallet export        # Export private key (base58)
  rcr stats [--days 7]     # Usage stats
  rcr models               # List available models + pricing
  rcr chat "Hello!"        # Quick chat with auto-routing
  rcr doctor               # AI-powered diagnostics
  ```

#### Step 3.4: MCP Server
- **Expected deliverable**: `npx @rustyclawrouter/mcp` for Claude Code integration
- Tools exposed:
  - `chat` — Send prompt to any model
  - `smart_chat` — Auto-routed chat
  - `wallet_status` — Check balance
  - `list_models` — Available models
  - `spending` — Usage stats

#### Step 3.5: Go SDK (`sdks/go/`)
- **Expected deliverable**: `go get github.com/rustyclawrouter/sdk-go`
- Package structure:
  ```
  sdks/go/
  ├── go.mod
  ├── go.sum
  ├── client.go            (LLMClient — sync + context-aware)
  ├── x402.go              (payment signing, Solana tx creation)
  ├── wallet.go            (key management, balance checking)
  ├── router.go            (client-side smart routing)
  ├── types.go             (ChatMessage, ChatResponse, etc.)
  ├── config.go            (API URLs, model registry)
  └── client_test.go
  ```

- Key interface:
  ```go
  type Client struct { ... }

  func NewClient(opts ...Option) (*Client, error)
  func (c *Client) Chat(ctx context.Context, model, prompt string) (string, error)
  func (c *Client) ChatCompletion(ctx context.Context, req ChatRequest) (*ChatResponse, error)
  func (c *Client) SmartChat(ctx context.Context, prompt string, profile Profile) (*SmartChatResponse, error)
  func (c *Client) GetBalance(ctx context.Context) (float64, error)
  func (c *Client) GetCostEstimate(model string, inputTokens, outputTokens int) (*CostBreakdown, error)
  func (c *Client) GetSpending(ctx context.Context) (*SpendInfo, error)
  ```

- Dependencies: `github.com/gagliardetto/solana-go`, `net/http` (stdlib)
- Design: idiomatic Go — `context.Context` everywhere, `Option` pattern for config, exported errors

---

### Phase 4: Anchor Escrow Program (Weeks 12–14)

> **Goal**: Trustless on-chain escrow for production payment settlement.

#### Step 4.1: Anchor Program (`programs/escrow/`)
- **Expected deliverable**: Deployed Anchor program with 3 instructions
- Program structure:
  ```rust
  // programs/escrow/src/lib.rs
  use anchor_lang::prelude::*;
  use anchor_spl::token::{self, Token, TokenAccount, Transfer, Mint};
  use anchor_spl::associated_token::AssociatedToken;
  
  declare_id!("...");
  
  #[program]
  pub mod rustyclawrouter_escrow {
      use super::*;
  
      /// Agent deposits USDC into PDA vault, specifying the service + max amount
      pub fn deposit(ctx: Context<Deposit>, amount: u64, service_id: [u8; 32]) -> Result<()> {
          // Transfer USDC from agent ATA → PDA vault ATA
          // Store deposit metadata in Escrow account
      }
  
      /// Service provider claims payment after delivering the service
      pub fn claim(ctx: Context<Claim>, actual_amount: u64) -> Result<()> {
          // Verify actual_amount ≤ deposited amount
          // Transfer actual_amount from vault → provider ATA
          // Refund remainder to agent ATA
          // Close escrow account
      }
  
      /// Agent reclaims funds if service not delivered (after timeout)
      pub fn refund(ctx: Context<Refund>) -> Result<()> {
          // Check: current_slot > escrow.expiry_slot
          // Transfer all funds from vault → agent ATA
          // Close escrow account
      }
  }
  
  #[account]
  #[derive(InitSpace)]
  pub struct Escrow {
      pub agent: Pubkey,          // depositor
      pub provider: Pubkey,       // service provider
      pub mint: Pubkey,           // USDC mint
      pub amount: u64,            // deposited amount
      pub service_id: [u8; 32],   // API call correlation ID
      pub expiry_slot: u64,       // timeout for refund
      pub bump: u8,
  }
  ```

- PDA derivation: `seeds = [b"escrow", agent.key().as_ref(), &service_id]`
- Testing: LiteSVM for fast unit tests, Solana test validator for integration

#### Step 4.2: Gateway Integration
- **Expected deliverable**: Gateway uses escrow for all production payments
- Payment flow upgrade:
  ```
  Agent → POST /v1/chat/completions → 402 (price)
  Agent signs deposit instruction (USDC → PDA vault)
  Agent → POST /v1/chat/completions + PAYMENT-SIGNATURE
  Gateway verifies deposit on-chain
  Gateway calls LLM provider
  Gateway claims actual cost from vault (refunds overage)
  Gateway → 200 OK + response
  ```

- Advantages over direct transfer:
  - Agent only pays actual cost (not estimated max)
  - Automatic refund on service failure
  - Timeout-based refund if gateway goes down

#### Step 4.3: Fee Payer Service Hardening
- **Expected deliverable**: Production-grade fee payer with monitoring
- Hot wallet rotation (2+ fee payer keys)
- Balance monitoring + auto-top-up alerts
- Rate limiting per agent wallet
- Durable nonces for long-lived transactions (avoids blockhash expiry)

---

### Phase 5: Dashboard + Enterprise (Weeks 15–17)

> **Goal**: Web dashboard for analytics, team management, and enterprise features.

#### Step 5.1: Admin Dashboard (Next.js)
- **Expected deliverable**: Web UI at `dashboard.rustyclawrouter.com`
- Stack: Next.js 15 + Tailwind CSS + shadcn/ui
- Pages:
  - **Overview**: Total spend, request count, cost savings chart
  - **Usage**: Per-model breakdown, per-wallet breakdown
  - **Models**: Available models, pricing, status
  - **Wallet**: Balance, transaction history, funding instructions
  - **Settings**: Budget limits, team management, API config

#### Step 5.2: Marketing Website
- **Expected deliverable**: Landing page at `rustyclawrouter.com`
- Pages: Home, Models, Pricing, Docs, Enterprise

#### Step 5.3: Enterprise Features
- **Expected deliverable**: Team/org billing, SSO, audit logs
- Organization → Team → User hierarchy
- Per-team budget limits
- Audit log for all transactions
- SSO via SAML/OIDC

---

### Phase 6: x402 Service Marketplace (Weeks 18–19)

> **Goal**: Open marketplace for any x402-compatible service — not just LLMs.

#### Step 6.1: x402 Service Marketplace
- **Expected deliverable**: Registry + discovery for any x402-compatible service
- Service registry (`config/services.toml`):
  ```toml
  [services.llm-gateway]
  name = "LLM Intelligence"
  endpoint = "/v1/chat/completions"
  category = "intelligence"
  x402_enabled = true
  internal = true

  [services.image-gen]
  name = "Image Generation"
  endpoint = "/v1/images/generations"
  category = "media"
  x402_enabled = true
  internal = true

  [services.weather-api]
  name = "Weather Data"
  endpoint = "https://weather.example.com/v1/forecast"
  category = "data"
  x402_enabled = true
  internal = false
  provider_fee_percent = 0  # external service sets own price

  [services.web-search]
  name = "Web Search"
  endpoint = "https://search.example.com/v1/query"
  category = "search"
  x402_enabled = true
  internal = false
  ```
- Discovery endpoint: `GET /v1/services` — returns available x402 services with pricing
  ```json
  {
    "services": [
      {
        "id": "llm-gateway",
        "name": "LLM Intelligence",
        "category": "intelligence",
        "endpoint": "/v1/chat/completions",
        "pricing": "per-token (see /pricing)",
        "chains": ["solana"]
      },
      {
        "id": "web-search",
        "name": "Web Search",
        "category": "search",
        "endpoint": "https://search.example.com/v1/query",
        "pricing": "$0.005/query",
        "chains": ["solana"]
      }
    ]
  }
  ```
- **Proxy mode** for external x402 services:
  - Gateway proxies requests to registered external services
  - 5% platform fee on all proxied transactions
  - Handles x402 payment flow on behalf of the agent
- **Service registration API** (Phase 6 stretch):
  - `POST /v1/services/register` — third parties register their x402 endpoints
  - Requires minimum USDC stake or admin approval to prevent spam
  - Automatic health checking of registered services

#### Step 6.2: Marketplace Integration + Testing
- **Expected deliverable**: End-to-end tests for service discovery and proxying
- Integration tests for external service proxying
- Load testing for marketplace discovery endpoint
- Documentation for third-party service providers

---

## Key Files

| File | Operation | Description |
|------|-----------|-------------|
| `Cargo.toml` | Create | Workspace root with all crate dependencies |
| `crates/gateway/src/main.rs` | Create | Axum server entry point |
| `crates/gateway/src/routes/chat.rs` | Create | `/v1/chat/completions` handler |
| `crates/gateway/src/middleware/x402.rs` | Create | x402 payment verification middleware |
| `crates/gateway/src/providers/*.rs` | Create | LLM provider adapters (OpenAI, Anthropic, etc.) |
| `crates/x402/src/traits.rs` | Create | Chain-agnostic PaymentVerifier trait (future multi-chain ready) |
| `crates/x402/src/solana.rs` | Create | Solana tx verification + settlement |
| `crates/x402/src/types.rs` | Create | x402 v2 protocol data structures |
| `crates/router/src/scorer.rs` | Create | 15-dimension weighted request scorer |
| `crates/router/src/profiles.rs` | Create | Routing profiles (eco/auto/premium/free) |
| `config/models.toml` | Create | Model registry with pricing |
| `config/default.toml` | Create | Gateway configuration |
| `programs/escrow/src/lib.rs` | Create | Anchor escrow program (Phase 4) |
| `sdks/python/rustyclawrouter/client.py` | Create | Python SDK client |
| `sdks/typescript/src/client.ts` | Create | TypeScript SDK client |
| `sdks/go/client.go` | Create | Go SDK client |
| `config/services.toml` | Create | x402 service marketplace registry (Phase 6) |
| `docker-compose.yml` | Create | Redis + PostgreSQL + Solana validator |

---

## Risks and Mitigation

| Risk | Probability | Impact | Mitigation |
|------|------------|--------|------------|
| Solana tx confirmation latency (400ms) adds perceived delay | High | Medium | Pre-fund escrow approach; batch settlements; cache balances in Redis |
| Fee payer wallet runs out of SOL | Medium | High | Multi-wallet rotation; balance alerts at 0.1 SOL; auto-top-up script |
| Blockhash expiry (60s) causes payment failures for slow agents | Medium | Medium | Implement durable nonces for long-lived payment authorizations |
| LLM provider rate limits at scale | High | Medium | Multi-provider fallback; per-provider rate tracking; circuit breakers |
| USDC ATA creation cost ($0.16) surprises new users | Low | Low | Pre-create ATAs at wallet init; document clearly |
| Rust compilation times slow development | Medium | Low | Use `cargo-watch`, incremental builds, split into small crates |
| Smart router misclassifies requests | Medium | Low | Fallback to user-specified model; A/B test routing accuracy |
| Upstream provider API format changes | Low | Medium | Version-pinned provider adapters; integration test suite per provider |
| Service marketplace spam/abuse registrations | Low | Medium | Require minimum USDC stake or admin approval for third-party service registration |

---

## Technology Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Language | Rust | Sub-µs routing, memory safety, Solana-native ecosystem |
| Web framework | Axum 0.8 | Tower middleware, async-first, best Rust web framework |
| Solana SDK | `solana-sdk 2.2` + `anchor-lang 0.31` | Official SDKs, well-maintained |
| Database | PostgreSQL (sqlx) | Spend logs, model config, audit trail |
| Cache | Redis 7 | Rate limiting, response cache, hot-path data |
| Config | TOML | Rust-native, human-readable model registry |
| Testing | `tokio::test` + LiteSVM | Fast unit tests, Solana program testing |
| CI/CD | GitHub Actions | Build, test, deploy pipeline |
| Go SDK | `gagliardetto/solana-go` + stdlib `net/http` | Idiomatic Go, matches BlockRun's Go offering |
| Deployment | Docker + Fly.io / Railway | Easy Rust deployment, global edge |

---

## Development Order (Critical Path)

```
Week 1:  Scaffolding + x402 types + PaymentVerifier trait + Solana payment verification
Week 2:  Axum gateway + OpenAI provider proxy + 402 flow (with cost breakdown)
Week 3:  Anthropic/Google providers + streaming support
Week 4:  Fee payer service + integration tests + Docker compose
Week 5:  Smart router (15-dim scorer) + routing profiles
Week 6:  Redis caching (exact match) + circuit breaker
Week 7:  Usage tracking (PostgreSQL) + budget management
Week 8:  Python SDK (client + x402 + wallet + cost_estimate)
Week 9:  TypeScript SDK (client + OpenAI compat)
Week 10: Go SDK (client + x402 + wallet)
Week 11: CLI tool + MCP server
Week 12: Anchor escrow program + LiteSVM tests
Week 13: Gateway escrow integration + durable nonces
Week 14: Escrow devnet deployment + E2E tests
Week 15: Next.js dashboard (overview + usage + wallet + fee analytics)
Week 16: Marketing site + docs
Week 17: Enterprise features + production hardening
Week 18: x402 service registry + discovery endpoint + proxy mode
Week 19: Service marketplace integration tests + documentation
```

---

## Future Features (Post-Launch)

### Base / EVM Chain Compatibility
> **Priority**: After Solana launch is stable and marketplace is live.
> **Rationale**: BlockRun owns the Base/EVM x402 market. We differentiate on Solana performance first, then expand to Base for ecosystem compatibility.

- **EVM/Base payment verification** (`crates/x402/src/evm.rs`):
  - EIP-712 typed data verification (matching BlockRun/Coinbase x402 standard)
  - USDC ERC-20 `TransferWithAuthorization` (EIP-3009)
  - Integration with Coinbase x402 facilitator for settlement
  - Dependencies: `alloy` or `ethers-rs` for EVM interaction
- **Refactor Solana impl to `PaymentVerifier` trait** (trait already designed in Phase 1):
  ```rust
  #[async_trait]
  pub trait PaymentVerifier: Send + Sync {
      fn network(&self) -> &str;
      fn supported_assets(&self) -> Vec<AssetInfo>;
      async fn verify_payment(&self, payload: &PaymentPayload) -> Result<VerificationResult>;
      async fn settle_payment(&self, payload: &PaymentPayload) -> Result<SettlementResult>;
      async fn estimate_fees(&self) -> Result<FeeEstimate>;
  }
  ```
- Gateway auto-detects chain from `network` field in `PAYMENT-SIGNATURE` header
- SDKs auto-detect chain from wallet type (Solana keypair vs EVM private key)
- **Multi-chain SDK updates**:
  - Python: add `eth-account`, `web3.py` as optional dependencies
  - TypeScript: add `viem` or `ethers` as optional peer dependency
  - Go: add `go-ethereum` as optional dependency
  - New env var: `EVM_WALLET_KEY` alongside existing `SOLANA_WALLET_KEY`
- **Risks**:
  - EVM integration adds scope — mitigated by trait abstraction designed upfront
  - Multi-chain wallet UX confusion — mitigated by auto-detection + clear docs
  - Competing with BlockRun on Base (their home turf) — mitigated by Solana-first positioning

### Additional Future Chains
- XRPL (BlockRun mentions this on their roadmap)
- Arbitrum, Optimism, other EVM L2s (via same `PaymentVerifier` trait)

---

## SESSION_ID (for /ccg:execute use)
- CODEX_SESSION: N/A (codeagent-wrapper not installed)
- GEMINI_SESSION: N/A (codeagent-wrapper not installed)
- Research sessions preserved:
  - x402 protocol: `ses_360419c1dffeOq3zbgaHPbfgPs`
  - Solana payments: `ses_3604183beffeCmbcjAURlgl1Yx`
  - LLM gateway patterns: `ses_360416abaffeFijYylhU7usuoh`
  - BlockRun architecture: `ses_360414ef0ffepL0V8GkgUTqbuN`
  - BlockRun deep research (5% model, multi-chain, marketplace): current session
