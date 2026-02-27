# RustyClawRouter

Solana-native AI agent payment infrastructure. AI agents pay for LLM API calls with USDC-SPL on Solana via the [x402 protocol](https://www.x402.org/). No API keys, no accounts, just wallets.

## How It Works

```
Agent → POST /v1/chat/completions → 402 Payment Required (price quote)
Agent signs USDC-SPL transfer on Solana
Agent → POST /v1/chat/completions + X-PAYMENT header → 200 OK (LLM response)
```

An AI agent requests an LLM API call, receives an HTTP 402 with the USDC price, signs a Solana transaction paying that amount, and retries with the signed payment attached. The gateway verifies the payment on-chain, proxies to the LLM provider, and returns the response.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│  CLIENT LAYER                                               │
│  Python SDK · TypeScript SDK · Go SDK · Rust CLI            │
└────────────────────────┬────────────────────────────────────┘
                         │ HTTPS + X-PAYMENT header (x402)
                         ▼
┌─────────────────────────────────────────────────────────────┐
│  RUSTYCLAWROUTER GATEWAY (Rust / Axum)                      │
│                                                             │
│  x402 Payment Middleware    Smart Router (15-dim scorer)     │
│  Response Cache (Redis)     Provider Fallback + Circuit Breaker │
│  Usage Tracker (PostgreSQL) Rate Limiter (per-wallet/IP)    │
│                                                             │
│  Provider Adapters: OpenAI · Anthropic · Google · xAI · DeepSeek │
│                                                             │
│  POST /v1/chat/completions   GET /v1/models                 │
│  GET /health                 GET /pricing                   │
└────────────────────────┬────────────────────────────────────┘
                         │ on-chain settlement
                         ▼
┌─────────────────────────────────────────────────────────────┐
│  SOLANA (devnet / mainnet)                                  │
│  USDC-SPL TransferChecked · Anchor Escrow (planned)         │
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
  escrow/      Anchor escrow program (Phase 4 — planned)
sdks/
  python/      pip install rustyclawrouter
  typescript/  npm install @rustyclawrouter/sdk
  go/          go get github.com/rustyclawrouter/sdk-go
config/
  models.toml     Model registry + pricing (15 models, 5 providers)
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

const client = new LLMClient();
const response = await client.chat('openai/gpt-4o', 'Hello!');
```

**Go**
```go
client, _ := rustyclawrouter.NewClient()
response, _ := client.Chat(ctx, "openai/gpt-4o", "Hello!")
```

## Build & Test

```bash
# Build
cargo build                  # Debug
cargo build --release        # Release

# Test
cargo test                   # All 105 Rust tests
cargo test -p gateway        # Gateway (39 unit + 12 integration)
cargo test -p x402           # x402 protocol (39 tests)
cargo test -p router         # Smart router (13 tests)

# Lint
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
```

### SDK Tests

```bash
# Python (63 tests)
cd sdks/python && python -m pytest

# TypeScript (19 tests)
cd sdks/typescript && npm test

# Go (12 tests)
cd sdks/go && go test ./...
```

## Supported Models

| Provider   | Models                                      |
|------------|---------------------------------------------|
| OpenAI     | gpt-4o, gpt-4o-mini, o1, o1-mini           |
| Anthropic  | claude-sonnet-4-20250514, claude-3.5-haiku              |
| Google     | gemini-2.0-flash, gemini-2.0-pro           |
| xAI        | grok-3, grok-3-mini                         |
| DeepSeek   | deepseek-chat, deepseek-reasoner            |

All prices include a 5% platform fee. See `config/models.toml` for full pricing.

## Smart Router

The smart router scores requests across 15 dimensions and picks the best model automatically:

- **Profiles**: `speed` (cheapest fast model), `balanced`, `quality` (best available), `reasoning` (chain-of-thought)
- **Aliases**: `auto`, `fast`, `cheap`, `smart`, `best`, `reason`, `code`, `creative`, `analyze`, `eco`

```bash
# Auto-route based on request complexity
curl -X POST http://localhost:8402/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{"model": "auto", "messages": [{"role": "user", "content": "Hello"}]}'
```

## Environment Variables

See [`.env.example`](.env.example) for all configuration options.

| Variable | Description |
|----------|-------------|
| `RCR_SERVER_PORT` | Gateway port (default: 8402) |
| `RCR_SOLANA_RPC_URL` | Solana RPC endpoint |
| `RCR_SOLANA_RECIPIENT_WALLET` | Payment destination wallet |
| `OPENAI_API_KEY` | OpenAI provider key |
| `ANTHROPIC_API_KEY` | Anthropic provider key |
| `GOOGLE_API_KEY` | Google/Gemini provider key |
| `DATABASE_URL` | PostgreSQL connection string |
| `REDIS_URL` | Redis connection string |

## License

MIT
