# RustyClawRouter

Solana-native AI agent payment infrastructure. AI agents pay for LLM API calls with USDC-SPL on Solana via the [x402 protocol](https://www.x402.org/). No API keys, no accounts, just wallets.

## How It Works

```
Agent → POST /v1/chat/completions → 402 Payment Required (price quote)
Agent signs USDC-SPL TransferChecked on Solana
Agent → POST /v1/chat/completions + PAYMENT-SIGNATURE header → 200 OK (LLM response)
```

An AI agent requests an LLM API call, receives an HTTP 402 with the USDC price, signs a Solana transaction paying that amount, and retries with the signed payment attached. The gateway verifies the payment on-chain, proxies to the LLM provider, and returns the response.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│  CLIENT LAYER                                               │
│  Python SDK · TypeScript SDK · Go SDK · Rust CLI · MCP     │
└────────────────────────┬────────────────────────────────────┘
                         │ HTTPS + PAYMENT-SIGNATURE header (x402 v2)
                         ▼
┌─────────────────────────────────────────────────────────────┐
│  RUSTYCLAWROUTER GATEWAY (Rust / Axum)                      │
│                                                             │
│  x402 Payment Middleware    Smart Router (15-dim scorer)    │
│  Prompt Guard (injection/PII)  Response Cache (Redis)       │
│  Usage Tracker (PostgreSQL) Rate Limiter (per-wallet)       │
│  Circuit Breaker (per-provider)  Provider Fallback          │
│                                                             │
│  Provider Adapters: OpenAI · Anthropic · Google · xAI · DeepSeek │
│                                                             │
│  POST /v1/chat/completions        GET /v1/models            │
│  POST /v1/images/generations*     GET /v1/supported         │
│  GET  /pricing                    GET /health               │
│                                                             │
│  * scaffolded, returns 501 until image provider is added    │
└────────────────────────┬────────────────────────────────────┘
                         │ on-chain settlement
                         ▼
┌─────────────────────────────────────────────────────────────┐
│  SOLANA (devnet / mainnet)                                  │
│                                                             │
│  Phase 1: USDC-SPL TransferChecked (direct, pre-signed tx)  │
│  Phase 4: Anchor Escrow (programs/escrow/) — scaffolded     │
│           deposit → claim → refund with PDA vault           │
└─────────────────────────────────────────────────────────────┘
```

## Project Structure

```
crates/
  gateway/     Axum HTTP server — routes, middleware, provider adapters
  x402/        x402 protocol — types, Solana verification, facilitator
  router/      Smart routing — 15-dimension scorer, profiles, model registry
  common/      Shared types — ChatRequest, ChatResponse, CostBreakdown
  cli/         CLI tool (rcr) — wallet, models, chat, health, stats, doctor
programs/
  escrow/      Anchor escrow program (Phase 4 scaffold)
               deposit / claim / refund instructions + LiteSVM unit tests
               Build with: anchor build (requires Anchor CLI + Solana toolchain)
sdks/
  python/      pip install rustyclawrouter   (63 tests)
  typescript/  npm install @rustyclawrouter/sdk  (19 tests)
  go/          go get github.com/rustyclawrouter/sdk-go  (18 tests)
  mcp/         npx @rustyclawrouter/mcp  (17 tests, Claude Code integration)
config/
  models.toml     Model registry + pricing
  default.toml    Gateway configuration
  services.toml   x402 service marketplace registry (Phase 6)
```

## Quick Start

### Prerequisites

- Rust 1.75+ (2021 edition)
- Docker & Docker Compose (for Redis + PostgreSQL)
- At least one LLM provider API key

### Setup

```bash
# Clone and build
git clone https://github.com/sky64/RustyClawRouter.git
cd RustyClawRouter
cargo build

# Start backing services
docker compose up -d

# Configure environment
cp .env.example .env
# Edit .env with your provider API keys and Solana wallet

# Run the gateway
cargo run -p gateway
```

The gateway starts on `http://localhost:8402`.

### Using the CLI

```bash
cargo run -p cli -- wallet init       # Generate Solana keypair
cargo run -p cli -- models            # List models + pricing
cargo run -p cli -- chat "Hello!"     # Chat with auto-routing
cargo run -p cli -- health            # Gateway health check
cargo run -p cli -- doctor            # Diagnostics
```

### Using the SDKs

**Python**
```python
from rustyclawrouter import LLMClient

client = LLMClient()  # Uses SOLANA_WALLET_KEY env var
response = client.chat("openai/gpt-4o", "Hello!")
```

**TypeScript**
```typescript
import { LLMClient } from '@rustyclawrouter/sdk';

// Real Solana signing when SOLANA_WALLET_KEY + SOLANA_RPC_URL are set
// Gracefully stubs when @solana/web3.js is not installed
const client = new LLMClient();
const response = await client.chat('openai/gpt-4o', 'Hello!');
```

**Go**
```go
client, _ := rustyclawrouter.NewClient()
response, _ := client.Chat(ctx, "openai/gpt-4o", "Hello!")
```

**MCP (Claude Code)**
```bash
# Add to your Claude Code MCP config:
# command: node /path/to/sdks/mcp/dist/index.js
# Tools: chat, smart_chat, wallet_status, list_models, spending
```

## API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| `POST` | `/v1/chat/completions` | OpenAI-compatible LLM chat (x402 paid) |
| `POST` | `/v1/images/generations` | Image generation scaffold (501 until provider added) |
| `GET`  | `/v1/models` | List available models with pricing |
| `GET`  | `/v1/supported` | x402 ecosystem payment method discovery |
| `GET`  | `/pricing` | Detailed per-model USDC cost breakdown with examples |
| `GET`  | `/health` | Gateway health check |

### Example: Check pricing

```bash
curl http://localhost:8402/pricing | jq '.models[0]'
# {
#   "id": "openai/gpt-4o",
#   "pricing": { "input_per_million_usdc": 2.5, "platform_fee_percent": 5 },
#   "example_1k_token_request": { "total_usdc": "0.006563" }
# }
```

## Build & Test

```bash
# Build
cargo build                  # Debug
cargo build --release        # Release

# Test (125 Rust tests)
cargo test                   # All workspace tests
cargo test -p gateway        # Gateway (56 unit + 15 integration)
cargo test -p x402           # x402 protocol (39 tests)
cargo test -p router         # Smart router (13 tests)

# Lint
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings

# Escrow program (standalone — not part of workspace)
cd programs/escrow
OPENSSL_NO_PKG_CONFIG=1 \
  OPENSSL_LIB_DIR=/usr/lib/x86_64-linux-gnu \
  OPENSSL_INCLUDE_DIR=/usr/include/openssl \
  cargo test   # 6 unit tests (PDA derivation, struct layout)
```

### SDK Tests

```bash
# Python (63 tests)
cd sdks/python && python -m pytest

# TypeScript (19 tests)
cd sdks/typescript && npm test

# Go (18 tests)
cd sdks/go && go test ./...

# MCP TypeScript (17 tests)
cd sdks/mcp && npm test
```

## Supported Models

| Provider   | Models                                           |
|------------|--------------------------------------------------|
| OpenAI     | gpt-5.2, gpt-4o, gpt-4o-mini, o3, gpt-oss-120b |
| Anthropic  | claude-opus-4, claude-sonnet-4, claude-haiku-4  |
| Google     | gemini-2.5-pro, gemini-2.5-flash                |
| xAI        | grok-3, grok-3-mini                              |
| DeepSeek   | deepseek-r2, deepseek-v3                         |

All prices include a 5% platform fee on top of provider cost. Run `GET /pricing` for full breakdown or see `config/models.toml`.

## Smart Router

The smart router scores requests across 15 dimensions and picks the best model automatically:

- **Profiles**: `speed` (cheapest fast model), `balanced`, `quality` (best available), `reasoning`
- **Aliases**: `auto`, `fast`, `cheap`, `smart`, `best`, `reason`, `code`, `creative`, `analyze`, `eco`

```bash
# Auto-route based on request complexity
curl -X POST http://localhost:8402/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model": "auto", "messages": [{"role": "user", "content": "Hello"}]}'
```

## Anchor Escrow Program (Phase 4)

Located in `programs/escrow/`. Trustless USDC-SPL escrow for production payment settlement:

```
Agent → deposit(max_amount, service_id, expiry_slot)  →  USDC locked in PDA vault
Gateway delivers LLM response
Gateway → claim(actual_cost)  →  actual_cost to provider, refund remainder to agent
OR (if timed out):
Agent → refund()  →  full deposit returned after expiry_slot
```

To build and deploy (requires Anchor CLI + Solana toolchain):
```bash
cd programs/escrow
anchor build            # Compiles to .so + generates IDL
anchor deploy           # Deploy to localnet/devnet
```

## Security

- **Payment verification**: ed25519 signature + ATA derivation + TransferChecked discriminator enforcement
- **Replay prevention**: Redis `SET NX EX 120` on tx signature
- **Prompt guard**: injection, jailbreak, and PII detection middleware
- **Rate limiting**: per-wallet (pubkey), not spoofable via `X-Forwarded-For`
- **CORS**: explicit allowlist, no wildcard
- **Private keys**: zeroed in memory after signing (TypeScript SDK); never logged

## Environment Variables

See [`.env.example`](.env.example) for all configuration options.

| Variable | Description |
|----------|-------------|
| `RCR_SERVER_PORT` | Gateway port (default: 8402) |
| `RCR_SOLANA_RPC_URL` | Solana RPC endpoint |
| `RCR_SOLANA_RECIPIENT_WALLET` | Payment destination wallet |
| `RCR_CORS_ORIGINS` | Comma-separated allowed CORS origins |
| `OPENAI_API_KEY` | OpenAI provider key |
| `ANTHROPIC_API_KEY` | Anthropic provider key |
| `GOOGLE_API_KEY` | Google/Gemini provider key |
| `XAI_API_KEY` | xAI/Grok provider key |
| `DEEPSEEK_API_KEY` | DeepSeek provider key |
| `DATABASE_URL` | PostgreSQL connection string |
| `REDIS_URL` | Redis connection string |
| `SOLANA_RPC_URL` | RPC URL for SDK tx signing |

## License

MIT
