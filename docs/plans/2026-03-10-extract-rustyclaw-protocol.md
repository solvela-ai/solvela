# Extract `rustyclaw-protocol` Crate — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Extract shared wire-format types from `x402-solana` and `rcr-common` into a single published protocol crate (`rustyclaw-protocol`), then delete both source crates.

**Architecture:** Merge all payment types (from `x402-solana`) and chat/LLM types (from `rcr-common`) into `crates/protocol/`. Update all workspace dependents to use `rustyclaw-protocol`. Move `ServiceRegistry` into `gateway`. Delete `x402-solana` and `rcr-common`.

**Tech Stack:** Rust, serde, serde_json, thiserror

---

## Context

### What exists today

- **`crates/x402-solana/`** — Payment wire types: `CostBreakdown`, `PaymentRequired`, `PaymentAccept`, `PaymentPayload`, `PayloadData`, `SolanaPayload`, `EscrowPayload`, `Resource`, `VerificationResult`, `SettlementResult`. Constants: `X402_VERSION`, `USDC_MINT`, `SOLANA_NETWORK`, `MAX_TIMEOUT_SECONDS`, `PLATFORM_FEE_MULTIPLIER`, `PLATFORM_FEE_PERCENT`. Deps: `serde`, `serde_json`, `thiserror`. Zero workspace deps.

- **`crates/common/`** (`rcr-common`) — Chat/LLM types: `Role`, `ChatMessage`, `ChatRequest`, `ChatResponse`, `ChatChoice`, `Usage`, `ChatDelta`, `ChatChunk`, `ChatChunkChoice`, `FunctionCall`, `ToolCall`, `FunctionCallDelta`, `ToolCallDelta`, `ImageUrl`, `ContentPart`, `FunctionDefinitionInner`, `ToolDefinition`, `ModelInfo`. Also: `ServiceRegistry`, `ServiceEntry`, `ServiceRegistryError`. Re-exports `CostBreakdown`, `PLATFORM_FEE_MULTIPLIER`, `PLATFORM_FEE_PERCENT` from `x402-solana`. Deps: `serde`, `serde_json`, `toml`, `thiserror`, `x402-solana`.

### Dependency graph (current)

```
x402-solana (zero workspace deps)
    ↑
rcr-common (depends on x402-solana)
    ↑               ↑
x402 (depends on rcr-common + x402-solana)
    ↑       router (depends on rcr-common)
    ↑           ↑
gateway (depends on rcr-common + x402 + router)
    ↑
cli (depends on rcr-common + x402 + router) [rcr-common listed but unused]
```

### Dependency graph (after)

```
rustyclaw-protocol (zero workspace deps)
    ↑           ↑           ↑
x402            router      gateway (also gains ServiceRegistry)
    ↑                           ↑
    ↑---------------------------↑
cli (depends on x402 + router; drops rcr-common dep)
```

### Files that import `rcr_common::*` (must change to `rustyclaw_protocol::*`)

**gateway crate:**
- `src/lib.rs:26` — `use rcr_common::services::ServiceRegistry` → `use crate::services::ServiceRegistry`
- `src/main.rs:12` — `use rcr_common::services::ServiceRegistry` → `use gateway::services::ServiceRegistry`
- `src/routes/chat.rs:11` — `use rcr_common::types::ChatRequest` → `use rustyclaw_protocol::ChatRequest`
- `src/routes/chat.rs:702` (test) — `use rcr_common::types::{ChatMessage, ModelInfo, Role}` → `use rustyclaw_protocol::{ChatMessage, ModelInfo, Role}`
- `src/routes/pricing.rs:7` — `use rcr_common::types::PLATFORM_FEE_PERCENT` → `use rustyclaw_protocol::PLATFORM_FEE_PERCENT`
- `src/providers/mod.rs:17` — `use rcr_common::types::{ChatChunk, ChatRequest, ChatResponse, ModelInfo}` → `use rustyclaw_protocol::{ChatChunk, ChatRequest, ChatResponse, ModelInfo}`
- `src/providers/openai.rs:3` — `use rcr_common::types::{ChatRequest, ChatResponse, ModelInfo}` → `use rustyclaw_protocol::{ChatRequest, ChatResponse, ModelInfo}`
- `src/providers/anthropic.rs:4` — `use rcr_common::types::{...}` → `use rustyclaw_protocol::{...}`
- `src/providers/google.rs:4` — `use rcr_common::types::{...}` → `use rustyclaw_protocol::{...}`
- `src/providers/xai.rs:3` — `use rcr_common::types::{ChatRequest, ChatResponse, ModelInfo}` → `use rustyclaw_protocol::{ChatRequest, ChatResponse, ModelInfo}`
- `src/providers/deepseek.rs:3` — `use rcr_common::types::{ChatRequest, ChatResponse, ModelInfo}` → `use rustyclaw_protocol::{ChatRequest, ChatResponse, ModelInfo}`
- `src/providers/heartbeat.rs:17` — `use rcr_common::types::ChatChunk` → `use rustyclaw_protocol::ChatChunk`
- `src/providers/heartbeat.rs:149` (test) — `use rcr_common::types::{ChatChunk, ChatChunkChoice, ChatDelta}` → `use rustyclaw_protocol::{ChatChunk, ChatChunkChoice, ChatDelta}`
- `src/providers/fallback.rs:10` — `use rcr_common::types::{ChatRequest, ChatResponse}` → `use rustyclaw_protocol::{ChatRequest, ChatResponse}`
- `src/middleware/prompt_guard.rs:13` — `use rcr_common::types::ChatMessage` → `use rustyclaw_protocol::ChatMessage`
- `src/middleware/prompt_guard.rs:272` (test) — `use rcr_common::types::Role` → `use rustyclaw_protocol::Role`
- `src/cache.rs:13` — `use rcr_common::types::{ChatRequest, ChatResponse}` → `use rustyclaw_protocol::{ChatRequest, ChatResponse}`
- `src/cache.rs:262` (test) — `use rcr_common::types::{ChatMessage, Role}` → `use rustyclaw_protocol::{ChatMessage, Role}`
- `tests/integration.rs:19` — `use rcr_common::services::ServiceRegistry` → `use gateway::services::ServiceRegistry`

**router crate:**
- `src/models.rs:6` — `use rcr_common::types::{CostBreakdown, ModelInfo, PLATFORM_FEE_MULTIPLIER, PLATFORM_FEE_PERCENT}` → `use rustyclaw_protocol::{CostBreakdown, ModelInfo, PLATFORM_FEE_MULTIPLIER, PLATFORM_FEE_PERCENT}`
- `src/scorer.rs:274` (test) — `use rcr_common::types::{ChatMessage, Role}` → `use rustyclaw_protocol::{ChatMessage, Role}`

**x402 crate:**
- `src/types.rs:3-4` — `pub use x402_solana::constants::*; pub use x402_solana::types::*` → `pub use rustyclaw_protocol::*` (re-exports for backward compat — but since `x402::types` now re-exports from `rustyclaw_protocol`, consumers like `cli` that use `x402::types::PaymentPayload` still work)

**cli crate:**
- `src/commands/chat.rs:3` — `use x402::types::{PaymentPayload, PaymentRequired, Resource, SolanaPayload}` → No change needed (x402 re-exports from rustyclaw-protocol)

---

## Tasks

### Task 1: Create `crates/protocol/` with Cargo.toml

**Files:**
- Create: `crates/protocol/Cargo.toml`

**Step 1: Create the Cargo.toml**

```toml
[package]
name = "rustyclaw-protocol"
version = "0.1.0"
edition = "2021"
description = "Shared wire-format types for the RustyClaw ecosystem (x402 payment protocol + OpenAI-compatible chat types)"
license = "MIT OR Apache-2.0"
repository = "https://github.com/sky64/RustyClawRouter"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
```

**Step 2: Verify directory structure**

Run: `ls crates/protocol/`
Expected: `Cargo.toml` exists

---

### Task 2: Create protocol crate source files (constants, cost, payment, settlement)

**Files:**
- Create: `crates/protocol/src/lib.rs`
- Create: `crates/protocol/src/constants.rs`
- Create: `crates/protocol/src/cost.rs`
- Create: `crates/protocol/src/payment.rs`
- Create: `crates/protocol/src/settlement.rs`

**Step 1: Create `src/lib.rs`**

```rust
pub mod chat;
pub mod constants;
pub mod cost;
pub mod model;
pub mod payment;
pub mod settlement;
pub mod streaming;
pub mod tools;
pub mod vision;

// Flat re-exports so consumers write:
//   use rustyclaw_protocol::{ChatRequest, PaymentRequired, CostBreakdown};
pub use chat::*;
pub use constants::*;
pub use cost::*;
pub use model::*;
pub use payment::*;
pub use settlement::*;
pub use streaming::*;
pub use tools::*;
pub use vision::*;
```

**Step 2: Create `src/constants.rs`**

Copy from `crates/x402-solana/src/constants.rs` verbatim:

```rust
/// x402 protocol version.
pub const X402_VERSION: u8 = 2;

/// USDC-SPL mint address on Solana mainnet.
pub const USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

/// Solana mainnet network identifier for x402.
pub const SOLANA_NETWORK: &str = "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp";

/// Maximum timeout for payment authorization (5 minutes).
pub const MAX_TIMEOUT_SECONDS: u64 = 300;

/// The platform fee multiplier (1.05 = provider cost + 5%).
pub const PLATFORM_FEE_MULTIPLIER: f64 = 1.05;

/// Platform fee percentage.
pub const PLATFORM_FEE_PERCENT: u8 = 5;
```

**Step 3: Create `src/cost.rs`**

Copy `CostBreakdown` from `crates/x402-solana/src/types.rs` (lines 9-21):

```rust
use serde::{Deserialize, Serialize};

/// Cost breakdown returned in 402 responses and receipts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostBreakdown {
    /// Raw provider cost in USDC.
    pub provider_cost: String,
    /// Platform fee in USDC (5%).
    pub platform_fee: String,
    /// Total cost to the agent in USDC.
    pub total: String,
    /// Always "USDC".
    pub currency: String,
    /// Platform fee percentage (5).
    pub fee_percent: u8,
}
```

**Step 4: Create `src/payment.rs`**

Copy from `crates/x402-solana/src/types.rs` (lines 27-107) — all payment types:

```rust
use serde::{Deserialize, Serialize};

use crate::cost::CostBreakdown;

/// Describes a resource that requires payment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resource {
    /// The URL path of the resource.
    pub url: String,
    /// HTTP method.
    pub method: String,
}

/// Describes an accepted payment method.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentAccept {
    /// Payment scheme (e.g., "exact", "escrow").
    pub scheme: String,
    /// Network identifier (e.g., "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp").
    pub network: String,
    /// Amount in atomic units (USDC has 6 decimals).
    pub amount: String,
    /// Token mint/contract address.
    pub asset: String,
    /// Recipient wallet address.
    pub pay_to: String,
    /// Maximum seconds the payment authorization is valid.
    pub max_timeout_seconds: u64,
    /// Escrow program ID — only present for scheme="escrow".
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub escrow_program_id: Option<String>,
}

/// The full 402 Payment Required response body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentRequired {
    pub x402_version: u8,
    pub resource: Resource,
    pub accepts: Vec<PaymentAccept>,
    pub cost_breakdown: CostBreakdown,
    pub error: String,
}

/// Solana-specific payment data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolanaPayload {
    /// Base64-encoded signed versioned transaction.
    pub transaction: String,
}

/// Escrow-specific payment payload (scheme = "escrow").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EscrowPayload {
    /// Base64-encoded signed deposit transaction (Solana versioned tx).
    pub deposit_tx: String,
    /// 32-byte request correlation ID — used as escrow PDA seed.
    /// Base64-encoded.
    pub service_id: String,
    /// Agent wallet pubkey (base58) — used to derive escrow PDA.
    pub agent_pubkey: String,
}

/// Union of direct-transfer and escrow payment payloads.
/// Uses untagged deserialization — EscrowPayload is tried first (it has
/// more fields), falling back to SolanaPayload for "exact" scheme clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PayloadData {
    Escrow(EscrowPayload),
    Direct(SolanaPayload),
}

/// The payment payload sent in the `PAYMENT-SIGNATURE` header.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentPayload {
    pub x402_version: u8,
    pub resource: Resource,
    pub accepted: PaymentAccept,
    pub payload: PayloadData,
}
```

**Step 5: Create `src/settlement.rs`**

Copy from `crates/x402-solana/src/types.rs` (lines 113-140):

```rust
use serde::{Deserialize, Serialize};

/// Result of payment verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    /// Whether the payment is valid.
    pub valid: bool,
    /// Human-readable reason if invalid.
    pub reason: Option<String>,
    /// Verified amount in atomic units.
    pub verified_amount: Option<u64>,
}

/// Result of payment settlement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettlementResult {
    /// Whether settlement was successful.
    pub success: bool,
    /// Transaction signature (base58 for Solana).
    pub tx_signature: Option<String>,
    /// Network the settlement occurred on.
    pub network: String,
    /// Error message if settlement failed.
    pub error: Option<String>,
    /// Verified deposit amount in atomic units (escrow scheme only).
    /// Used to cap the claim amount so it never exceeds the deposited amount.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub verified_amount: Option<u64>,
}
```

---

### Task 3: Create protocol crate source files (chat, streaming, tools, vision, model)

**Files:**
- Create: `crates/protocol/src/chat.rs`
- Create: `crates/protocol/src/streaming.rs`
- Create: `crates/protocol/src/tools.rs`
- Create: `crates/protocol/src/vision.rs`
- Create: `crates/protocol/src/model.rs`

**Step 1: Create `src/chat.rs`**

```rust
use serde::{Deserialize, Serialize};

use crate::tools::{ToolCall, ToolDefinition};

/// Role of a message participant.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
    Developer,
}

/// A single message in a chat conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_call_id: Option<String>,
}

/// Incoming chat completion request (OpenAI-compatible).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(default)]
    pub stream: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tools: Option<Vec<ToolDefinition>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_choice: Option<serde_json::Value>,
}

/// Token usage breakdown for a completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// A single choice in a chat completion response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatChoice {
    pub index: u32,
    pub message: ChatMessage,
    pub finish_reason: Option<String>,
}

/// Chat completion response (OpenAI-compatible).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatResponse {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<ChatChoice>,
    pub usage: Option<Usage>,
}
```

**Step 2: Create `src/streaming.rs`**

```rust
use serde::{Deserialize, Serialize};

use crate::chat::Role;
use crate::tools::{FunctionCallDelta, ToolCallDelta};

/// Delta content in a streaming chunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub tool_calls: Option<Vec<ToolCallDelta>>,
}

/// A single choice in a streaming chunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatChunkChoice {
    pub index: u32,
    pub delta: ChatDelta,
    pub finish_reason: Option<String>,
}

/// Streaming chat completion chunk (OpenAI-compatible).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatChunk {
    pub id: String,
    pub object: String,
    pub created: i64,
    pub model: String,
    pub choices: Vec<ChatChunkChoice>,
}
```

**Step 3: Create `src/tools.rs`**

```rust
use serde::{Deserialize, Serialize};

/// A function call within a tool call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

/// A tool call requested by the model.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub r#type: String,
    pub function: FunctionCall,
}

/// Delta for a function call in a streaming chunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCallDelta {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub arguments: Option<String>,
}

/// Delta for a tool call in a streaming chunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallDelta {
    pub index: u32,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default, rename = "type")]
    pub r#type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub function: Option<FunctionCallDelta>,
}

/// Inner function definition within a tool definition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FunctionDefinitionInner {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub parameters: Option<serde_json::Value>,
}

/// A tool definition sent in the request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub r#type: String,
    pub function: FunctionDefinitionInner,
}
```

**Step 4: Create `src/vision.rs`**

```rust
use serde::{Deserialize, Serialize};

/// An image URL with optional detail level.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ImageUrl {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub detail: Option<String>,
}

/// A single part of multi-modal content (text or image).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String },
    ImageUrl { image_url: ImageUrl },
}
```

**Step 5: Create `src/model.rs`**

```rust
use serde::{Deserialize, Serialize};

/// Information about a supported model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub provider: String,
    pub model_id: String,
    pub display_name: String,
    pub input_cost_per_million: f64,
    pub output_cost_per_million: f64,
    pub context_window: u32,
    #[serde(default)]
    pub supports_streaming: bool,
    #[serde(default)]
    pub supports_tools: bool,
    #[serde(default)]
    pub supports_vision: bool,
    #[serde(default)]
    pub reasoning: bool,
    #[serde(default)]
    pub supports_structured_output: bool,
    #[serde(default)]
    pub supports_batch: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub max_output_tokens: Option<u32>,
}
```

**Step 6: Verify it compiles**

Run: `cargo check -p rustyclaw-protocol`
Expected: Compilation succeeds (need to add to workspace first — see Task 4)

---

### Task 4: Add protocol crate to workspace, verify it compiles

**Files:**
- Modify: `Cargo.toml` (workspace root)

**Step 1: Add `crates/protocol` to workspace members**

In root `Cargo.toml`, add `"crates/protocol"` to `[workspace.members]` and add `rustyclaw-protocol` to `[workspace.dependencies]`:

```toml
[workspace]
resolver = "2"
members = [
    "crates/gateway",
    "crates/x402",
    "crates/x402-solana",
    "crates/router",
    "crates/common",
    "crates/cli",
    "crates/protocol",
]
```

And add to `[workspace.dependencies]`:

```toml
rustyclaw-protocol = { path = "crates/protocol" }
```

**Step 2: Verify protocol crate compiles**

Run: `cargo check -p rustyclaw-protocol`
Expected: Compiles with zero errors, zero warnings

**Step 3: Commit**

```bash
git add crates/protocol/ Cargo.toml
git commit -m "$(cat <<'EOF'
feat: add rustyclaw-protocol crate with merged wire-format types

Merges payment types from x402-solana and chat/LLM types from rcr-common
into a single protocol crate. This will be the shared wire-format contract
for the RustyClaw ecosystem (gateway + future client).

Modules: constants, cost, payment, settlement, chat, streaming, tools,
vision, model. All re-exported flat from lib.rs.
EOF
)"
```

---

### Task 5: Add protocol crate tests (serde round-trips)

**Files:**
- Modify: `crates/protocol/src/payment.rs` (add tests)
- Modify: `crates/protocol/src/cost.rs` (add tests)
- Modify: `crates/protocol/src/settlement.rs` (add tests)
- Modify: `crates/protocol/src/chat.rs` (add tests)
- Modify: `crates/protocol/src/tools.rs` (add tests)
- Modify: `crates/protocol/src/vision.rs` (add tests)
- Modify: `crates/protocol/src/model.rs` (add tests)

These tests are copied from `crates/x402-solana/src/types.rs` and `crates/common/src/types.rs` with import paths updated. Each module gets its relevant tests.

**Step 1: Add tests to `payment.rs`**

Append to `crates/protocol/src/payment.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::*;
    use crate::cost::CostBreakdown;

    #[test]
    fn test_payment_required_serialization() {
        let pr = PaymentRequired {
            x402_version: X402_VERSION,
            resource: Resource {
                url: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
            },
            accepts: vec![PaymentAccept {
                scheme: "exact".to_string(),
                network: SOLANA_NETWORK.to_string(),
                amount: "2625".to_string(),
                asset: USDC_MINT.to_string(),
                pay_to: "RecipientWalletPubkeyHere".to_string(),
                max_timeout_seconds: MAX_TIMEOUT_SECONDS,
                escrow_program_id: None,
            }],
            cost_breakdown: CostBreakdown {
                provider_cost: "0.002500".to_string(),
                platform_fee: "0.000125".to_string(),
                total: "0.002625".to_string(),
                currency: "USDC".to_string(),
                fee_percent: 5,
            },
            error: "Payment required".to_string(),
        };

        let json = serde_json::to_string_pretty(&pr).unwrap();
        assert!(json.contains("x402_version"));
        assert!(json.contains("solana:"));
        assert!(json.contains("cost_breakdown"));
    }

    #[test]
    fn test_payment_accept_escrow_serialization() {
        let accept = PaymentAccept {
            scheme: "escrow".to_string(),
            network: SOLANA_NETWORK.to_string(),
            amount: "5000".to_string(),
            asset: USDC_MINT.to_string(),
            pay_to: "RecipientWalletPubkeyHere".to_string(),
            max_timeout_seconds: MAX_TIMEOUT_SECONDS,
            escrow_program_id: Some("9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU".to_string()),
        };
        let json = serde_json::to_string(&accept).unwrap();
        assert!(json.contains("escrow_program_id"));
        assert!(json.contains("9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU"));
    }

    #[test]
    fn test_payment_accept_exact_no_escrow_field() {
        let accept = PaymentAccept {
            scheme: "exact".to_string(),
            network: SOLANA_NETWORK.to_string(),
            amount: "2625".to_string(),
            asset: USDC_MINT.to_string(),
            pay_to: "RecipientWalletPubkeyHere".to_string(),
            max_timeout_seconds: MAX_TIMEOUT_SECONDS,
            escrow_program_id: None,
        };
        let json = serde_json::to_string(&accept).unwrap();
        assert!(
            !json.contains("escrow_program_id"),
            "escrow_program_id should be absent when None"
        );
    }

    #[test]
    fn test_payload_data_direct_roundtrip() {
        let direct = PayloadData::Direct(SolanaPayload {
            transaction: "dGVzdA==".to_string(),
        });
        let json = serde_json::to_string(&direct).unwrap();
        let deserialized: PayloadData = serde_json::from_str(&json).unwrap();
        match deserialized {
            PayloadData::Direct(p) => assert_eq!(p.transaction, "dGVzdA=="),
            PayloadData::Escrow(_) => panic!("expected Direct variant"),
        }
    }

    #[test]
    fn test_payload_data_escrow_roundtrip() {
        let escrow = PayloadData::Escrow(EscrowPayload {
            deposit_tx: "dGVzdA==".to_string(),
            service_id: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_string(),
            agent_pubkey: "11111111111111111111111111111111".to_string(),
        });
        let json = serde_json::to_string(&escrow).unwrap();
        let deserialized: PayloadData = serde_json::from_str(&json).unwrap();
        match deserialized {
            PayloadData::Escrow(p) => {
                assert_eq!(p.deposit_tx, "dGVzdA==");
                assert_eq!(p.agent_pubkey, "11111111111111111111111111111111");
            }
            PayloadData::Direct(_) => panic!("expected Escrow variant"),
        }
    }

    #[test]
    fn test_escrow_payload_serde_roundtrip() {
        let ep = EscrowPayload {
            deposit_tx: "abc123".to_string(),
            service_id: "c2VydmljZTEyMzQ1Njc4OTAxMjM0NTY3ODkwMTIzNA==".to_string(),
            agent_pubkey: "9noXzpXnkyEcKF3AeXqUHTdR59V5uvrRBUo9bwsHaByz".to_string(),
        };
        let json = serde_json::to_string(&ep).unwrap();
        let deserialized: EscrowPayload = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.deposit_tx, ep.deposit_tx);
        assert_eq!(deserialized.service_id, ep.service_id);
        assert_eq!(deserialized.agent_pubkey, ep.agent_pubkey);
    }
}
```

**Step 2: Add tests to `cost.rs`**

Append to `crates/protocol/src/cost.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::constants::PLATFORM_FEE_PERCENT;

    #[test]
    fn test_cost_breakdown_serialization() {
        let cost = CostBreakdown {
            provider_cost: "0.002500".to_string(),
            platform_fee: "0.000125".to_string(),
            total: "0.002625".to_string(),
            currency: "USDC".to_string(),
            fee_percent: PLATFORM_FEE_PERCENT,
        };
        let json = serde_json::to_value(&cost).unwrap();
        assert_eq!(json["fee_percent"], 5);
        assert_eq!(json["currency"], "USDC");
    }
}
```

**Step 3: Add tests to `chat.rs`**

Append to `crates/protocol/src/chat.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{FunctionCall, ToolCall};

    #[test]
    fn test_chat_request_serialization() {
        let req = ChatRequest {
            model: "openai/gpt-4o".to_string(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: "Hello!".to_string(),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            max_tokens: Some(100),
            temperature: Some(0.7),
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
        };

        let json = serde_json::to_string(&req).unwrap();
        let deser: ChatRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.model, "openai/gpt-4o");
        assert_eq!(deser.messages.len(), 1);
        assert_eq!(deser.messages[0].role, Role::User);
    }

    #[test]
    fn test_developer_role_serde() {
        let role = Role::Developer;
        let json = serde_json::to_string(&role).unwrap();
        assert_eq!(json, "\"developer\"");
        let deser: Role = serde_json::from_str(&json).unwrap();
        assert_eq!(deser, Role::Developer);
    }

    #[test]
    fn test_chat_message_with_tool_calls() {
        let msg = ChatMessage {
            role: Role::Assistant,
            content: String::new(),
            name: None,
            tool_calls: Some(vec![ToolCall {
                id: "call_1".to_string(),
                r#type: "function".to_string(),
                function: FunctionCall {
                    name: "search".to_string(),
                    arguments: "{}".to_string(),
                },
            }]),
            tool_call_id: None,
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert!(json.get("tool_calls").is_some());
        assert!(json.get("tool_call_id").is_none());
        let arr = json["tool_calls"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["function"]["name"], "search");
    }

    #[test]
    fn test_chat_message_tool_result() {
        let msg = ChatMessage {
            role: Role::Tool,
            content: r#"{"temp":72}"#.to_string(),
            name: Some("get_weather".to_string()),
            tool_calls: None,
            tool_call_id: Some("call_abc123".to_string()),
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["role"], "tool");
        assert_eq!(json["tool_call_id"], "call_abc123");
        assert!(json.get("tool_calls").is_none());
    }

    #[test]
    fn test_backward_compat_no_tool_fields() {
        let json = r#"{"role":"user","content":"Hello!"}"#;
        let msg: ChatMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.role, Role::User);
        assert_eq!(msg.content, "Hello!");
        assert!(msg.tool_calls.is_none());
        assert!(msg.tool_call_id.is_none());
        assert!(msg.name.is_none());
    }

    #[test]
    fn test_backward_compat_request_no_tools() {
        let json = r#"{
            "model": "openai/gpt-4o",
            "messages": [{"role":"user","content":"Hi"}],
            "stream": false
        }"#;
        let req: ChatRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.model, "openai/gpt-4o");
        assert!(req.tools.is_none());
        assert!(req.tool_choice.is_none());
    }

    #[test]
    fn test_chat_request_with_tools() {
        let req = ChatRequest {
            model: "openai/gpt-4o".to_string(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: "What's the weather?".to_string(),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            max_tokens: None,
            temperature: None,
            top_p: None,
            stream: false,
            tools: Some(vec![crate::tools::ToolDefinition {
                r#type: "function".to_string(),
                function: crate::tools::FunctionDefinitionInner {
                    name: "get_weather".to_string(),
                    description: Some("Get weather for a location".to_string()),
                    parameters: Some(serde_json::json!({
                        "type": "object",
                        "properties": {
                            "location": { "type": "string" }
                        }
                    })),
                },
            }]),
            tool_choice: Some(serde_json::json!("auto")),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert!(json.get("tools").is_some());
        assert_eq!(json["tool_choice"], "auto");
        let tools = json["tools"].as_array().unwrap();
        assert_eq!(tools[0]["type"], "function");
        assert_eq!(tools[0]["function"]["name"], "get_weather");
    }
}
```

**Step 4: Add tests to `tools.rs`**

Append to `crates/protocol/src/tools.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_call_serde_roundtrip() {
        let tc = ToolCall {
            id: "call_abc123".to_string(),
            r#type: "function".to_string(),
            function: FunctionCall {
                name: "get_weather".to_string(),
                arguments: r#"{"location":"NYC"}"#.to_string(),
            },
        };
        let json = serde_json::to_string(&tc).unwrap();
        let deser: ToolCall = serde_json::from_str(&json).unwrap();
        assert_eq!(deser, tc);
        let val: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(val.get("type").is_some());
        assert!(val.get("r#type").is_none());
    }
}
```

**Step 5: Add tests to `vision.rs`**

Append to `crates/protocol/src/vision.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_content_part_text_and_image() {
        let parts = vec![
            ContentPart::Text {
                text: "What's in this image?".to_string(),
            },
            ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: "https://example.com/img.png".to_string(),
                    detail: Some("high".to_string()),
                },
            },
        ];
        let json = serde_json::to_string(&parts).unwrap();
        let deser: Vec<ContentPart> = serde_json::from_str(&json).unwrap();
        assert_eq!(deser.len(), 2);
        match &deser[0] {
            ContentPart::Text { text } => assert_eq!(text, "What's in this image?"),
            _ => panic!("expected Text variant"),
        }
        match &deser[1] {
            ContentPart::ImageUrl { image_url } => {
                assert_eq!(image_url.url, "https://example.com/img.png");
                assert_eq!(image_url.detail.as_deref(), Some("high"));
            }
            _ => panic!("expected ImageUrl variant"),
        }
    }
}
```

**Step 6: Add tests to `model.rs`**

Append to `crates/protocol/src/model.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_info_capability_fields() {
        let json = r#"{
            "id": "openai/gpt-4o",
            "provider": "openai",
            "model_id": "gpt-4o",
            "display_name": "GPT-4o",
            "input_cost_per_million": 2.5,
            "output_cost_per_million": 10.0,
            "context_window": 128000,
            "supports_streaming": true,
            "supports_tools": true,
            "supports_vision": true,
            "reasoning": false,
            "supports_structured_output": true,
            "supports_batch": true,
            "max_output_tokens": 16384
        }"#;
        let info: ModelInfo = serde_json::from_str(json).unwrap();
        assert!(info.supports_structured_output);
        assert!(info.supports_batch);
        assert_eq!(info.max_output_tokens, Some(16384));
    }

    #[test]
    fn test_model_info_backward_compat() {
        let json = r#"{
            "id": "openai/gpt-4o",
            "provider": "openai",
            "model_id": "gpt-4o",
            "display_name": "GPT-4o",
            "input_cost_per_million": 2.5,
            "output_cost_per_million": 10.0,
            "context_window": 128000
        }"#;
        let info: ModelInfo = serde_json::from_str(json).unwrap();
        assert!(!info.supports_structured_output);
        assert!(!info.supports_batch);
        assert_eq!(info.max_output_tokens, None);
        assert!(!info.supports_streaming);
        assert!(!info.supports_tools);
        assert!(!info.supports_vision);
        assert!(!info.reasoning);
    }
}
```

**Step 7: Run all protocol tests**

Run: `cargo test -p rustyclaw-protocol`
Expected: All tests pass (should be ~20 tests total)

**Step 8: Commit**

```bash
git add crates/protocol/
git commit -m "$(cat <<'EOF'
test: add serde round-trip tests for all protocol types

Migrated from x402-solana and rcr-common test suites. Covers payment
serialization, escrow payloads, chat request/response, tool calls,
vision content parts, model info backward compatibility.
EOF
)"
```

---

### Task 6: Update `x402` crate to depend on `rustyclaw-protocol`

**Files:**
- Modify: `crates/x402/Cargo.toml`
- Modify: `crates/x402/src/types.rs`

**Step 1: Update `Cargo.toml`**

Replace `x402-solana` dependency with `rustyclaw-protocol`:

```toml
[dependencies]
rcr-common = { workspace = true }
rustyclaw-protocol = { workspace = true }
```

Remove the `x402-solana = { workspace = true }` line. Keep `rcr-common` for now — it will be removed in Task 8 after all its usages are migrated.

**Step 2: Update `src/types.rs`**

Replace contents:

```rust
// Re-export all types from rustyclaw-protocol for backward compatibility.
// Downstream crates that depend on x402 continue to work unchanged.
pub use rustyclaw_protocol::*;
```

**Step 3: Find and update any direct `x402_solana::` imports in x402 crate**

Run: `grep -r "x402_solana" crates/x402/src/ --include="*.rs"`

For each file that imports `x402_solana::`, change to either `rustyclaw_protocol::` or `crate::types::` (since `crate::types` re-exports everything from `rustyclaw_protocol`).

Common pattern — in files like `solana.rs`, `facilitator.rs`, `traits.rs`, `escrow/*.rs`:
- `use x402_solana::types::*` → `use crate::types::*` (already works since types.rs re-exports)
- `use x402_solana::constants::*` → `use crate::types::*` (constants are also re-exported)

**Step 4: Verify x402 compiles**

Run: `cargo check -p x402`
Expected: Compiles cleanly

**Step 5: Verify x402 tests pass**

Run: `cargo test -p x402`
Expected: All x402 tests pass

**Step 6: Commit**

```bash
git add crates/x402/
git commit -m "$(cat <<'EOF'
refactor: x402 crate depends on rustyclaw-protocol instead of x402-solana

x402::types now re-exports from rustyclaw-protocol. All internal x402_solana
imports updated. No behavioral changes.
EOF
)"
```

---

### Task 7: Update `router` crate to depend on `rustyclaw-protocol`

**Files:**
- Modify: `crates/router/Cargo.toml`
- Modify: `crates/router/src/models.rs` (line 6)
- Modify: `crates/router/src/scorer.rs` (line 274, in test)

**Step 1: Update `Cargo.toml`**

Replace `rcr-common = { workspace = true }` with `rustyclaw-protocol = { workspace = true }`.

**Step 2: Update imports in `models.rs`**

Line 6: `use rcr_common::types::{CostBreakdown, ModelInfo, PLATFORM_FEE_MULTIPLIER, PLATFORM_FEE_PERCENT};`
→ `use rustyclaw_protocol::{CostBreakdown, ModelInfo, PLATFORM_FEE_MULTIPLIER, PLATFORM_FEE_PERCENT};`

**Step 3: Update imports in `scorer.rs`**

Line 274 (in test module): `use rcr_common::types::{ChatMessage, Role};`
→ `use rustyclaw_protocol::{ChatMessage, Role};`

**Step 4: Verify**

Run: `cargo check -p router && cargo test -p router`
Expected: Compiles and all 13 tests pass

**Step 5: Commit**

```bash
git add crates/router/
git commit -m "$(cat <<'EOF'
refactor: router crate depends on rustyclaw-protocol instead of rcr-common
EOF
)"
```

---

### Task 8: Move `ServiceRegistry` into `gateway`, update gateway deps

**Files:**
- Create: `crates/gateway/src/services.rs` (copy from `crates/common/src/services.rs`)
- Modify: `crates/gateway/Cargo.toml`
- Modify: `crates/gateway/src/lib.rs`
- Modify: `crates/gateway/src/main.rs`
- Modify: `crates/gateway/src/routes/chat.rs`
- Modify: `crates/gateway/src/routes/pricing.rs`
- Modify: `crates/gateway/src/providers/mod.rs`
- Modify: `crates/gateway/src/providers/openai.rs`
- Modify: `crates/gateway/src/providers/anthropic.rs`
- Modify: `crates/gateway/src/providers/google.rs`
- Modify: `crates/gateway/src/providers/xai.rs`
- Modify: `crates/gateway/src/providers/deepseek.rs`
- Modify: `crates/gateway/src/providers/heartbeat.rs`
- Modify: `crates/gateway/src/providers/fallback.rs`
- Modify: `crates/gateway/src/middleware/prompt_guard.rs`
- Modify: `crates/gateway/src/cache.rs`
- Modify: `crates/gateway/tests/integration.rs`

**Step 1: Copy `services.rs` into gateway**

Copy `crates/common/src/services.rs` → `crates/gateway/src/services.rs` verbatim. No changes needed — it only depends on `serde`, `thiserror`, `toml`, and `std::collections::HashMap`, all of which gateway already has.

**Step 2: Add `pub mod services;` to gateway's `lib.rs`**

Add `pub mod services;` to `crates/gateway/src/lib.rs`.

**Step 3: Update `Cargo.toml`**

Replace `rcr-common = { workspace = true }` with `rustyclaw-protocol = { workspace = true }`. Add `toml = { workspace = true }` (needed by services.rs; gateway didn't have it before since it came via rcr-common).

**Step 4: Update all gateway `use rcr_common::types::*` → `use rustyclaw_protocol::*`**

Apply this find-and-replace across all gateway source files:
- `use rcr_common::types::` → `use rustyclaw_protocol::`
- `use rcr_common::types:` → `use rustyclaw_protocol:`

Specific files and their changes:

`src/routes/chat.rs:11`: `use rcr_common::types::ChatRequest` → `use rustyclaw_protocol::ChatRequest`
`src/routes/chat.rs:702`: `use rcr_common::types::{ChatMessage, ModelInfo, Role}` → `use rustyclaw_protocol::{ChatMessage, ModelInfo, Role}`
`src/routes/pricing.rs:7`: `use rcr_common::types::PLATFORM_FEE_PERCENT` → `use rustyclaw_protocol::PLATFORM_FEE_PERCENT`
`src/providers/mod.rs:17`: `use rcr_common::types::{ChatChunk, ChatRequest, ChatResponse, ModelInfo}` → `use rustyclaw_protocol::{ChatChunk, ChatRequest, ChatResponse, ModelInfo}`
`src/providers/openai.rs:3`: `use rcr_common::types::{ChatRequest, ChatResponse, ModelInfo}` → `use rustyclaw_protocol::{ChatRequest, ChatResponse, ModelInfo}`
`src/providers/anthropic.rs:4`: `use rcr_common::types::{...}` → `use rustyclaw_protocol::{...}`
`src/providers/google.rs:4`: `use rcr_common::types::{...}` → `use rustyclaw_protocol::{...}`
`src/providers/xai.rs:3`: `use rcr_common::types::{ChatRequest, ChatResponse, ModelInfo}` → `use rustyclaw_protocol::{ChatRequest, ChatResponse, ModelInfo}`
`src/providers/deepseek.rs:3`: `use rcr_common::types::{ChatRequest, ChatResponse, ModelInfo}` → `use rustyclaw_protocol::{ChatRequest, ChatResponse, ModelInfo}`
`src/providers/heartbeat.rs:17`: `use rcr_common::types::ChatChunk` → `use rustyclaw_protocol::ChatChunk`
`src/providers/heartbeat.rs:149`: `use rcr_common::types::{ChatChunk, ChatChunkChoice, ChatDelta}` → `use rustyclaw_protocol::{ChatChunk, ChatChunkChoice, ChatDelta}`
`src/providers/fallback.rs:10`: `use rcr_common::types::{ChatRequest, ChatResponse}` → `use rustyclaw_protocol::{ChatRequest, ChatResponse}`
`src/middleware/prompt_guard.rs:13`: `use rcr_common::types::ChatMessage` → `use rustyclaw_protocol::ChatMessage`
`src/middleware/prompt_guard.rs:272`: `use rcr_common::types::Role` → `use rustyclaw_protocol::Role`
`src/cache.rs:13`: `use rcr_common::types::{ChatRequest, ChatResponse}` → `use rustyclaw_protocol::{ChatRequest, ChatResponse}`
`src/cache.rs:262`: `use rcr_common::types::{ChatMessage, Role}` → `use rustyclaw_protocol::{ChatMessage, Role}`

**Step 5: Update ServiceRegistry imports**

`src/lib.rs:26`: `use rcr_common::services::ServiceRegistry` → `use crate::services::ServiceRegistry`
`src/main.rs:12`: `use rcr_common::services::ServiceRegistry` → `use gateway::services::ServiceRegistry`
`tests/integration.rs:19`: `use rcr_common::services::ServiceRegistry` → `use gateway::services::ServiceRegistry`

**Step 6: Verify gateway compiles**

Run: `cargo check -p gateway`
Expected: Compiles cleanly

**Step 7: Run all gateway tests**

Run: `cargo test -p gateway`
Expected: All gateway tests pass (191 tests)

**Step 8: Commit**

```bash
git add crates/gateway/
git commit -m "$(cat <<'EOF'
refactor: gateway depends on rustyclaw-protocol, ServiceRegistry moved in

All rcr_common::types imports replaced with rustyclaw_protocol.
ServiceRegistry moved from rcr-common into gateway::services.
EOF
)"
```

---

### Task 9: Update `cli` crate, drop unused `rcr-common` dependency

**Files:**
- Modify: `crates/cli/Cargo.toml`

**Step 1: Remove `rcr-common` from cli's Cargo.toml**

Remove the line `rcr-common = { workspace = true }` from `crates/cli/Cargo.toml`. The cli crate doesn't actually import anything from `rcr-common` (verified by grep — only uses `x402::types::*`).

**Step 2: Verify cli compiles**

Run: `cargo check -p rustyclawrouter-cli`
Expected: Compiles cleanly

**Step 3: Commit**

```bash
git add crates/cli/
git commit -m "$(cat <<'EOF'
chore: remove unused rcr-common dependency from cli crate
EOF
)"
```

---

### Task 10: Remove `rcr-common` dependency from `x402` crate

**Files:**
- Modify: `crates/x402/Cargo.toml`

**Step 1: Check for remaining `rcr_common` imports in x402**

Run: `grep -r "rcr_common" crates/x402/src/ --include="*.rs"`

If any remain, update them to `rustyclaw_protocol::` or `crate::types::`.

**Step 2: Remove `rcr-common` from x402 Cargo.toml**

Remove the line `rcr-common = { workspace = true }`.

**Step 3: Verify**

Run: `cargo check -p x402 && cargo test -p x402`
Expected: Compiles and all tests pass

**Step 4: Commit**

```bash
git add crates/x402/
git commit -m "$(cat <<'EOF'
chore: remove rcr-common dependency from x402 crate
EOF
)"
```

---

### Task 11: Delete `x402-solana` and `rcr-common` crates

**Files:**
- Delete: `crates/x402-solana/` (entire directory)
- Delete: `crates/common/` (entire directory)
- Modify: `Cargo.toml` (workspace root)

**Step 1: Remove from workspace members**

In root `Cargo.toml`, remove `"crates/x402-solana"` and `"crates/common"` from `[workspace.members]`.

Remove from `[workspace.dependencies]`:
- `rcr-common = { path = "crates/common" }`
- `x402-solana = { path = "crates/x402-solana" }`

**Step 2: Delete the directories**

```bash
rm -rf crates/x402-solana/ crates/common/
```

**Step 3: Full workspace check**

Run: `cargo check`
Expected: Entire workspace compiles

**Step 4: Full test suite**

Run: `cargo test`
Expected: All tests pass. Test count should be approximately the same as before (305 base + protocol crate tests ~ 325 total; the duplicate tests that existed in both x402-solana and rcr-common are now in protocol only).

**Step 5: Lint**

Run: `cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings`
Expected: Clean — no warnings, no errors

**Step 6: Commit**

```bash
git add -A
git commit -m "$(cat <<'EOF'
refactor: delete x402-solana and rcr-common crates

Both fully replaced by rustyclaw-protocol. All types, tests, and
re-exports migrated. Workspace reduced from 6 to 5 crates.
EOF
)"
```

---

### Task 12: Update CLAUDE.md and documentation

**Files:**
- Modify: `CLAUDE.md`

**Step 1: Update CLAUDE.md workspace crates section**

Replace the `### Workspace Crates` section to reflect the new structure:
- Remove references to `x402-solana` and `rcr-common`
- Add `rustyclaw-protocol` with its description
- Update `gateway` description to mention it contains `ServiceRegistry`
- Update test counts

**Step 2: Update import ordering example**

The import ordering example in CLAUDE.md references `rcr_common`. Update to `rustyclaw_protocol`.

**Step 3: Update any other `rcr-common` or `x402-solana` references**

Search CLAUDE.md for `rcr-common`, `rcr_common`, `x402-solana`, `x402_solana` and update each reference.

**Step 4: Commit**

```bash
git add CLAUDE.md
git commit -m "$(cat <<'EOF'
docs: update CLAUDE.md for rustyclaw-protocol extraction
EOF
)"
```

---

## Summary

| Task | What | Files changed | Parallelizable with |
|------|------|--------------|---------------------|
| 1 | Create protocol Cargo.toml | 1 new | — |
| 2 | Create protocol source (payment side) | 5 new | Task 3 |
| 3 | Create protocol source (chat side) | 5 new | Task 2 |
| 4 | Add to workspace, verify compile | 1 modified | — |
| 5 | Add protocol tests | 7 modified | — |
| 6 | Update x402 → rustyclaw-protocol | 2+ modified | Task 7 |
| 7 | Update router → rustyclaw-protocol | 3 modified | Task 6 |
| 8 | Move ServiceRegistry, update gateway | 17+ modified | — |
| 9 | Drop rcr-common from cli | 1 modified | Task 10 |
| 10 | Drop rcr-common from x402 | 1 modified | Task 9 |
| 11 | Delete old crates | 2 deleted + 1 modified | — |
| 12 | Update docs | 1 modified | — |

**Tasks 2+3** can run in parallel (different files).
**Tasks 6+7** can run in parallel (different crates, no shared files).
**Tasks 9+10** can run in parallel (different Cargo.toml files).
All other tasks are sequential.
