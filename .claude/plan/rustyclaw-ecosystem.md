# Implementation Plan: RustyClawClient Ecosystem

> Two products, one protocol. RustyClawRouter is the gateway (server). RustyClawClient is the client.
> Together they form a self-sovereign, Solana-native AI agent payment stack.

---

## Products

| Product | Role | Analogy |
|---------|------|---------|
| **RustyClawRouter** | Gateway server — verifies payments, routes to LLM providers, settles on-chain | BlockRun's API backend |
| **RustyClawClient** | Client library/sidecar — holds wallet, signs payments, makes LLM calls transparent | ClawRouter (the npm package) |
| **rustyclaw-protocol** | Shared wire format — x402 types used by both client and server | The contract between them |

```
┌──────────────────────────────────────────────────────────┐
│  YOUR APP (Python, Rust, Go, TS, or any HTTP client)     │
│                                                          │
│  Uses RustyClawClient library or localhost proxy           │
└────────────────────────┬─────────────────────────────────┘
                         │  OpenAI-compatible API call
                         ▼
┌──────────────────────────────────────────────────────────┐
│  RUSTYCLAWCLIENT                                         │
│                                                          │
│  1. Sends request → gets 402 + cost                      │
│  2. Builds Solana USDC transfer tx (exact or escrow)     │
│  3. Signs with local wallet                              │
│  4. Resends with PAYMENT-SIGNATURE header                │
│  5. Returns response to app                              │
│                                                          │
│  Also: session sticking, response caching, balance       │
│  monitoring, degraded response detection, free fallback  │
└────────────────────────┬─────────────────────────────────┘
                         │  HTTPS + PAYMENT-SIGNATURE (x402 v2)
                         ▼
┌──────────────────────────────────────────────────────────┐
│  RUSTYCLAWROUTER (Gateway)                               │
│                                                          │
│  1. Decodes payment header                               │
│  2. Replay protection (Redis)                            │
│  3. Verifies + settles on Solana (Facilitator)           │
│  4. Smart routes request (15-dim scorer)                 │
│  5. Proxies to LLM provider (with fallback)              │
│  6. Claims escrow (if escrow scheme)                     │
│  7. Logs usage (PostgreSQL)                              │
│  8. Returns response (JSON or SSE stream)                │
└──────────────────────────────────────────────────────────┘
                         │
                         ▼
                    LLM Providers (OpenAI, Anthropic, Google, xAI, DeepSeek)
                         │
                         ▼
                    Solana Mainnet (USDC-SPL settlement)
```

---

## Prerequisite: Naming Cleanup

The existing "rustyclaw" trading platform project needs to be renamed to avoid confusion.
Pick a new name for the trading platform and rename its directory/repo before starting
RustyClawClient development.

**Action items:**
- [ ] Rename the trading platform project (repo, directory, references)
- [ ] Reserve `rustyclawclient` name for the client project (crates.io, GitHub, npm, PyPI)

---

## Phase A: Extract Shared Protocol Crate

> **Goal**: Publish `rustyclaw-protocol` so both client and gateway depend on the same wire format.

**What moves out of `x402/src/types.rs`:**
- `PaymentRequired`, `PaymentAccept`, `Resource`
- `PaymentPayload`, `PayloadData`, `SolanaPayload`, `EscrowPayload`
- `VerificationResult`, `SettlementResult`
- `CostBreakdown` (from `rcr-common`)
- Constants: `X402_VERSION`, `USDC_MINT`, `SOLANA_NETWORK`, `MAX_TIMEOUT_SECONDS`

**What stays in RustyClawRouter's `x402` crate:**
- `traits.rs` (PaymentVerifier — server-side only)
- `solana.rs` (on-chain verification — server-side only)
- `facilitator.rs` (settlement orchestration)
- `fee_payer.rs`, `nonce_pool.rs` (server infrastructure)
- `escrow/` (server-side claim/verify)

**Structure:**
```
rustyclaw-protocol/
├── Cargo.toml            # [package] name = "rustyclaw-protocol"
└── src/
    ├── lib.rs
    ├── types.rs           # PaymentRequired, PaymentPayload, PayloadData, etc.
    ├── cost.rs            # CostBreakdown, usdc_atomic_amount(), fee calculation
    └── constants.rs       # USDC_MINT, SOLANA_NETWORK, X402_VERSION
```

**Dependencies**: `serde`, `serde_json` only. No crypto, no Solana SDK, no HTTP framework.
Publish to crates.io. Both RustyClawRouter and RustyClawClient depend on it.

**Decision**: This crate can live in the RustyClawRouter workspace initially (as `crates/protocol/`)
and be published from there. It doesn't need its own repo yet.

---

## Phase B: RustyClawClient — Core Library

> **Goal**: Rust library that any project can add as a dependency to make paid LLM calls.

### B.1: Project Scaffolding

```
RustyClawClient/                     # New repo: github.com/<you>/RustyClawClient
├── Cargo.toml                       # Workspace root
├── crates/
│   ├── rustyclaw-client/            # Core client library
│   │   ├── Cargo.toml               # depends on: rustyclaw-protocol, solana-sdk, reqwest
│   │   └── src/
│   │       ├── lib.rs               # Public API surface
│   │       ├── client.rs            # RustyClawClient — the main entry point
│   │       ├── wallet.rs            # Wallet create, import (BIP39), export, balance check
│   │       ├── signer.rs            # Build + sign USDC-SPL transfer tx
│   │       ├── config.rs            # Gateway URL, wallet source, timeouts, defaults
│   │       └── error.rs             # Client error enum (thiserror)
│   │
│   ├── rustyclawclient-proxy/        # Local sidecar HTTP proxy (Phase C)
│   └── rustyclawclient-cli/         # User CLI (Phase D)
│
├── config/
│   └── default.toml                 # Default gateway URL, profile, timeouts
│
└── README.md
```

**Key workspace dependencies:**
```toml
[workspace.dependencies]
rustyclaw-protocol = "0.1"           # Shared x402 wire format
solana-sdk = "2.2"
solana-client = "2.2"
spl-token = "7"
spl-associated-token-account = "5"
reqwest = { version = "0.12", features = ["json", "stream"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
base64 = "0.22"
bs58 = "0.5"
ed25519-dalek = "2"
bip39 = "2"
tokio = { version = "1", features = ["full"] }
tracing = "0.1"
thiserror = "2"
```

### B.2: Wallet Management (`wallet.rs`)

The wallet is the product. Non-custodial, local, user controls the key.

```rust
pub struct Wallet {
    signing_key: SigningKey,    // Ed25519 (zeroed on Drop)
    pubkey: Pubkey,             // Solana address
}

impl Wallet {
    /// Create a new random wallet with BIP39 mnemonic backup.
    pub fn create() -> (Self, Mnemonic);

    /// Import from BIP39 mnemonic phrase.
    pub fn from_mnemonic(mnemonic: &str) -> Result<Self, WalletError>;

    /// Import from base58-encoded 64-byte keypair (Solana CLI format).
    pub fn from_keypair_b58(b58: &str) -> Result<Self, WalletError>;

    /// Import from environment variable.
    pub fn from_env(var: &str) -> Result<Self, WalletError>;

    /// Solana public key (base58).
    pub fn address(&self) -> String;

    /// Check USDC-SPL balance via RPC.
    pub async fn usdc_balance(&self, rpc_url: &str) -> Result<f64, WalletError>;

    /// Sign a message (used internally by signer.rs).
    pub(crate) fn sign(&self, message: &[u8]) -> Signature;
}

impl Drop for Wallet {
    fn drop(&mut self) { /* zero signing key bytes */ }
}

impl Debug for Wallet {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        write!(f, "Wallet({})", self.address())  // Never print key
    }
}
```

**Security rules:**
- Private key never leaves the `Wallet` struct
- `Drop` zeros key material
- `Debug` prints only the public address
- No `Serialize` implementation — keys are never serialized by the library

### B.3: Payment Signer (`signer.rs`)

Builds and signs Solana transactions for x402 payments.

```rust
/// Build a signed USDC-SPL transfer transaction.
///
/// Creates a versioned transaction with:
/// - SPL Token TransferChecked instruction
/// - Recent blockhash (or durable nonce if gateway provides one)
/// - Wallet signature
pub async fn sign_exact_payment(
    wallet: &Wallet,
    rpc_url: &str,
    recipient: &str,       // Gateway's recipient wallet (from 402 response)
    amount_atomic: u64,    // USDC atomic units (from 402 response)
) -> Result<String, SignerError>;   // Returns base64-encoded signed tx

/// Build a signed escrow deposit transaction.
pub async fn sign_escrow_deposit(
    wallet: &Wallet,
    rpc_url: &str,
    escrow_program_id: &str,
    recipient: &str,
    amount_atomic: u64,
    service_id: [u8; 32],
) -> Result<EscrowDepositResult, SignerError>;
```

### B.4: Client (`client.rs`)

The main entry point. Handles the 402 → sign → resend flow transparently.

```rust
pub struct RustyClawClient {
    wallet: Wallet,
    gateway_url: String,
    rpc_url: String,
    http: reqwest::Client,
    config: ClientConfig,
}

impl RustyClawClient {
    pub fn builder() -> ClientBuilder;

    /// OpenAI-compatible chat completion. Payment handled transparently.
    ///
    /// 1. POST to gateway with model + messages
    /// 2. If 402 → read cost breakdown → sign payment → resend
    /// 3. Return response
    pub async fn chat(&self, req: ChatRequest) -> Result<ChatResponse, ClientError>;

    /// Streaming chat completion. Returns an async stream of chunks.
    pub async fn chat_stream(&self, req: ChatRequest)
        -> Result<impl Stream<Item = Result<ChatChunk, ClientError>>, ClientError>;

    /// Check USDC balance.
    pub async fn balance(&self) -> Result<f64, ClientError>;

    /// List available models and pricing from gateway.
    pub async fn models(&self) -> Result<Vec<ModelInfo>, ClientError>;

    /// Get cost estimate before paying.
    pub async fn estimate_cost(&self, model: &str, input_tokens: u32, output_tokens: u32)
        -> Result<CostBreakdown, ClientError>;
}

pub struct ClientBuilder {
    gateway_url: Option<String>,     // default: "http://localhost:8402"
    rpc_url: Option<String>,         // default: Solana mainnet
    wallet: Option<Wallet>,
    prefer_escrow: bool,             // default: true (safer for the agent)
    timeout: Duration,               // default: 180s
}
```

**The 402 handshake (inside `chat()`):**
```
1. POST /v1/chat/completions (no payment header)
2. Gateway returns 402 + PaymentRequired { accepts, cost_breakdown }
3. Client picks scheme (exact or escrow based on config + availability)
4. Client builds + signs Solana tx
5. Client encodes PaymentPayload as base64 JSON
6. POST /v1/chat/completions + PAYMENT-SIGNATURE header
7. Gateway verifies, proxies, returns 200
8. Client returns ChatResponse to caller
```

### B.5: Error Types (`error.rs`)

```rust
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("wallet error: {0}")]
    Wallet(#[from] WalletError),

    #[error("insufficient USDC balance: have {have}, need {need}")]
    InsufficientBalance { have: f64, need: f64 },

    #[error("payment signing failed: {0}")]
    Signing(#[from] SignerError),

    #[error("gateway error ({status}): {message}")]
    Gateway { status: u16, message: String },

    #[error("payment rejected by gateway: {0}")]
    PaymentRejected(String),

    #[error("model not found: {0}")]
    ModelNotFound(String),

    #[error("request timeout after {0:?}")]
    Timeout(Duration),

    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),

    #[error("stream interrupted: {0}")]
    StreamError(String),
}
```

---

## Phase C: RustyClawClient — Local Proxy Sidecar

> **Goal**: HTTP proxy on localhost so any language/tool can use RustyClawClient without an SDK.

This is the ClawRouter equivalent — `rustyclawclient-proxy` listens on `localhost:8402`,
intercepts OpenAI-compatible requests, signs payments, forwards to the gateway.

```
RustyClawClient/crates/rustyclawclient-proxy/
├── Cargo.toml                # depends on: rustyclaw-client, axum (lightweight)
└── src/
    ├── main.rs               # CLI args, start server
    └── proxy.rs              # Catch-all handler: intercept → sign → forward
```

**How it works:**
```
Any HTTP client → http://localhost:8402/v1/chat/completions
                → rustyclaw-proxy intercepts
                → Uses RustyClawClient internally (402 → sign → resend)
                → Returns response to caller

The caller never sees the 402 or the payment. It just works.
```

**Config:**
```bash
rustyclawclient-proxy \
  --gateway https://my-gateway.fly.dev \
  --wallet-env RUSTYCLAWCLIENT_WALLET_KEY \  # or --wallet-file ~/.rustyclawclient/wallet.json
  --port 8402 \
  --profile auto                          # default routing profile
```

**Why this matters:**
Any tool that speaks OpenAI API (LangChain, AutoGPT, CrewAI, Claude Code, etc.)
can point at `localhost:8402` and get Solana-paid LLM access with zero code changes.

---

## Phase D: RustyClawClient — CLI

> **Goal**: User-facing command-line tool for wallet management, chat, and diagnostics.

```
RustyClawClient/crates/rustyclawclient-cli/
├── Cargo.toml               # depends on: rustyclaw-client, clap
└── src/
    ├── main.rs
    └── commands/
        ├── mod.rs
        ├── wallet.rs         # create, import, export, balance
        ├── chat.rs           # Interactive chat with payment
        ├── models.rs         # List models + pricing from gateway
        ├── stats.rs          # Spending history (from gateway /stats endpoint)
        └── doctor.rs         # Connectivity + config diagnostics
```

**Commands:**
```bash
rustyclawclient wallet create              # Generate new wallet, show mnemonic
rustyclawclient wallet import              # Import from mnemonic or keypair
rustyclawclient wallet balance             # Show USDC-SPL balance
rustyclawclient wallet address             # Show public address
rustyclawclient wallet export              # Export keypair (with confirmation)

rustyclawclient chat "Explain quicksort"   # One-shot chat (auto profile)
rustyclawclient chat -m gpt-4o "Hello"     # Specific model
rustyclawclient chat -p eco "Hello"        # Specific profile
rustyclawclient chat --stream "Hello"      # Streaming output

rustyclawclient models                     # List models + pricing
rustyclawclient models --provider openai   # Filter by provider

rustyclawclient stats                      # Spending summary
rustyclawclient stats --days 7             # Last 7 days

rustyclawclient doctor                     # Check: wallet, gateway, Solana RPC, balance
```

---

## Phase E: RustyClawClient — Smart Features

> **Goal**: Feature parity with ClawRouter's battle-tested UX, then surpass it.

### E.1: Session Sticking

Same conversation → same model. Prevents mid-conversation model switches.

```rust
// In rustyclaw-client/src/session.rs
pub struct SessionStore {
    sessions: DashMap<String, SessionEntry>,
}

struct SessionEntry {
    model: String,
    tier: Tier,
    created: Instant,
    request_count: u32,
    recent_hashes: VecDeque<u64>,  // For repetition detection
}
```

- Session ID from `x-session-id` header, or derived from first user message hash
- Three-strike escalation: 3 identical request hashes → bump tier
- TTL: 30 minutes (configurable)
- Cleanup: background task every 5 minutes

### E.2: Client-Side Response Cache

Prevent double-paying for identical requests (agent retries, OpenClaw duplicates).

```rust
// In rustyclaw-client/src/cache.rs
pub struct ResponseCache {
    entries: DashMap<u64, CacheEntry>,  // hash → response
    config: CacheConfig,
}
```

- Key: SHA256(model + messages normalized — strip timestamps, request IDs)
- TTL: 10 minutes default
- Max entries: 200
- Dedup window: 30 seconds (catches retries)
- Skip: streaming requests, cache-control: no-cache

### E.3: Balance Monitoring

```rust
// In rustyclaw-client/src/balance.rs
pub struct BalanceMonitor { ... }

impl BalanceMonitor {
    /// Start background polling. Calls `on_low_balance` when below threshold.
    pub fn start(
        wallet: &Wallet,
        rpc_url: &str,
        threshold_usdc: f64,
        on_low_balance: impl Fn(f64) + Send + 'static,
    ) -> Self;
}
```

- Polls every 30 seconds
- Fires callback when balance drops below threshold
- Gateway pre-check: if balance < estimated cost, don't even send the request

### E.4: Degraded Response Detection

Detect when the LLM returns garbage and auto-retry with a different model.

```rust
// In rustyclaw-client/src/quality.rs
pub fn is_degraded(response: &ChatResponse) -> Option<DegradedReason> {
    // Check for:
    // - Repetitive loops (same sentence repeated 3+ times)
    // - Overloaded placeholders ("I'm unable to process", etc.)
    // - Empty or near-empty content
    // - Truncated mid-word (provider timeout)
    // - Known error phrases baked into response text
}
```

- If degraded: retry with next model in tier, up to 3 attempts
- Log the degraded response for future pattern matching

### E.5: Free Tier Fallback

When wallet is empty, fall back to a free model instead of failing.

```rust
pub struct ClientConfig {
    /// Model to use when wallet balance is zero. None = fail with error.
    pub free_fallback_model: Option<String>,  // e.g., "nvidia/gpt-oss-120b"
}
```

- Before signing payment, check balance
- If zero (or below minimum), use free model if configured
- Log a warning so the user knows they're on the free tier

### E.6: SSE Heartbeat (proxy only)

In `rustyclawclient-proxy`, send heartbeat comments during streaming to prevent client timeouts.

```rust
// In proxy.rs, during streaming:
// Send `: heartbeat\n\n` every 2 seconds while waiting for first chunk
// This keeps the connection alive for slow models (reasoning, complex prompts)
```

---

## Phase F: SDKs (Non-Rust Languages)

> **Goal**: Python, TypeScript, Go SDKs that wrap the same payment flow.

These SDKs move from RustyClawRouter to RustyClawClient. They are client libraries.

### F.1: Python SDK

```
RustyClawClient/sdks/python/
├── pyproject.toml              # name = "rustyclawclient"
├── rustyclawclient/
│   ├── __init__.py             # from .client import RustyClawClient
│   ├── client.py               # Main client — chat(), chat_stream(), balance()
│   ├── wallet.py               # Wallet create/import/export/balance
│   ├── signer.py               # Build + sign Solana USDC transfer
│   ├── session.py              # Session sticking
│   ├── cache.py                # Response dedup
│   ├── types.py                # Pydantic models matching rustyclaw-protocol
│   └── config.py               # Gateway URL, defaults
└── tests/
```

**Usage:**
```python
from rustyclawclient import RustyClawClient

client = RustyClawClient(
    gateway="https://my-gateway.fly.dev",
    wallet_key=os.environ["RUSTYCLAWCLIENT_WALLET_KEY"],
)

response = client.chat(model="auto", messages=[
    {"role": "user", "content": "Explain quicksort"}
])
print(response.choices[0].message.content)
print(f"Cost: ${response.cost_breakdown.total} USDC")
```

**Dependencies**: `solders`, `solana-py`, `httpx`, `pydantic`, `bip39`

### F.2: TypeScript SDK

```
RustyClawClient/sdks/typescript/
├── package.json                # name = "@rustyclawclient/sdk"
├── src/
│   ├── index.ts
│   ├── client.ts               # RustyClawClient
│   ├── wallet.ts               # Wallet management
│   ├── signer.ts               # Solana tx signing
│   ├── session.ts              # Session sticking
│   ├── cache.ts                # Response dedup
│   ├── types.ts                # TypeScript interfaces matching protocol
│   └── openai-compat.ts        # Drop-in OpenAI SDK replacement
└── tests/
```

**OpenAI drop-in:**
```typescript
import { OpenAI } from "@rustyclawclient/sdk";

// Same API as `openai` npm package — just swap the import
const client = new OpenAI({
  walletKey: process.env.RUSTYCLAWCLIENT_WALLET_KEY,
  gateway: "https://my-gateway.fly.dev",
});

const response = await client.chat.completions.create({
  model: "auto",
  messages: [{ role: "user", content: "Hello" }],
});
```

**Dependencies**: `@solana/web3.js`, `@solana/spl-token`, `bs58`, `bip39`

### F.3: Go SDK

```
RustyClawClient/sdks/go/
├── go.mod                      # module github.com/<you>/rustyclawclient-go
├── client.go                   # Client struct + Chat(), ChatStream(), Balance()
├── wallet.go                   # Wallet management
├── signer.go                   # Solana tx signing
├── session.go                  # Session sticking
├── cache.go                    # Response dedup
├── types.go                    # Structs matching protocol
├── config.go                   # Options pattern
└── client_test.go
```

**Dependencies**: `github.com/gagliardetto/solana-go`, stdlib `net/http`

---

## Phase G: Gateway Changes (RustyClawRouter)

> **Goal**: Modifications to the gateway to support RustyClawClient features.

These are small, targeted changes — the gateway architecture doesn't change.

### G.1: Session ID Support

- Read `x-session-id` header in `routes/chat.rs`
- If present, include in response headers so client can track
- Router can use session context for model sticking (optional server-side assist)
- **Not required for client-side session sticking** — client handles this itself

### G.2: Debug Response Headers

Return routing diagnostics so clients and debugging tools can see what happened.

```
X-RCR-Model: anthropic/claude-sonnet-4.6
X-RCR-Tier: Complex
X-RCR-Score: 0.4237
X-RCR-Profile: auto
X-RCR-Provider: anthropic
X-RCR-Cache: miss
X-RCR-Latency-Ms: 1847
```

Only when `X-RCR-Debug: true` request header is set.

### G.3: SSE Heartbeat

Send `: heartbeat\n\n` comment events during streaming before first data chunk.
Prevents client-side timeout for slow providers (reasoning models).

### G.4: Nonce Endpoint Enhancement

`GET /v1/nonce` already exists. Ensure it returns a durable nonce the client can use
for transaction construction, avoiding blockhash expiry on slow connections.

### G.5: Stats Endpoint

`GET /v1/stats?wallet=<address>` — returns spending summary for a wallet.
The CLI and SDKs use this to show the user their spend history.

---

## Build Order

```
Phase A:  Extract rustyclaw-protocol crate (from RustyClawRouter's x402/types.rs)
          ↓
Phase B:  RustyClawClient core library (wallet → signer → client)
          Depends on: rustyclaw-protocol published
          ↓
Phase C:  Local proxy sidecar (rustyclawclient-proxy)
          Depends on: Phase B core library working
          ↓
Phase D:  CLI (rustyclawclient-cli)
          Depends on: Phase B core library working
          Can parallelize with Phase C
          ↓
Phase E:  Smart features (session, cache, balance, degraded detection, free fallback)
          Depends on: Phase B core working + real usage feedback
          ↓
Phase F:  SDKs (Python, TypeScript, Go)
          Depends on: Phase B proving the API surface is right
          Can start once Phase B API is stable
          ↓
Phase G:  Gateway changes (session headers, debug headers, SSE heartbeat, stats)
          Can parallelize with Phases C-F
          Small changes, no architectural risk
```

**What can run in parallel:**
- Phase C + D (proxy and CLI are independent, both depend on core)
- Phase F + G (SDKs and gateway changes are independent)
- Phase E features are incremental — add them one at a time as needed

---

## What This Gives You Over ClawRouter

| Advantage | Why |
|-----------|-----|
| No middleman | Client signs → Gateway verifies → Solana settles. No BlockRun dependency. |
| Escrow | Agent deposits, gateway claims actual cost, overage refunded. ClawRouter can't do this. |
| Rust performance | Client-side signing + proxy in Rust. Gateway in Rust. Sub-µs routing. |
| Multi-SDK | Same protocol, any language. ClawRouter is npm-only. |
| Self-hostable | Run your own gateway. No vendor dependency. |
| On-chain transparency | Every payment is a verifiable Solana transaction. |

## What ClawRouter Still Does Better (Until You Build It)

| Feature | Status | Phase |
|---------|--------|-------|
| Session sticking | Not built yet | E.1 |
| Degraded response detection | Not built yet | E.4 |
| Free tier fallback | Not built yet | E.5 |
| SSE heartbeat | Not built yet | E.6 (proxy) + G.3 (gateway) |
| Zero-config DX (`npx` and go) | Requires gateway deploy | Inherent tradeoff — you own infra, they don't |
| Battle-tested provider quirks | Needs production usage | Time, not architecture |

---

## Decision Log

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Separate repos (client vs gateway) | Yes | Different deployment targets, different release cycles, different users |
| Shared protocol crate | `rustyclaw-protocol` on crates.io | Single source of truth for wire format |
| Client wallet model | Single wallet, non-custodial | "Your wallet, your keys, your money until you spend it" |
| Default payment scheme | Prefer escrow | Safer for agent — only pays actual cost, auto-refund on failure |
| Proxy as optional sidecar | Separate binary, not required | Library-first design; proxy is convenience for non-Rust users |
| SDKs are client-side only | Move from RustyClawRouter to RustyClawClient | SDKs sign payments — that's client behavior, not server behavior |
| Session sticking is client-side | Client manages sessions, not gateway | Gateway is stateless by design; session is per-agent context |
