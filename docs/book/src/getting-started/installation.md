# Installation

## Prerequisites

| Requirement | Version | Notes |
|-------------|---------|-------|
| Rust | 1.85+ | Edition 2021, `resolver = "2"` |
| Docker | 20.10+ | For PostgreSQL and Redis |
| Docker Compose | v2+ | Bundled with Docker Desktop |

Optional:

| Tool | Purpose |
|------|---------|
| Anchor CLI | Building/deploying the escrow program |
| `solana-keygen` | Generating Solana wallets |

## Clone and Build

```bash
git clone https://github.com/sky64/RustyClawRouter.git
cd RustyClawRouter
cargo build
```

For a release build:

```bash
cargo build --release
```

The release binary is located at `target/release/rustyclawrouter`.

## Start Backing Services

PostgreSQL (spend logging, wallet budgets) and Redis (response cache, replay protection) are both optional. The gateway degrades gracefully without them:

- Without PostgreSQL: spend events log to stdout only
- Without Redis: every request hits the upstream provider; replay protection uses an in-memory LRU fallback

To start both:

```bash
docker compose up -d
```

This starts:

- **PostgreSQL 16** on `127.0.0.1:5432` (user: `rcr`, db: `rustyclawrouter`)
- **Redis 7** on `127.0.0.1:6379` (256MB LRU eviction, no persistence)

Migrations in `migrations/` run automatically on first start and are idempotent (`CREATE TABLE IF NOT EXISTS`).

## Configure Environment

```bash
cp .env.example .env
```

At minimum, set one LLM provider API key:

```bash
# .env
OPENAI_API_KEY=sk-...
# or
ANTHROPIC_API_KEY=sk-ant-...
```

See [Configuration](./configuration.md) for the full environment variable reference.

## Verify

```bash
RUST_LOG=info cargo run -p gateway
```

The gateway starts on `http://localhost:8402`. Verify it is running:

```bash
curl http://localhost:8402/health
```

Expected response:

```json
{"status": "ok"}
```

## Run Tests

```bash
# All workspace tests (304 total)
cargo test

# Individual crates
cargo test -p gateway             # 199 tests (161 unit + 38 integration)
cargo test -p x402                # 74 tests
cargo test -p router              # 13 tests
cargo test -p rustyclaw-protocol  # 18 tests

# Lint (must pass before committing)
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings
```

The escrow program is not a workspace member. Test it separately:

```bash
cargo test --manifest-path programs/escrow/Cargo.toml
```
