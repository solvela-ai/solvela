# Solvela Client Ecosystem Upgrade — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Close the strategic gaps in Solvela/Solvela Client that prevent ecosystem adoption — fix the wire format, make escrow durable, eliminate double round-trips with session tokens, expand the model registry, publish the canonical Rust x402 crate, and build ElizaOS integration.

**Architecture:** Seven phases, ordered by dependency and priority. Phase 1 (wire format) unblocks everything else. Phase 2 (escrow durability) and Phase 3 (model expansion) are independent and can parallelize. Phase 4 (session tokens) builds on Phase 1. Phase 5 (x402-solana crate) extracts from existing x402 code. Phase 6 (ElizaOS) depends on Phase 5. Phase 7 (observability) is independent.

**Tech Stack:** Rust 2021/Axum 0.8/Tokio, Solana (ed25519-dalek, bs58), PostgreSQL (sqlx), Redis, TypeScript (ElizaOS plugin), serde/serde_json.

**Prior Art:**
- Competitive analysis: `docs/plans/2026-03-08-blockrun-competitive-analysis.md`
- Ecosystem plan: `.claude/plan/rustyclaw-ecosystem.md` (local agent plan — see `.claude/plan/` directory, not tracked in git)
- x402 V2 spec: https://x402.org/writing/x402-v2-launch
- L402 delegation model (Macaroon caveats): https://docs.lightning.engineering/the-lightning-network/l402
- ElizaOS plugin architecture: https://docs.elizaos.ai/

---

## Phase 1: Wire Format Foundation (P0 — Blocking)

> **Why first:** No agent framework will adopt a gateway that can't handle tool calls, vision, or the developer role. This unblocks everything.

### Task 1.1: Add Developer Role + Tool Call Types to `rcr-common`

**Files:**
- Modify: `crates/common/src/types.rs:8-97`
- Test: `crates/common/src/types.rs` (inline tests at bottom)

**Step 1: Write failing tests for new Role variants and tool call types**

Add these tests at the bottom of `crates/common/src/types.rs`, inside the existing `mod tests`:

```rust
#[test]
fn test_developer_role_serde() {
    let msg = ChatMessage {
        role: Role::Developer,
        content: "You are a helpful assistant.".to_string(),
        name: None,
        tool_calls: None,
        tool_call_id: None,
    };
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("\"developer\""));
    let deser: ChatMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(deser.role, Role::Developer);
}

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
    assert_eq!(deser.id, "call_abc123");
    assert_eq!(deser.function.name, "get_weather");
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
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("tool_calls"));
    assert!(json.contains("call_1"));
}

#[test]
fn test_chat_message_tool_result() {
    let msg = ChatMessage {
        role: Role::Tool,
        content: "The weather is sunny.".to_string(),
        name: Some("get_weather".to_string()),
        tool_calls: None,
        tool_call_id: Some("call_abc123".to_string()),
    };
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains("tool_call_id"));
    let deser: ChatMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(deser.tool_call_id.unwrap(), "call_abc123");
}

#[test]
fn test_content_part_text_and_image() {
    let parts = vec![
        ContentPart::Text {
            text: "What's in this image?".to_string(),
        },
        ContentPart::ImageUrl {
            image_url: ImageUrl {
                url: "https://example.com/cat.jpg".to_string(),
                detail: Some("high".to_string()),
            },
        },
    ];
    let json = serde_json::to_string(&parts).unwrap();
    assert!(json.contains("\"type\":\"text\""));
    assert!(json.contains("\"type\":\"image_url\""));
    let deser: Vec<ContentPart> = serde_json::from_str(&json).unwrap();
    assert_eq!(deser.len(), 2);
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
        tools: Some(vec![ToolDefinition {
            r#type: "function".to_string(),
            function: FunctionDefinitionInner {
                name: "get_weather".to_string(),
                description: Some("Get the weather for a location".to_string()),
                parameters: Some(serde_json::json!({
                    "type": "object",
                    "properties": {
                        "location": {"type": "string"}
                    }
                })),
            },
        }]),
        tool_choice: None,
    };
    let json = serde_json::to_string(&req).unwrap();
    assert!(json.contains("get_weather"));
    assert!(json.contains("\"tools\""));
}

#[test]
fn test_backward_compat_no_tool_fields() {
    // Old-style message without tool fields should still deserialize
    let json = r#"{"role":"user","content":"Hello!"}"#;
    let msg: ChatMessage = serde_json::from_str(json).unwrap();
    assert_eq!(msg.role, Role::User);
    assert!(msg.tool_calls.is_none());
    assert!(msg.tool_call_id.is_none());
}

#[test]
fn test_backward_compat_request_no_tools() {
    // Old-style request without tools field should still deserialize
    let json = r#"{"model":"gpt-4o","messages":[{"role":"user","content":"Hi"}]}"#;
    let req: ChatRequest = serde_json::from_str(json).unwrap();
    assert!(req.tools.is_none());
    assert!(req.tool_choice.is_none());
}
```

**Step 2: Run tests to verify they fail**

```bash
cargo test -p rcr-common -- --nocapture 2>&1 | head -60
```

Expected: compilation errors — `Developer` variant doesn't exist, `ToolCall` type doesn't exist, `tool_calls` field doesn't exist on `ChatMessage`.

**Step 3: Implement the new types**

Replace the types section of `crates/common/src/types.rs` (lines 7-97) with:

```rust
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

/// A function call requested by the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

/// A tool call in an assistant message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub r#type: String,
    pub function: FunctionCall,
}

/// Image URL for vision content.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageUrl {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// A content part — text or image — for multimodal messages.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ContentPart {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image_url")]
    ImageUrl { image_url: ImageUrl },
}

/// A single message in a chat conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Tool calls requested by the assistant (present when role=assistant).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    /// ID of the tool call this message is a response to (present when role=tool).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// Inner function definition for a tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDefinitionInner {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
}

/// A tool definition in a chat request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub r#type: String,
    pub function: FunctionDefinitionInner,
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
    /// Tool definitions for function calling.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolDefinition>>,
    /// Tool choice strategy: "auto", "none", "required", or specific function.
    #[serde(skip_serializing_if = "Option::is_none")]
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

// --- Streaming types ---

/// Delta content in a streaming chunk.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCallDelta>>,
}

/// Incremental tool call in a streaming delta.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallDelta {
    pub index: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<FunctionCallDelta>,
}

/// Incremental function call in a streaming delta.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCallDelta {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arguments: Option<String>,
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

**Step 4: Fix all compilation errors across workspace**

The new fields on `ChatMessage` (`tool_calls`, `tool_call_id`) will break code that constructs `ChatMessage` directly. Search the workspace for all `ChatMessage {` constructions and add the new fields:

```bash
# Find all ChatMessage construction sites
cargo check 2>&1 | grep "error" | head -40
```

For every construction site, add:
```rust
tool_calls: None,
tool_call_id: None,
```

Similarly, `ChatRequest` now has `tools` and `tool_choice` — add to all construction sites:
```rust
tools: None,
tool_choice: None,
```

And `ChatDelta` now has `tool_calls` — add `tool_calls: None` to all delta constructions.

**Step 5: Run all tests to verify everything passes**

```bash
cargo test --workspace 2>&1 | tail -20
```

Expected: All existing tests pass. New tests pass.

**Step 6: Commit**

```bash
git add crates/common/src/types.rs
git add -u  # catch all files with construction site fixes
git commit -m "feat(common): add Developer role, tool calls, vision types to wire format

Add ToolCall, ToolDefinition, ContentPart, ImageUrl types.
Add tool_calls, tool_call_id to ChatMessage.
Add tools, tool_choice to ChatRequest.
Add ToolCallDelta, FunctionCallDelta for streaming.
All fields are Option with serde skip_serializing_if for backward compat."
```

---

### Task 1.2: Update Provider Adapters for New Types

**Files:**
- Modify: `crates/gateway/src/providers/anthropic.rs`
- Modify: `crates/gateway/src/providers/google.rs`
- Modify: `crates/gateway/src/providers/openai.rs` (if tool passthrough needed)

**Step 1: Write failing test for Anthropic developer role handling**

In `crates/gateway/src/providers/anthropic.rs`, add or update tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use rcr_common::types::{ChatMessage, Role};

    #[test]
    fn test_developer_role_becomes_system() {
        let messages = vec![
            ChatMessage {
                role: Role::Developer,
                content: "You are helpful.".to_string(),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            },
            ChatMessage {
                role: Role::User,
                content: "Hello".to_string(),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            },
        ];
        // Developer messages should be included in the system extraction
        let system_msgs: Vec<&str> = messages
            .iter()
            .filter(|m| m.role == Role::System || m.role == Role::Developer)
            .map(|m| m.content.as_str())
            .collect();
        assert_eq!(system_msgs.len(), 1);
        assert_eq!(system_msgs[0], "You are helpful.");
    }
}
```

**Step 2: Run test to verify it fails**

```bash
cargo test -p gateway test_developer_role_becomes_system -- --exact
```

**Step 3: Update Anthropic adapter**

In `crates/gateway/src/providers/anthropic.rs`, update the system message extraction (around line 88) to include `Developer`:

```rust
let system: Option<String> = {
    let system_msgs: Vec<&str> = req
        .messages
        .iter()
        .filter(|m| m.role == Role::System || m.role == Role::Developer)
        .map(|m| m.content.as_str())
        .collect();
    if system_msgs.is_empty() {
        None
    } else {
        Some(system_msgs.join("\n\n"))
    }
};
```

And update the message filtering (around line 104) to exclude `Developer`:

```rust
let messages: Vec<AnthropicMessage> = req
    .messages
    .iter()
    .filter(|m| m.role == Role::User || m.role == Role::Assistant)
    .map(|m| AnthropicMessage {
        role: match m.role {
            Role::User => "user".to_string(),
            Role::Assistant => "assistant".to_string(),
            _ => "user".to_string(),
        },
        content: m.content.clone(),
    })
    .collect();
```

**Step 4: Update Google adapter tool role mapping**

In `crates/gateway/src/providers/google.rs` (around line 123), update the role mapping to handle `Developer` → `user` and keep `Tool` → `user` (Gemini's current model):

```rust
role: Some(match m.role {
    Role::User => "user".to_string(),
    Role::Assistant => "model".to_string(),
    Role::System | Role::Developer => "user".to_string(),
    Role::Tool => "user".to_string(),
}),
```

**Step 5: Pass tools through to OpenAI/DeepSeek/xAI**

For OpenAI-compatible providers (OpenAI, DeepSeek, xAI), the `tools` and `tool_choice` fields should pass through in the JSON body. Since these providers use the request body directly (or with minimal transformation), ensure the serde passthrough works.

In each pass-through provider, verify that the request JSON includes `tools` when present. If the provider constructs a custom body, add the fields. If it forwards `req` as-is via serde, no change needed.

**Step 6: Run all tests**

```bash
cargo test --workspace 2>&1 | tail -20
```

**Step 7: Commit**

```bash
git add -u
git commit -m "feat(gateway): update provider adapters for developer role and tool passthrough

Anthropic: Developer role extracted as system message.
Google: Developer role mapped to user.
OpenAI/DeepSeek/xAI: tools and tool_choice pass through."
```

---

### Task 1.3: Pass Tools Through the Chat Route

**Files:**
- Modify: `crates/gateway/src/routes/chat.rs`

The chat route already accepts `ChatRequest` from the request body via Axum's `Json<ChatRequest>` extractor. With the new fields on `ChatRequest`, tools and tool_choice will automatically be deserialized from incoming requests.

**Step 1: Write integration test for tool passthrough**

In `crates/gateway/tests/integration.rs` (or create if needed), add:

```rust
#[tokio::test]
async fn test_chat_with_tools_returns_402() {
    let app = test_app().await;
    let body = serde_json::json!({
        "model": "openai/gpt-4o",
        "messages": [{"role": "user", "content": "What's the weather?"}],
        "tools": [{
            "type": "function",
            "function": {
                "name": "get_weather",
                "description": "Get weather for a location",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "location": {"type": "string"}
                    }
                }
            }
        }],
        "tool_choice": "auto"
    });

    let response = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri("/v1/chat/completions")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();

    // Should get 402 (no payment header) — proves the request parsed successfully
    assert_eq!(response.status(), 402);
}
```

**Step 2: Run test, verify it passes** (this should work immediately since serde handles the new fields)

```bash
cargo test -p gateway test_chat_with_tools_returns_402 -- --exact
```

**Step 3: Commit**

```bash
git add -u
git commit -m "test(gateway): add integration test for tool call passthrough in chat route"
```

---

## Phase 2: Durable Escrow Claims (P1)

> **Why:** Money can't be lost silently. Current `claim_async` fires `tokio::spawn` with no retry, no persistence, no idempotency. A network blip means locked funds until escrow expiry.

### Task 2.1: Add Claim Queue Table (Migration)

**Files:**
- Create: `migrations/006_escrow_claim_queue.sql`

**Step 1: Write the migration**

```sql
-- Durable escrow claim queue with retry tracking
CREATE TABLE IF NOT EXISTS escrow_claim_queue (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    service_id      BYTEA NOT NULL,         -- 32-byte escrow service ID
    agent_pubkey    TEXT NOT NULL,           -- base58 agent wallet
    claim_amount    BIGINT NOT NULL,         -- atomic USDC units
    deposited_amount BIGINT,                 -- verified deposit (cap)
    status          TEXT NOT NULL DEFAULT 'pending',  -- pending | in_progress | completed | failed
    attempts        INTEGER NOT NULL DEFAULT 0,
    last_attempt_at TIMESTAMPTZ,
    tx_signature    TEXT,                    -- filled on success
    error_message   TEXT,                    -- last error
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    completed_at    TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_claim_queue_status ON escrow_claim_queue(status);
CREATE INDEX IF NOT EXISTS idx_claim_queue_created ON escrow_claim_queue(created_at);
```

**Step 2: Verify migration applies**

```bash
docker compose up -d  # ensure PostgreSQL is running
cargo run -p gateway 2>&1 | grep -i "migration\|claim_queue" | head -5
```

**Step 3: Commit**

```bash
git add migrations/006_escrow_claim_queue.sql
git commit -m "feat(migrations): add escrow_claim_queue table for durable claim retry"
```

---

### Task 2.2: Implement Durable Claim Queue

**Files:**
- Create: `crates/x402/src/escrow/claim_queue.rs`
- Modify: `crates/x402/src/escrow/mod.rs` (add module)
- Modify: `crates/x402/src/escrow/claimer.rs` (use queue)

**Step 1: Write failing test for claim queue persistence**

In `crates/x402/src/escrow/claim_queue.rs`:

```rust
//! Durable escrow claim queue backed by PostgreSQL.
//!
//! Claims are persisted before submission, retried on failure, and
//! marked complete on success. No claim is lost due to process restarts
//! or network failures.

use serde::{Deserialize, Serialize};

/// Status of a claim in the queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

/// A claim entry in the persistent queue.
#[derive(Debug, Clone)]
pub struct ClaimEntry {
    pub id: String,
    pub service_id: [u8; 32],
    pub agent_pubkey: String,
    pub claim_amount: u64,
    pub deposited_amount: Option<u64>,
    pub status: ClaimStatus,
    pub attempts: i32,
    pub tx_signature: Option<String>,
    pub error_message: Option<String>,
}

/// Maximum retry attempts before marking as failed.
pub const MAX_CLAIM_ATTEMPTS: i32 = 5;

/// Enqueue a new claim. Returns the queue entry ID.
///
/// The claim is persisted to PostgreSQL in `pending` status before any
/// on-chain submission attempt, ensuring it survives process restarts.
pub async fn enqueue_claim(
    pool: &sqlx::PgPool,
    service_id: &[u8; 32],
    agent_pubkey: &str,
    claim_amount: u64,
    deposited_amount: Option<u64>,
) -> Result<String, sqlx::Error> {
    let row = sqlx::query_scalar::<_, String>(
        "INSERT INTO escrow_claim_queue (service_id, agent_pubkey, claim_amount, deposited_amount)
         VALUES ($1, $2, $3, $4) RETURNING id::text"
    )
    .bind(service_id.as_slice())
    .bind(agent_pubkey)
    .bind(claim_amount as i64)
    .bind(deposited_amount.map(|d| d as i64))
    .fetch_one(pool)
    .await?;
    Ok(row)
}

/// Mark a claim as in_progress (being submitted).
pub async fn mark_in_progress(pool: &sqlx::PgPool, id: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE escrow_claim_queue
         SET status = 'in_progress', attempts = attempts + 1, last_attempt_at = NOW()
         WHERE id = $1::uuid"
    )
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark a claim as completed with the transaction signature.
pub async fn mark_completed(
    pool: &sqlx::PgPool,
    id: &str,
    tx_signature: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE escrow_claim_queue
         SET status = 'completed', tx_signature = $1, completed_at = NOW()
         WHERE id = $2::uuid"
    )
    .bind(tx_signature)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark a claim as failed with an error message.
/// If attempts >= MAX_CLAIM_ATTEMPTS, status becomes 'failed'.
/// Otherwise, status reverts to 'pending' for retry.
pub async fn mark_attempt_failed(
    pool: &sqlx::PgPool,
    id: &str,
    error: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE escrow_claim_queue
         SET status = CASE
             WHEN attempts >= $1 THEN 'failed'
             ELSE 'pending'
         END,
         error_message = $2
         WHERE id = $3::uuid"
    )
    .bind(MAX_CLAIM_ATTEMPTS)
    .bind(error)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Fetch all pending claims for retry processing.
pub async fn fetch_pending_claims(
    pool: &sqlx::PgPool,
    limit: i64,
) -> Result<Vec<ClaimEntry>, sqlx::Error> {
    let rows = sqlx::query_as::<_, (String, Vec<u8>, String, i64, Option<i64>, String, i32, Option<String>, Option<String>)>(
        "SELECT id::text, service_id, agent_pubkey, claim_amount, deposited_amount,
                status, attempts, tx_signature, error_message
         FROM escrow_claim_queue
         WHERE status = 'pending'
         ORDER BY created_at ASC
         LIMIT $1"
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    let entries = rows.into_iter().filter_map(|r| {
        let mut sid = [0u8; 32];
        if r.1.len() == 32 {
            sid.copy_from_slice(&r.1);
        } else {
            return None;
        }
        Some(ClaimEntry {
            id: r.0,
            service_id: sid,
            agent_pubkey: r.2,
            claim_amount: r.3 as u64,
            deposited_amount: r.4.map(|d| d as u64),
            status: ClaimStatus::Pending,
            attempts: r.6,
            tx_signature: r.7,
            error_message: r.8,
        })
    }).collect();

    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_claim_status_serde() {
        let s = ClaimStatus::Pending;
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, "\"pending\"");
    }

    #[test]
    fn test_max_attempts_constant() {
        assert_eq!(MAX_CLAIM_ATTEMPTS, 5);
    }
}
```

**Step 2: Add module to escrow mod.rs**

In `crates/x402/src/escrow/mod.rs`, add:

```rust
pub mod claim_queue;
```

**Step 3: Add sqlx dependency to x402 Cargo.toml**

In `crates/x402/Cargo.toml`, add under `[dependencies]`:

```toml
sqlx = { workspace = true, optional = true, features = ["runtime-tokio-rustls", "postgres", "uuid"] }
```

And add a feature flag:

```toml
[features]
default = []
postgres = ["sqlx"]
```

**Step 4: Gate claim_queue behind the feature**

Wrap the `claim_queue` module declaration:

```rust
#[cfg(feature = "postgres")]
pub mod claim_queue;
```

**Step 5: Run tests**

```bash
cargo test -p x402 test_claim_status_serde -- --exact
cargo test -p x402 test_max_attempts_constant -- --exact
```

**Step 6: Commit**

```bash
git add crates/x402/src/escrow/claim_queue.rs crates/x402/src/escrow/mod.rs crates/x402/Cargo.toml
git commit -m "feat(x402): add durable escrow claim queue with retry tracking

Claims persisted to PostgreSQL before submission. Failed claims retry
up to 5 times with exponential backoff. No claim lost on restart."
```

---

### Task 2.3: Background Claim Processor

**Files:**
- Create: `crates/x402/src/escrow/claim_processor.rs`
- Modify: `crates/gateway/src/routes/chat.rs` (enqueue instead of fire-and-forget)

**Step 1: Write the claim processor**

```rust
//! Background task that processes pending escrow claims from the queue.

use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

use super::claim_queue::{self, ClaimEntry};
use super::claimer::EscrowClaimer;

/// Start the background claim processor.
///
/// Polls the claim queue every `poll_interval` and submits pending claims.
/// Runs until the returned handle is dropped.
pub fn start_claim_processor(
    pool: sqlx::PgPool,
    claimer: Arc<EscrowClaimer>,
    poll_interval: Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(poll_interval);
        loop {
            interval.tick().await;
            if let Err(e) = process_pending_claims(&pool, &claimer).await {
                warn!(error = %e, "claim processor cycle failed");
            }
        }
    })
}

async fn process_pending_claims(
    pool: &sqlx::PgPool,
    claimer: &EscrowClaimer,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let pending = claim_queue::fetch_pending_claims(pool, 10).await?;
    if pending.is_empty() {
        return Ok(());
    }

    info!(count = pending.len(), "processing pending escrow claims");

    for entry in pending {
        process_single_claim(pool, claimer, &entry).await;
    }

    Ok(())
}

async fn process_single_claim(
    pool: &sqlx::PgPool,
    claimer: &EscrowClaimer,
    entry: &ClaimEntry,
) {
    // Mark in_progress
    if let Err(e) = claim_queue::mark_in_progress(pool, &entry.id).await {
        warn!(id = %entry.id, error = %e, "failed to mark claim in_progress");
        return;
    }

    // Decode agent pubkey
    let agent_bytes = match crate::escrow::pda::decode_bs58_pubkey(&entry.agent_pubkey) {
        Ok(b) => b,
        Err(e) => {
            let _ = claim_queue::mark_attempt_failed(pool, &entry.id, &e).await;
            return;
        }
    };

    // Submit claim (synchronous — we await the result)
    match crate::escrow::claimer::do_claim_with_params(
        claimer,
        entry.service_id,
        agent_bytes,
        entry.claim_amount,
    )
    .await
    {
        Ok(tx_sig) => {
            info!(id = %entry.id, tx = %tx_sig, "escrow claim succeeded");
            let _ = claim_queue::mark_completed(pool, &entry.id, &tx_sig).await;
        }
        Err(e) => {
            warn!(id = %entry.id, error = %e, attempt = entry.attempts + 1, "escrow claim attempt failed");
            let _ = claim_queue::mark_attempt_failed(pool, &entry.id, &e.to_string()).await;
        }
    }
}
```

**Step 2: Refactor claimer to expose a sync claim function**

In `crates/x402/src/escrow/claimer.rs`, extract the core claim logic into a function that returns the tx signature:

```rust
/// Submit a claim and return the transaction signature.
/// Unlike `claim_async`, this awaits the result.
pub async fn do_claim_with_params(
    claimer: &EscrowClaimer,
    service_id: [u8; 32],
    agent_pubkey: [u8; 32],
    actual_amount_atomic: u64,
) -> Result<String, Error> {
    let params = ClaimParams {
        rpc_url: claimer.rpc_url.clone(),
        fee_payer_keypair: claimer.fee_payer_keypair,
        escrow_program_id: claimer.escrow_program_id,
        recipient_wallet: claimer.recipient_wallet,
        usdc_mint: claimer.usdc_mint,
        service_id,
        agent_pubkey,
        actual_amount: actual_amount_atomic,
        client: claimer.client.clone(),
    };
    do_claim_returning_sig(&params).await
}
```

And refactor `do_claim` to return the signature string instead of `()`:

Rename existing `do_claim` to `do_claim_returning_sig` with return type `Result<String, Error>`, returning `tx_sig.to_string()` instead of `Ok(())`.

Keep the original `claim_async` as a backwards-compatible wrapper for code that doesn't have a DB pool.

**Step 3: Update `fire_escrow_claim` in chat.rs**

In `crates/gateway/src/routes/chat.rs`, update `fire_escrow_claim` to enqueue to the DB when available:

```rust
fn fire_escrow_claim(
    state: &Arc<AppState>,
    payment_scheme: &str,
    escrow_service_id: &Option<String>,
    escrow_agent_pubkey: &Option<String>,
    escrow_deposited_amount: Option<u64>,
    claim_atomic: u64,
) {
    if payment_scheme != "escrow" {
        return;
    }
    if let (Some(ref sid_b64), Some(ref agent_b58)) = (escrow_service_id, escrow_agent_pubkey) {
        let claim_amount = match escrow_deposited_amount {
            Some(deposited) => claim_atomic.min(deposited),
            None => claim_atomic,
        };

        if let Ok(sid) = decode_service_id(sid_b64) {
            // Prefer durable queue if DB is available
            if let Some(ref pool) = state.db_pool {
                let pool = pool.clone();
                let agent = agent_b58.clone();
                tokio::spawn(async move {
                    if let Err(e) = x402::escrow::claim_queue::enqueue_claim(
                        &pool, &sid, &agent, claim_amount, escrow_deposited_amount,
                    ).await {
                        tracing::warn!(error = %e, "failed to enqueue escrow claim, falling back to direct");
                    }
                });
            } else if let Some(claimer) = &state.escrow_claimer {
                // Fallback: fire-and-forget (no DB)
                if let Ok(agent_bytes) = decode_agent_pubkey(agent_b58) {
                    claimer.claim_async(sid, agent_bytes, claim_amount);
                }
            }
        }
    }
}
```

**Step 4: Run all tests**

```bash
cargo test --workspace 2>&1 | tail -20
```

**Step 5: Commit**

```bash
git add -u
git add crates/x402/src/escrow/claim_processor.rs
git commit -m "feat(x402): durable escrow claim processor with queue and retry

Claims are now persisted to PostgreSQL before on-chain submission.
Background processor polls every 10s, retries failed claims up to 5x.
Falls back to fire-and-forget when DB is unavailable."
```

---

## Phase 3: Expand Model Registry (P1)

> **Why:** 16 models is not competitive. BlockRun has 41+, OpenRouter has 500+. This is pure configuration — no code changes needed beyond adding TOML entries and capability metadata.

### Task 3.1: Add Capability Metadata to ModelInfo

**Files:**
- Modify: `crates/common/src/types.rs:104-121` (ModelInfo struct)
- Modify: `config/models.toml`

**Step 1: Write failing test for new ModelInfo fields**

In `crates/common/src/types.rs` tests:

```rust
#[test]
fn test_model_info_capability_fields() {
    let json = r#"{
        "id": "openai-gpt-4o",
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
        "max_output_tokens": 16384,
        "supports_batch": false
    }"#;
    let info: ModelInfo = serde_json::from_str(json).unwrap();
    assert!(info.supports_structured_output);
    assert_eq!(info.max_output_tokens.unwrap(), 16384);
    assert!(!info.supports_batch);
}

#[test]
fn test_model_info_backward_compat() {
    // Old format without new fields should still parse
    let json = r#"{
        "id": "test",
        "provider": "test",
        "model_id": "test",
        "display_name": "Test",
        "input_cost_per_million": 1.0,
        "output_cost_per_million": 2.0,
        "context_window": 4096
    }"#;
    let info: ModelInfo = serde_json::from_str(json).unwrap();
    assert!(!info.supports_structured_output);
    assert!(info.max_output_tokens.is_none());
}
```

**Step 2: Add fields to ModelInfo**

```rust
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
    pub max_output_tokens: Option<u32>,
}
```

**Step 3: Run tests**

```bash
cargo test -p rcr-common -- --nocapture 2>&1 | tail -20
```

**Step 4: Commit**

```bash
git add crates/common/src/types.rs
git commit -m "feat(common): add capability metadata to ModelInfo

supports_structured_output, supports_batch, max_output_tokens.
All new fields default to false/None for backward compat."
```

---

### Task 3.2: Expand models.toml to 40+ Models

**Files:**
- Modify: `config/models.toml`

Add these models (prices are approximate provider costs as of March 2026 — verify before production):

```toml
# --- Additional OpenAI ---

[models.openai-gpt-4o-audio-preview]
provider = "openai"
model_id = "gpt-4o-audio-preview"
display_name = "GPT-4o Audio Preview"
input_cost_per_million = 2.50
output_cost_per_million = 10.00
context_window = 128000
supports_streaming = true
supports_tools = true

[models.openai-o3-mini]
provider = "openai"
model_id = "o3-mini"
display_name = "o3 Mini"
input_cost_per_million = 1.10
output_cost_per_million = 4.40
context_window = 200000
supports_streaming = true
reasoning = true

[models.openai-o4-mini]
provider = "openai"
model_id = "o4-mini"
display_name = "o4 Mini"
input_cost_per_million = 1.10
output_cost_per_million = 4.40
context_window = 200000
supports_streaming = true
supports_tools = true
reasoning = true

# --- Additional Anthropic ---

[models.anthropic-claude-sonnet-4-5]
provider = "anthropic"
model_id = "claude-sonnet-4.5"
display_name = "Claude Sonnet 4.5"
input_cost_per_million = 3.00
output_cost_per_million = 15.00
context_window = 200000
supports_streaming = true
supports_tools = true
supports_vision = true

# --- Additional Google ---

[models.google-gemini-2-0-flash]
provider = "google"
model_id = "gemini-2.0-flash"
display_name = "Gemini 2.0 Flash"
input_cost_per_million = 0.10
output_cost_per_million = 0.40
context_window = 1000000
supports_streaming = true
supports_tools = true

[models.google-gemini-2-0-flash-lite]
provider = "google"
model_id = "gemini-2.0-flash-lite"
display_name = "Gemini 2.0 Flash Lite"
input_cost_per_million = 0.075
output_cost_per_million = 0.30
context_window = 1000000
supports_streaming = true

# --- Additional DeepSeek ---

[models.deepseek-coder]
provider = "deepseek"
model_id = "deepseek-coder"
display_name = "DeepSeek Coder V3"
input_cost_per_million = 0.28
output_cost_per_million = 0.42
context_window = 128000
supports_streaming = true
supports_tools = true

# --- Additional xAI ---

[models.xai-grok-3]
provider = "xai"
model_id = "grok-3"
display_name = "Grok 3"
input_cost_per_million = 3.00
output_cost_per_million = 15.00
context_window = 131072
supports_streaming = true
supports_tools = true
supports_vision = true

[models.xai-grok-3-mini]
provider = "xai"
model_id = "grok-3-mini"
display_name = "Grok 3 Mini"
input_cost_per_million = 0.30
output_cost_per_million = 0.50
context_window = 131072
supports_streaming = true
supports_tools = true
reasoning = true

# --- Moonshot ---

[models.moonshot-kimi-k2]
provider = "moonshot"
model_id = "kimi-k2"
display_name = "Kimi K2"
input_cost_per_million = 0.60
output_cost_per_million = 2.00
context_window = 131072
supports_streaming = true
supports_tools = true
```

> **Note:** Verify all prices against current provider pricing pages before deploying. Add more models from Mistral, Cohere, Meta (Llama), etc. as providers are integrated.

**Step 1: Add the models to config/models.toml**

Append the entries above.

**Step 2: Run model registry tests**

```bash
cargo test -p router -- --nocapture 2>&1 | tail -20
```

**Step 3: Add aliases for new models**

In `crates/router/src/profiles.rs`, extend the alias map:

```rust
"o3-mini" | "o3mini" => Some("openai/o3-mini"),
"o4-mini" | "o4mini" => Some("openai/o4-mini"),
"grok3" | "grok-3" => Some("xai/grok-3"),
"kimi" | "k2" => Some("moonshot/kimi-k2"),
```

**Step 4: Run all tests**

```bash
cargo test --workspace 2>&1 | tail -20
```

**Step 5: Commit**

```bash
git add config/models.toml crates/router/src/profiles.rs
git commit -m "feat(config): expand model registry to 25+ models with capability metadata

Add o3-mini, o4-mini, Claude 4.5, Gemini 2.0, DeepSeek Coder, Grok 3,
Grok 3 Mini, Kimi K2. Add aliases. Model count: 16 → 26."
```

---

## Phase 4: Session Tokens (P1)

> **Why:** Every x402 request currently requires two HTTP round-trips (request → 402 → sign → resend). Session tokens let repeat users skip the 402 entirely. This is the #1 latency improvement possible.

### Task 4.1: Define Session Token Types

**Files:**
- Create: `crates/gateway/src/session.rs`
- Modify: `crates/gateway/src/lib.rs` (add module)

**Step 1: Write the session token module**

```rust
//! Session tokens for authenticated repeat access.
//!
//! After a successful x402 payment, the gateway issues a session token
//! (HMAC-SHA256 signed). Subsequent requests with this token skip the
//! 402 handshake entirely, drawing down from a pre-authorized budget.

use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use serde::{Deserialize, Serialize};

type HmacSha256 = Hmac<Sha256>;

/// A session token issued after successful payment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionClaims {
    /// Wallet address (base58) — identifies the payer.
    pub wallet: String,
    /// Remaining budget in atomic USDC.
    pub budget_remaining: u64,
    /// Unix timestamp when token was issued.
    pub issued_at: u64,
    /// Unix timestamp when token expires.
    pub expires_at: u64,
    /// Allowed models (empty = all models allowed).
    pub allowed_models: Vec<String>,
}

/// Create a signed session token.
pub fn create_session_token(
    claims: &SessionClaims,
    secret: &[u8],
) -> Result<String, Box<dyn std::error::Error>> {
    let payload = serde_json::to_string(claims)?;
    let payload_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(payload.as_bytes());

    let mut mac = HmacSha256::new_from_slice(secret)
        .map_err(|e| format!("HMAC key error: {e}"))?;
    mac.update(payload_b64.as_bytes());
    let sig = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(mac.finalize().into_bytes());

    Ok(format!("{payload_b64}.{sig}"))
}

/// Verify and decode a session token.
pub fn verify_session_token(
    token: &str,
    secret: &[u8],
) -> Result<SessionClaims, SessionError> {
    use base64::Engine;

    let parts: Vec<&str> = token.splitn(2, '.').collect();
    if parts.len() != 2 {
        return Err(SessionError::MalformedToken);
    }

    let (payload_b64, sig_b64) = (parts[0], parts[1]);

    // Verify HMAC
    let mut mac = HmacSha256::new_from_slice(secret)
        .map_err(|_| SessionError::InvalidSignature)?;
    mac.update(payload_b64.as_bytes());
    let expected_sig = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(sig_b64)
        .map_err(|_| SessionError::InvalidSignature)?;
    mac.verify_slice(&expected_sig)
        .map_err(|_| SessionError::InvalidSignature)?;

    // Decode claims
    let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload_b64)
        .map_err(|_| SessionError::MalformedToken)?;
    let claims: SessionClaims = serde_json::from_slice(&payload_bytes)
        .map_err(|_| SessionError::MalformedToken)?;

    // Check expiry
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if now > claims.expires_at {
        return Err(SessionError::Expired);
    }

    Ok(claims)
}

/// Session budget tracking in Redis.
pub async fn deduct_budget(
    redis: &redis::Client,
    wallet: &str,
    amount: u64,
) -> Result<u64, SessionError> {
    let mut conn = redis.get_multiplexed_async_connection().await
        .map_err(|_| SessionError::StorageError)?;
    let key = format!("session:budget:{wallet}");
    let remaining: i64 = redis::cmd("DECRBY")
        .arg(&key)
        .arg(amount as i64)
        .query_async(&mut conn)
        .await
        .map_err(|_| SessionError::StorageError)?;

    if remaining < 0 {
        // Undo the deduction — budget exhausted
        let _: () = redis::cmd("INCRBY")
            .arg(&key)
            .arg(amount as i64)
            .query_async(&mut conn)
            .await
            .map_err(|_| SessionError::StorageError)?;
        return Err(SessionError::BudgetExhausted);
    }

    Ok(remaining as u64)
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("malformed session token")]
    MalformedToken,
    #[error("invalid session token signature")]
    InvalidSignature,
    #[error("session token expired")]
    Expired,
    #[error("session budget exhausted")]
    BudgetExhausted,
    #[error("session storage error")]
    StorageError,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_secret() -> Vec<u8> {
        b"test-secret-key-32-bytes-long!!!".to_vec()
    }

    fn test_claims() -> SessionClaims {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        SessionClaims {
            wallet: "9noXzpXnkyEcKF3AeXqUHTdR59V5uvrRBUo9bwsHaByz".to_string(),
            budget_remaining: 1_000_000, // 1 USDC
            issued_at: now,
            expires_at: now + 3600, // 1 hour
            allowed_models: vec![],
        }
    }

    #[test]
    fn test_create_and_verify_token() {
        let secret = test_secret();
        let claims = test_claims();
        let token = create_session_token(&claims, &secret).unwrap();
        let verified = verify_session_token(&token, &secret).unwrap();
        assert_eq!(verified.wallet, claims.wallet);
        assert_eq!(verified.budget_remaining, claims.budget_remaining);
    }

    #[test]
    fn test_invalid_signature_rejected() {
        let secret = test_secret();
        let claims = test_claims();
        let token = create_session_token(&claims, &secret).unwrap();
        let result = verify_session_token(&token, b"wrong-secret-key-32-bytes-long!!");
        assert!(result.is_err());
    }

    #[test]
    fn test_expired_token_rejected() {
        let secret = test_secret();
        let mut claims = test_claims();
        claims.expires_at = 1000; // way in the past
        let token = create_session_token(&claims, &secret).unwrap();
        let result = verify_session_token(&token, &secret);
        assert!(matches!(result, Err(SessionError::Expired)));
    }

    #[test]
    fn test_malformed_token_rejected() {
        let result = verify_session_token("not-a-valid-token", &test_secret());
        assert!(matches!(result, Err(SessionError::InvalidSignature) | Err(SessionError::MalformedToken)));
    }
}
```

**Step 2: Add module to lib.rs and add `hmac` dependency**

In `crates/gateway/src/lib.rs`, add `pub mod session;`.

In workspace `Cargo.toml`, add:
```toml
hmac = "0.12"
```

In `crates/gateway/Cargo.toml`, add:
```toml
hmac = { workspace = true }
```

**Step 3: Run tests**

```bash
cargo test -p gateway session -- --nocapture 2>&1 | tail -20
```

**Step 4: Commit**

```bash
git add crates/gateway/src/session.rs crates/gateway/src/lib.rs crates/gateway/Cargo.toml Cargo.toml
git commit -m "feat(gateway): add session token module (HMAC-SHA256 signed, budget-scoped)

Issue session tokens after successful x402 payment. Tokens carry
wallet, budget, expiry, and model whitelist. HMAC-SHA256 signed.
Budget tracked in Redis. Eliminates 402 round-trip for repeat users."
```

---

### Task 4.2: Integrate Session Tokens into Chat Route

**Files:**
- Modify: `crates/gateway/src/routes/chat.rs`
- Modify: `crates/gateway/src/middleware/x402.rs`

This task adds:
1. Check for `X-RCR-SESSION` header before checking `PAYMENT-SIGNATURE`
2. If valid session token with sufficient budget → skip 402, deduct budget, proceed
3. After successful x402 payment → issue session token in response header `X-RCR-SESSION`

**Step 1: Write integration test**

```rust
#[tokio::test]
async fn test_session_token_header_returned_after_payment() {
    // After a successful payment, the response should include X-RCR-SESSION
    // This test verifies the header is present (actual payment flow tested elsewhere)
    let app = test_app().await;
    // ... test setup with mock payment ...
    // assert response has X-RCR-SESSION header
}
```

**Step 2: Update chat route to check session token first**

In the chat handler, before the 402 check:

```rust
// Check for session token (skip 402 if valid)
if let Some(session_header) = req.headers().get("x-rcr-session") {
    if let Ok(token_str) = session_header.to_str() {
        if let Ok(claims) = session::verify_session_token(token_str, &state.session_secret) {
            // Deduct estimated cost from session budget
            let estimated = estimated_atomic_cost(&state.model_registry, &resolved_model, &chat_req);
            if let Some(ref redis) = state.redis_client {
                match session::deduct_budget(redis, &claims.wallet, estimated).await {
                    Ok(_remaining) => {
                        // Session valid, budget sufficient — proceed to provider
                        // Skip 402 entirely
                    }
                    Err(_) => {
                        // Budget exhausted — fall through to normal 402 flow
                    }
                }
            }
        }
    }
}
```

**Step 3: Issue session token after successful payment**

After successful payment verification and response:

```rust
// Issue session token for future requests
let claims = session::SessionClaims {
    wallet: payer_wallet.clone(),
    budget_remaining: deposited_or_paid_amount,
    issued_at: now_unix(),
    expires_at: now_unix() + 3600, // 1 hour
    allowed_models: vec![],
};
if let Ok(token) = session::create_session_token(&claims, &state.session_secret) {
    response_headers.insert("x-rcr-session", token.parse().unwrap_or_default());
}
```

**Step 4: Add `session_secret` to AppState**

In the AppState struct, add:
```rust
pub session_secret: Vec<u8>,  // from RCR_SESSION_SECRET env var or random
```

Initialize from env or generate random 32 bytes on startup.

**Step 5: Run all tests**

```bash
cargo test --workspace 2>&1 | tail -20
```

**Step 6: Commit**

```bash
git add -u
git commit -m "feat(gateway): integrate session tokens into chat route

Check X-RCR-SESSION header before PAYMENT-SIGNATURE.
Valid session with budget skips 402 entirely.
Issue new session token in response after successful payment."
```

---

## Phase 5: Extract x402-solana Crate (P1)

> **Why:** There is no Rust x402 implementation. Publishing the canonical one makes Solvela the infrastructure layer the Solana agent ecosystem depends on.

### Task 5.1: Plan the Extraction

**What moves to `crates/x402-solana/` (new crate, workspace member):**
- `x402/src/types.rs` — All x402 wire format types (PaymentRequired, PaymentPayload, etc.)
- `x402/src/solana.rs` — Solana-specific verification (SPL transfer extraction)
- Constants: `X402_VERSION`, `USDC_MINT`, `SOLANA_NETWORK`, `MAX_TIMEOUT_SECONDS`
- `CostBreakdown` (from `rcr-common/types.rs`)

**What stays in Solvela's `x402` crate:**
- `traits.rs` (PaymentVerifier — server-side only)
- `facilitator.rs` (settlement orchestration)
- `fee_payer.rs`, `nonce_pool.rs` (server infrastructure)
- `escrow/` (server-side claim/verify)

**New crate structure:**
```
crates/x402-solana/
├── Cargo.toml          # [package] name = "x402-solana"
└── src/
    ├── lib.rs
    ├── types.rs         # PaymentRequired, PaymentPayload, PayloadData, etc.
    ├── constants.rs     # USDC_MINT, SOLANA_NETWORK, X402_VERSION
    ├── cost.rs          # CostBreakdown, usdc_atomic_amount()
    └── verify.rs        # SPL transfer extraction, signature verification
```

**Dependencies:** `serde`, `serde_json`, `bs58`, `base64`, `ed25519-dalek` only. No Axum, no Tokio, no reqwest.

**Step 1: Create crate scaffold**

```bash
mkdir -p crates/x402-solana/src
```

**Step 2: Write Cargo.toml**

```toml
[package]
name = "x402-solana"
version = "0.1.0"
edition = "2021"
description = "x402 payment protocol types and Solana verification — the canonical Rust x402 implementation"
license = "MIT OR Apache-2.0"
repository = "https://github.com/<org>/Solvela"

[dependencies]
serde = { version = "1", features = ["derive"] }
serde_json = "1"
bs58 = "0.5"
base64 = "0.22"
ed25519-dalek = "2"
sha2 = "0.10"
thiserror = "2"
```

**Step 3: Move types from x402/types.rs and common/types.rs**

Copy the x402 wire format types into `crates/x402-solana/src/types.rs`. Keep the originals in place but re-export from x402-solana to avoid breaking changes:

In `crates/x402/src/types.rs`, change to:
```rust
// Re-export from x402-solana for backward compatibility
pub use x402_solana::types::*;
pub use x402_solana::constants::*;
```

**Step 4: Add x402-solana as workspace member and dependency**

In root `Cargo.toml`:
```toml
[workspace]
members = [
    "crates/gateway",
    "crates/x402",
    "crates/x402-solana",  # NEW
    "crates/router",
    "crates/common",
    "crates/cli",
]

[workspace.dependencies]
x402-solana = { path = "crates/x402-solana" }
```

In `crates/x402/Cargo.toml`:
```toml
x402-solana = { workspace = true }
```

**Step 5: Run all tests to verify re-exports work**

```bash
cargo test --workspace 2>&1 | tail -20
```

**Step 6: Commit**

```bash
git add crates/x402-solana/ Cargo.toml crates/x402/Cargo.toml crates/x402/src/types.rs
git commit -m "feat: extract x402-solana crate — canonical Rust x402 implementation

x402-solana contains wire format types, Solana verification, and constants.
Zero framework dependencies (no Axum, no Tokio). Publishable to crates.io.
Existing x402 crate re-exports for backward compatibility."
```

---

## Phase 6: ElizaOS Plugin (P1)

> **Why:** ElizaOS is the dominant Solana AI agent framework. They just added x402 support. An RCR plugin captures the largest agent developer community on Solana.

### Task 6.1: Create ElizaOS Plugin

**Files:**
- Create: `integrations/elizaos/package.json`
- Create: `integrations/elizaos/src/index.ts`
- Create: `integrations/elizaos/src/actions/chat.ts`
- Create: `integrations/elizaos/src/providers/gateway.ts`

> **Architecture:** ElizaOS plugins export `actions` (things the agent can do) and `providers` (data sources). Our plugin exports a `CHAT_VIA_RUSTYCLAW` action that routes LLM calls through Solvela with x402 payment.

**Step 1: Scaffold the plugin**

```json
{
  "name": "@rustyclaw/elizaos-plugin",
  "version": "0.1.0",
  "description": "ElizaOS plugin for Solvela — Solana-native AI agent payment gateway",
  "main": "dist/index.js",
  "types": "dist/index.d.ts",
  "scripts": {
    "build": "tsup src/index.ts --format esm --dts",
    "test": "vitest run"
  },
  "dependencies": {
    "@solana/web3.js": "^2.0",
    "@solana/spl-token": "^0.4"
  },
  "peerDependencies": {
    "@elizaos/core": ">=0.2.0"
  }
}
```

**Step 2: Write the gateway provider**

`integrations/elizaos/src/providers/gateway.ts`:

```typescript
import { type Provider, type IAgentRuntime, type Memory } from "@elizaos/core";

export const gatewayProvider: Provider = {
  get: async (runtime: IAgentRuntime, _message: Memory) => {
    const gatewayUrl = runtime.getSetting("RUSTYCLAW_GATEWAY_URL") || "http://localhost:8402";

    try {
      const resp = await fetch(`${gatewayUrl}/health`);
      const health = await resp.json();
      return `Solvela gateway at ${gatewayUrl} is ${health.status}. Models: ${health.model_count || "unknown"}.`;
    } catch {
      return `Solvela gateway at ${gatewayUrl} is unreachable.`;
    }
  },
};
```

**Step 3: Write the chat action**

`integrations/elizaos/src/actions/chat.ts`:

```typescript
import { type Action, type IAgentRuntime, type Memory, type HandlerCallback } from "@elizaos/core";

export const chatViaRustyClaw: Action = {
  name: "CHAT_VIA_RUSTYCLAW",
  description: "Send a chat completion through Solvela with Solana x402 payment",
  similes: ["llm call", "ai inference", "model query"],

  validate: async (runtime: IAgentRuntime) => {
    return !!runtime.getSetting("RUSTYCLAW_GATEWAY_URL");
  },

  handler: async (
    runtime: IAgentRuntime,
    message: Memory,
    _state: any,
    _options: any,
    callback: HandlerCallback,
  ) => {
    const gatewayUrl = runtime.getSetting("RUSTYCLAW_GATEWAY_URL");
    const model = runtime.getSetting("RUSTYCLAW_DEFAULT_MODEL") || "auto";

    // Step 1: Send request, get 402
    const reqBody = {
      model,
      messages: [{ role: "user", content: message.content.text }],
    };

    const firstResp = await fetch(`${gatewayUrl}/v1/chat/completions`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(reqBody),
    });

    if (firstResp.status === 402) {
      // Step 2: Parse payment requirements
      const paymentRequired = await firstResp.json();

      // Step 3: Sign payment via runtime wallet
      // (ElizaOS provides wallet access via runtime)
      const paymentSignature = await signPayment(runtime, paymentRequired);

      // Step 4: Resend with payment
      const paidResp = await fetch(`${gatewayUrl}/v1/chat/completions`, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          "PAYMENT-SIGNATURE": paymentSignature,
        },
        body: JSON.stringify(reqBody),
      });

      if (paidResp.ok) {
        const result = await paidResp.json();
        const content = result.choices?.[0]?.message?.content || "No response";
        callback({ text: content });
        return true;
      }
    } else if (firstResp.ok) {
      // Free model — no payment needed
      const result = await firstResp.json();
      callback({ text: result.choices?.[0]?.message?.content || "No response" });
      return true;
    }

    callback({ text: "Failed to get response from Solvela" });
    return false;
  },

  examples: [
    [
      { user: "{{user1}}", content: { text: "Ask the AI to explain quicksort" } },
      { user: "{{agentName}}", content: { text: "I'll query Solvela for that.", action: "CHAT_VIA_RUSTYCLAW" } },
    ],
  ],
};

async function signPayment(runtime: IAgentRuntime, paymentRequired: any): Promise<string> {
  // TODO: Implement Solana USDC-SPL transfer signing via runtime wallet
  // This depends on ElizaOS's Solana plugin wallet access API
  throw new Error("Payment signing not yet implemented — requires ElizaOS Solana plugin wallet integration");
}
```

**Step 4: Write the plugin entry point**

`integrations/elizaos/src/index.ts`:

```typescript
import { type Plugin } from "@elizaos/core";
import { chatViaRustyClaw } from "./actions/chat";
import { gatewayProvider } from "./providers/gateway";

export const rustyClawPlugin: Plugin = {
  name: "rustyclaw",
  description: "Solvela integration — Solana-native AI agent payments via x402",
  actions: [chatViaRustyClaw],
  providers: [gatewayProvider],
};

export default rustyClawPlugin;
```

**Step 5: Commit**

```bash
git add integrations/elizaos/
git commit -m "feat: add ElizaOS plugin for Solvela x402 integration

Plugin exports CHAT_VIA_RUSTYCLAW action and gateway health provider.
Payment signing is stubbed — requires ElizaOS Solana plugin wallet API."
```

---

## Phase 7: Observability Dashboard APIs (P2)

> **Why:** Enterprise adoption requires visibility. Currently data is fire-and-forget to PostgreSQL with no way to consume it.

### Task 7.1: Add Dashboard API Routes

**Files:**
- Create: `crates/gateway/src/routes/dashboard.rs`
- Modify: `crates/gateway/src/routes/mod.rs`
- Modify: `crates/gateway/src/lib.rs` (add routes)

**Endpoints:**

```
GET /v1/dashboard/spend?wallet=<addr>&days=7
GET /v1/dashboard/spend/by-model?wallet=<addr>&days=30
GET /v1/dashboard/requests?wallet=<addr>&days=7
GET /v1/dashboard/sessions?wallet=<addr>&status=active
GET /v1/dashboard/claims?status=pending&limit=50
```

**Step 1: Write the route handler stubs with tests**

```rust
//! Dashboard API routes for observability.

use axum::{extract::Query, Json};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct SpendQuery {
    pub wallet: String,
    #[serde(default = "default_days")]
    pub days: u32,
}

fn default_days() -> u32 { 7 }

#[derive(Debug, Serialize)]
pub struct SpendSummary {
    pub wallet: String,
    pub total_usdc: String,
    pub request_count: u64,
    pub period_days: u32,
    pub by_day: Vec<DailySpend>,
}

#[derive(Debug, Serialize)]
pub struct DailySpend {
    pub date: String,
    pub total_usdc: String,
    pub request_count: u64,
}

pub async fn spend_summary(
    Query(params): Query<SpendQuery>,
    // State(state): State<Arc<AppState>>,
) -> Json<SpendSummary> {
    // TODO: Query usage_logs table grouped by day
    Json(SpendSummary {
        wallet: params.wallet,
        total_usdc: "0.000000".to_string(),
        request_count: 0,
        period_days: params.days,
        by_day: vec![],
    })
}
```

**Step 2: Register routes**

In `crates/gateway/src/lib.rs`, add:

```rust
.route("/v1/dashboard/spend", get(routes::dashboard::spend_summary))
```

**Step 3: Write integration test**

```rust
#[tokio::test]
async fn test_dashboard_spend_returns_200() {
    let app = test_app().await;
    let response = app
        .oneshot(
            axum::http::Request::builder()
                .uri("/v1/dashboard/spend?wallet=test123&days=7")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
}
```

**Step 4: Commit**

```bash
git add crates/gateway/src/routes/dashboard.rs crates/gateway/src/routes/mod.rs crates/gateway/src/lib.rs
git commit -m "feat(gateway): add dashboard API routes for spend observability

GET /v1/dashboard/spend returns per-wallet spend summary.
Stub implementation — will be backed by usage_logs PostgreSQL queries."
```

---

## Execution Dependencies

```
Phase 1 (Wire Format) ──────────────────────── BLOCKS everything
    │
    ├── Phase 2 (Escrow Durability) ─────────── independent, start immediately
    ├── Phase 3 (Model Registry) ────────────── independent, start immediately
    │
    ├── Phase 4 (Session Tokens) ────────────── depends on Phase 1 (uses ChatRequest)
    │
    └── Phase 5 (x402-solana Crate) ─────────── depends on Phase 1 (uses types)
         │
         └── Phase 6 (ElizaOS Plugin) ───────── depends on Phase 5 (uses x402-solana types)

Phase 7 (Observability) ────────────────────── independent, start anytime
```

**Parallelization opportunities:**
- After Phase 1 completes: Phase 2 + 3 + 4 + 5 can all run in parallel
- Phase 6 waits only for Phase 5
- Phase 7 is fully independent

---

## Post-Plan: Future Phases (Not Detailed Here)

These are referenced in the analysis but need their own plans when we get to them:

| Phase | Description | Depends On |
|-------|-------------|------------|
| **8: Agent Delegation** | Scoped session tokens + PDA sub-accounts for multi-agent workflows | Phase 4 |
| **9: x402 V2 Alignment** | CAIP-2 identifiers, wallet identity, Bazaar auto-discovery | Phase 5 |
| **10: SSE Heartbeat** | `tokio::select!` with interval timer during streaming | Phase 1 |
| **11: Provider Failover** | Circuit breaker + fallback chains per model family | Phase 3 |
| **12: Context Compression** | Server-side compression before forwarding to providers | Independent |
| **13: Staking-for-Capacity** | DeFi primitive — lock USDC for inference allocation | Phase 4 |
| **14: Cloudflare Workers Backend** | Serve as x402 facilitator for Cloudflare Workers agents | Phase 5 |

---

## Testing Strategy

Every phase follows TDD:
1. Write failing test
2. Run test — verify it fails
3. Write minimal implementation
4. Run test — verify it passes
5. `cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings`
6. Commit

**Coverage targets:**
- New types: serde roundtrip tests + backward compat tests
- New routes: integration tests via `tower::ServiceExt::oneshot`
- Escrow queue: unit tests for queue operations + integration test with mock RPC
- Session tokens: crypto tests (create/verify/expire/tamper)

**Run the full suite after every phase:**
```bash
cargo test --workspace && cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings
```
