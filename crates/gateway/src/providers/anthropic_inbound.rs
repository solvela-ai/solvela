//! Inbound Anthropic Messages API translation (`POST /v1/messages`).
//!
//! This is the MIRROR of [`crate::providers::anthropic`], which is
//! OUTBOUND-only (gateway → Anthropic). Here Solvela acts as the SERVER of the
//! Anthropic Messages wire format: a client (e.g. Claude Code) sends an
//! Anthropic-shaped request, the gateway translates it into the internal
//! OpenAI-shaped [`ChatRequest`], runs the EXISTING chat pipeline (model
//! resolution → x402 payment → provider → record), then translates the
//! resulting [`ChatResponse`] back into the Anthropic Messages response shape.
//!
//! The field-mapping decisions are settled in `anthropic.rs` and reused here so
//! the inbound and outbound directions never drift:
//! - `system` is a top-level param, NOT a message role (Anthropic carries only
//!   `user`/`assistant` turns). Inbound, the `system` field becomes a
//!   [`Role::System`] message at the head of `messages`.
//! - `content` is either a bare string OR an array of typed content blocks;
//!   text-only is a one-element `[{"type":"text"}]`. Claude Code sends BOTH the
//!   bare-string `system` and the array-of-blocks `system`, so both are
//!   accepted.
//! - `stop_reason` maps `end_turn`/`stop_sequence` → OpenAI `stop`, `max_tokens`
//!   → `length` (the outbound `from_anthropic_response` map). The INVERSE map is
//!   applied here when emitting the Anthropic response: `stop` → `end_turn`,
//!   `length` → `max_tokens`.
//! - `usage` is reconstructed via [`solvela_protocol::Usage`]'s saturating
//!   semantics; the Anthropic response exposes `input_tokens`/`output_tokens`
//!   (which Claude Code reads).
//!
//! SCOPE (PR2): text-only, streaming (SSE) AND non-streaming. A `stream:true`
//! request is framed into the Anthropic Messages SSE event sequence by
//! [`frame_anthropic_stream`]; a `stream:false`/absent request runs the
//! buffer-and-translate path. Tools (top-level `tools`/`tool_choice` definitions
//! AND `tool_use`/`tool_result` content blocks), `count_tokens`, and images
//! remain OUT OF SCOPE. Every one of these is rejected LOUDLY rather than
//! silently dropped: a dropped image or a discarded tool definition would change
//! the prompt's meaning (or strip tool-calling) while still billing the agent —
//! the same fail-loud posture the outbound adapter takes for system/tool-role
//! images. Tool definitions in particular must be modeled on the wire
//! (`AnthropicMessagesRequest::tools`), not left to serde's ignore-unknown-fields
//! default, because Claude Code attaches them to nearly every request.

use std::collections::VecDeque;
use std::convert::Infallible;

use axum::response::sse;
use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};
use tracing::error;

use solvela_protocol::{ChatChunk, ChatMessage, ChatRequest, ChatResponse, MessageContent, Role};

use crate::providers::heartbeat::HeartbeatItem;

/// Errors from translating an inbound Anthropic Messages request.
///
/// Mapped by the route handler to an Anthropic error envelope
/// (`{"type":"error","error":{"type":"invalid_request_error","message":…}}`)
/// with a 400 status. The message is a static/safe description of the
/// client-side problem — it never carries internal detail.
#[derive(Debug, thiserror::Error)]
pub enum AnthropicInboundError {
    /// The request body did not deserialize as an Anthropic Messages request.
    #[error("invalid Anthropic Messages request: {0}")]
    InvalidBody(String),

    /// An image content block was present. Images are deferred to a later PR;
    /// reject loudly rather than silently dropping (which would bill the agent
    /// for a prompt the model never fully sees).
    #[error(
        "image content is not yet supported on POST /v1/messages; send text-only content \
         (image support is planned for a later release)"
    )]
    ImageUnsupported,

    /// A tool definition (top-level `tools`/`tool_choice`) OR a tool content
    /// block (`tool_use`/`tool_result`) was present. Tools are OUT OF SCOPE for
    /// PR1; reject loudly rather than silently dropping the tool definitions and
    /// billing the agent for a tool-blind text answer (Claude Code attaches its
    /// tool definitions to nearly every request, so a silent drop is the common
    /// case, not the edge case).
    #[error(
        "tool use is not yet supported on POST /v1/messages; remove `tools`/`tool_choice` \
         and tool content blocks (tool support is planned for a later release)"
    )]
    ToolUseUnsupported,
}

// ---------------------------------------------------------------------------
// Inbound request wire types (Anthropic Messages → ChatRequest)
// ---------------------------------------------------------------------------

/// The inbound Anthropic Messages request. Deserialize-only (the gateway is the
/// server here). Unknown fields are ignored (forward-compat with newer Claude
/// Code clients sending fields we do not yet model, e.g. `metadata`).
#[derive(Debug, Deserialize)]
pub struct AnthropicMessagesRequest {
    pub model: String,
    /// Anthropic requires `max_tokens`. We keep it optional on the wire so a
    /// missing value degrades to the pipeline's own default rather than a hard
    /// parse failure, matching the lenient posture of the OpenAI path.
    #[serde(default)]
    pub max_tokens: Option<u32>,
    /// `system` is either a bare string OR an array of text blocks (Claude Code
    /// sends the array form). Both deserialize via [`AnthropicSystem`].
    #[serde(default)]
    pub system: Option<AnthropicSystem>,
    pub messages: Vec<AnthropicInboundMessage>,
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub top_p: Option<f32>,
    #[serde(default)]
    pub stop_sequences: Option<Vec<String>>,
    /// Streaming is OUT OF SCOPE for PR1. Captured so a `stream:true` request
    /// can be rejected with a clear error rather than silently served
    /// non-streaming (which would break a client expecting SSE).
    #[serde(default)]
    pub stream: bool,
    /// Top-level tool DEFINITIONS. Tools are OUT OF SCOPE for PR1, but the field
    /// MUST be modeled: Claude Code attaches `tools` to nearly every request, and
    /// without a field here serde would silently discard them (unknown fields are
    /// ignored) → the provider gets a tool-blind call and the agent PAYS for a
    /// degraded text answer. Captured so a request carrying tools is rejected
    /// loudly BEFORE the money path, mirroring the `stream:true` rejection. A
    /// JSON `null` (absent) is the only accepted value.
    #[serde(default)]
    pub tools: Option<serde_json::Value>,
    /// Tool-choice directive. OUT OF SCOPE for PR1 and rejected for the same
    /// reason as `tools` (a `tool_choice` without `tools` is degenerate, but a
    /// present value still signals the client expects tool-calling behavior we do
    /// not yet support — reject rather than silently ignore).
    #[serde(default)]
    pub tool_choice: Option<serde_json::Value>,
}

/// The `system` field: a bare string OR an array of text blocks.
///
/// `#[serde(untagged)]` tries the string variant first; a JSON array falls
/// through to `Blocks`. This matches the two shapes Claude Code emits.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum AnthropicSystem {
    Text(String),
    Blocks(Vec<AnthropicSystemTextBlock>),
}

impl AnthropicSystem {
    /// Flatten the system field to a single string. Array-of-blocks joins the
    /// text of each text block with `\n\n` (the same separator the outbound
    /// adapter uses to join multiple system messages).
    fn flatten(&self) -> String {
        match self {
            AnthropicSystem::Text(s) => s.clone(),
            AnthropicSystem::Blocks(blocks) => blocks
                .iter()
                .map(|b| b.text.as_str())
                .collect::<Vec<_>>()
                .join("\n\n"),
        }
    }

    /// True when the flattened system text is empty (after trimming). An empty
    /// system prompt is dropped rather than prepended as an empty System
    /// message.
    fn is_empty(&self) -> bool {
        self.flatten().trim().is_empty()
    }
}

/// A single `system` text block (`{"type":"text","text":"…"}`). Only the text
/// is retained; `cache_control` and other annotations are ignored on the
/// inbound side (the gateway re-derives its own caching on the outbound call).
#[derive(Debug, Deserialize)]
pub struct AnthropicSystemTextBlock {
    pub text: String,
}

/// An inbound Anthropic message. Anthropic carries only `user`/`assistant`
/// roles; an unknown role string is preserved as a raw string and mapped to
/// [`Role::User`] (the safe default the outbound adapter also uses).
#[derive(Debug, Deserialize)]
pub struct AnthropicInboundMessage {
    pub role: String,
    pub content: AnthropicInboundContent,
}

/// Inbound message content: a bare string OR an array of content blocks.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum AnthropicInboundContent {
    Text(String),
    Blocks(Vec<AnthropicInboundContentBlock>),
}

/// An inbound content block. PR1 models only `text`; an `image` block is
/// captured by the `Image` variant so it can be rejected with a clear error
/// (rather than silently dropped). Any other block type (e.g. `tool_use`,
/// `tool_result`) is OUT OF SCOPE and lands in `Other`, which is also rejected.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnthropicInboundContentBlock {
    Text {
        text: String,
    },
    Image,
    #[serde(other)]
    Other,
}

// ---------------------------------------------------------------------------
// Outbound response wire types (ChatResponse → Anthropic Messages response)
// ---------------------------------------------------------------------------

/// The Anthropic Messages response the gateway emits. Serialize-only.
///
/// Wire shape (verified against the Anthropic Messages API docs
/// platform.claude.com/docs/en/api/messages, 2026-06-19):
/// ```json
/// {
///   "id": "msg_…",
///   "type": "message",
///   "role": "assistant",
///   "model": "claude-…",
///   "content": [{"type":"text","text":"…"}],
///   "stop_reason": "end_turn",
///   "stop_sequence": null,
///   "usage": {"input_tokens": 10, "output_tokens": 8}
/// }
/// ```
#[derive(Debug, Serialize)]
pub struct AnthropicMessagesResponse {
    pub id: String,
    #[serde(rename = "type")]
    pub message_type: AnthropicMessageType,
    pub role: AnthropicAssistantRole,
    pub model: String,
    pub content: Vec<AnthropicResponseTextBlock>,
    /// `stop_reason` is `null` until the model finishes; once finished it is
    /// `end_turn` / `max_tokens` / `stop_sequence`. We emit `null` only when the
    /// internal `finish_reason` is absent.
    pub stop_reason: Option<String>,
    /// The matched stop sequence, or `null`. PR1 does not surface which stop
    /// sequence matched (the internal pipeline does not expose it), so this is
    /// always `null` — a field Claude Code tolerates being null.
    pub stop_sequence: Option<String>,
    pub usage: AnthropicResponseUsage,
}

/// Pins the literal `"message"` discriminant so it can never drift.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnthropicMessageType {
    Message,
}

/// Pins the literal `"assistant"` role on the response.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnthropicAssistantRole {
    Assistant,
}

/// A response text block (`{"type":"text","text":"…"}`).
#[derive(Debug, Serialize)]
pub struct AnthropicResponseTextBlock {
    #[serde(rename = "type")]
    pub block_type: AnthropicResponseTextBlockType,
    pub text: String,
}

/// Pins the literal `"text"` discriminant for a response content block.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnthropicResponseTextBlockType {
    Text,
}

/// Anthropic response usage. Claude Code reads `input_tokens`/`output_tokens`.
#[derive(Debug, Serialize)]
pub struct AnthropicResponseUsage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

// ---------------------------------------------------------------------------
// Translation
// ---------------------------------------------------------------------------

/// Translate one inbound Anthropic content value into an internal
/// [`MessageContent`].
///
/// Text-only content (bare string, or array of text blocks) becomes
/// [`MessageContent::Text`] — the same flattened representation the internal
/// pipeline expects for text. An image block returns
/// [`AnthropicInboundError::ImageUnsupported`]; any other non-text block
/// (`tool_use`/`tool_result`) returns
/// [`AnthropicInboundError::ToolUseUnsupported`] so the request is rejected with
/// an accurate diagnostic rather than billed with a silently-dropped block (PR1
/// is text-only).
fn inbound_content_to_message_content(
    content: &AnthropicInboundContent,
) -> Result<MessageContent, AnthropicInboundError> {
    match content {
        AnthropicInboundContent::Text(s) => Ok(MessageContent::Text(s.clone())),
        AnthropicInboundContent::Blocks(blocks) => {
            let mut texts: Vec<&str> = Vec::with_capacity(blocks.len());
            for block in blocks {
                match block {
                    AnthropicInboundContentBlock::Text { text } => texts.push(text.as_str()),
                    AnthropicInboundContentBlock::Image => {
                        return Err(AnthropicInboundError::ImageUnsupported);
                    }
                    // `tool_use`/`tool_result` (and any future non-text block)
                    // land here. Return the tool-specific error so the client
                    // gets an accurate diagnostic, not a misleading "image"
                    // message that sends it down the wrong path.
                    AnthropicInboundContentBlock::Other => {
                        return Err(AnthropicInboundError::ToolUseUnsupported);
                    }
                }
            }
            // Join multiple text blocks with a single space, matching
            // `MessageContent::as_text`'s separator so a multi-block message
            // reads naturally to the prompt guard and string-based adapters.
            Ok(MessageContent::Text(texts.join(" ")))
        }
    }
}

/// Map an inbound Anthropic role string to an internal [`Role`].
///
/// Anthropic carries only `user`/`assistant`; an unknown role maps to
/// [`Role::User`] (the safe default the outbound adapter uses for the inverse).
fn inbound_role(role: &str) -> Role {
    match role {
        "assistant" => Role::Assistant,
        // "user" and anything unexpected both map to User.
        _ => Role::User,
    }
}

/// Deserialize and translate an inbound Anthropic Messages request into the
/// internal [`ChatRequest`].
///
/// - `system` (bare string OR array-of-blocks) is prepended as a single
///   [`Role::System`] message when non-empty.
/// - Each `messages[]` entry's text content (string OR `[{type:"text"}]`)
///   becomes a [`MessageContent::Text`]; an image/other block is rejected.
/// - `model`, `max_tokens`, `temperature`, `top_p` carry across.
/// - `stop_sequences` is carried via the request — the internal pipeline does
///   not yet forward stop sequences to providers, so this is accepted and
///   ignored for now (documented; a later PR can thread it through). It is NOT
///   silently lost in a money-relevant way: it does not affect cost.
///
/// Streaming requests, and requests carrying top-level tool definitions
/// (`tools`/`tool_choice`) or tool content blocks, are rejected here (PR1 is
/// text-only, non-streaming, no tools) rather than silently served / degraded.
pub fn anthropic_request_to_chat(
    req: AnthropicMessagesRequest,
) -> Result<ChatRequest, AnthropicInboundError> {
    // Streaming is now SUPPORTED (PR2): `stream` carries through to the internal
    // `ChatRequest` and the shared core frames the OpenAI `ChatChunk` stream into
    // the Anthropic SSE event sequence (see `frame_anthropic_stream`). Tools and
    // images remain OUT OF SCOPE and are still rejected below.

    // Reject top-level tool definitions LOUDLY, BEFORE the money path. Without
    // this, a request carrying `tools`/`tool_choice` would translate to a
    // tool-blind `ChatRequest` (we hard-code `tools: None` below) and the agent
    // would PAY for a degraded text answer — the silent-degradation failure mode
    // this endpoint explicitly forbids. Mirror the `stream:true` rejection.
    if req.tools.is_some() || req.tool_choice.is_some() {
        return Err(AnthropicInboundError::ToolUseUnsupported);
    }

    let mut messages: Vec<ChatMessage> = Vec::with_capacity(req.messages.len() + 1);

    // Prepend the system prompt as a System message when present and non-empty.
    if let Some(ref system) = req.system {
        if !system.is_empty() {
            messages.push(ChatMessage {
                role: Role::System,
                content: MessageContent::Text(system.flatten()),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            });
        }
    }

    for m in &req.messages {
        let content = inbound_content_to_message_content(&m.content)?;
        messages.push(ChatMessage {
            role: inbound_role(&m.role),
            content,
            name: None,
            tool_calls: None,
            tool_call_id: None,
        });
    }

    Ok(ChatRequest {
        model: req.model,
        messages,
        max_tokens: req.max_tokens,
        temperature: req.temperature,
        top_p: req.top_p,
        // Carry the client's streaming choice through to the shared core. When
        // `true`, the core frames the internal OpenAI `ChatChunk` stream into the
        // Anthropic SSE event sequence; when `false`, the existing buffer-and-
        // translate path runs. Tools/images stay rejected above either way.
        stream: req.stream,
        tools: None,
        tool_choice: None,
    })
}

/// Map an internal OpenAI-style `finish_reason` to an Anthropic `stop_reason`.
///
/// This is the INVERSE of the outbound `from_anthropic_response` map in
/// `anthropic.rs` (`end_turn`/`stop_sequence` → `stop`, `max_tokens` →
/// `length`). Here: `stop` → `end_turn`, `length` → `max_tokens`. Any other
/// value passes through unchanged (forward-compat) so a future internal reason
/// is never lost. `None` (model still going / provider omitted it) stays
/// `None` — Claude Code tolerates a null `stop_reason`.
fn finish_reason_to_stop_reason(finish_reason: Option<&str>) -> Option<String> {
    finish_reason.map(|r| match r {
        "stop" => "end_turn".to_string(),
        "length" => "max_tokens".to_string(),
        // INTENTIONAL passthrough (do NOT "fix" to a default): the provider
        // boundary is trusted — adapters are registry-controlled, so an
        // unrecognized reason is a forward-compat signal to forward verbatim,
        // never client-injected. Coercing it to a default would silently lose a
        // real provider signal. Locked by `unknown_finish_reason_passes_through`
        // (non-streaming) and `framer_unknown_finish_reason_passes_through`
        // (streaming).
        other => other.to_string(),
    })
}

/// Translate an internal [`ChatResponse`] into the Anthropic Messages response.
///
/// - Concatenates the text of every assistant choice's message into a single
///   `text` content block (PR1 emits one block; the internal response carries a
///   single choice).
/// - `stop_reason` is the inverse-mapped `finish_reason` of the first choice.
/// - `usage.{input_tokens,output_tokens}` come from the internal `Usage`
///   (0/0 when the provider omitted usage — the same fallback the OpenAI path
///   tolerates; Claude Code reads these but does not require them to be
///   non-zero).
pub fn chat_response_to_anthropic(resp: &ChatResponse) -> AnthropicMessagesResponse {
    // Concatenate text content across choices (there is normally exactly one).
    let text: String = resp
        .choices
        .iter()
        .map(|c| c.message.content.as_text())
        .collect::<Vec<_>>()
        .join("");

    let stop_reason = finish_reason_to_stop_reason(
        resp.choices
            .first()
            .and_then(|c| c.finish_reason.as_deref()),
    );

    let (input_tokens, output_tokens) = match &resp.usage {
        Some(u) => (u.prompt_tokens, u.completion_tokens),
        None => (0, 0),
    };

    AnthropicMessagesResponse {
        id: resp.id.clone(),
        message_type: AnthropicMessageType::Message,
        role: AnthropicAssistantRole::Assistant,
        model: resp.model.clone(),
        content: vec![AnthropicResponseTextBlock {
            block_type: AnthropicResponseTextBlockType::Text,
            text,
        }],
        stop_reason,
        stop_sequence: None,
        usage: AnthropicResponseUsage {
            input_tokens,
            output_tokens,
        },
    }
}

// ---------------------------------------------------------------------------
// Streaming response translation (ChatChunk stream → Anthropic SSE events)
// ---------------------------------------------------------------------------

/// The fixed index of the single text content block emitted for a streamed
/// text response. PR2 is text-only, so there is exactly one block (`index: 0`).
const ANTHROPIC_TEXT_BLOCK_INDEX: u32 = 0;

/// The Anthropic streaming event discriminant. The SAME value drives BOTH the
/// SSE `event:` name line AND the `data` JSON's `"type"` field, so the two can
/// never diverge — a requirement of `@anthropic-ai/sdk`, which rejects a frame
/// whose `data.type` does not equal its `event:` name.
///
/// Serialize-only: `#[serde(rename_all = "snake_case")]` produces the exact
/// wire strings (`message_start`, `content_block_delta`, …). `event_name`
/// returns the SAME string for the `event:` line, so the single source of truth
/// is this enum value. There is no `Deserialize` (the gateway is the SERVER of
/// this dialect — it only emits these events, never parses them).
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum AnthropicStreamEventKind {
    MessageStart,
    ContentBlockStart,
    ContentBlockDelta,
    ContentBlockStop,
    MessageDelta,
    MessageStop,
    Ping,
    Error,
}

impl AnthropicStreamEventKind {
    /// The static `event:` name line value. Derived from the SAME variant the
    /// `data.type` field serializes from (via the `#[serde(rename_all)]` above),
    /// so name/type divergence is impossible at the type level.
    fn event_name(self) -> &'static str {
        match self {
            AnthropicStreamEventKind::MessageStart => "message_start",
            AnthropicStreamEventKind::ContentBlockStart => "content_block_start",
            AnthropicStreamEventKind::ContentBlockDelta => "content_block_delta",
            AnthropicStreamEventKind::ContentBlockStop => "content_block_stop",
            AnthropicStreamEventKind::MessageDelta => "message_delta",
            AnthropicStreamEventKind::MessageStop => "message_stop",
            AnthropicStreamEventKind::Ping => "ping",
            AnthropicStreamEventKind::Error => "error",
        }
    }
}

/// `message_start.message.usage`: `{input_tokens, output_tokens}`. Output is
/// always `0` at start (no tokens generated yet); the billed input estimate is
/// surfaced as `input_tokens`. Modeled as a typed struct so a wrong/missing
/// field cannot silently serialize. Fields are declared in ALPHABETICAL order
/// to keep the emitted bytes byte-for-byte identical to the BTreeMap-ordered
/// `serde_json::Value` they replace.
#[derive(Debug, Serialize)]
struct MessageStartUsage {
    input_tokens: u32,
    output_tokens: u32,
}

/// `message_delta.usage`: `{output_tokens}` ONLY — distinct from
/// [`MessageStartUsage`], which also carries `input_tokens`. Using a separate
/// typed struct makes it impossible to accidentally emit `input_tokens` here
/// (the Anthropic contract omits it on `message_delta`).
#[derive(Debug, Serialize)]
struct MessageDeltaUsage {
    output_tokens: u32,
}

// --- Per-event data payloads (each WITHOUT a `type` field; the helper injects
//     `"type"` from the event kind). Fields are declared so the emitted JSON is
//     a stable, well-typed shape; the `type` key is the helper's sole job. ---

/// The `message` object nested inside `message_start`.
#[derive(Debug, Serialize)]
struct MessageStartMessage {
    id: String,
    #[serde(rename = "type")]
    message_type: AnthropicMessageType,
    role: AnthropicAssistantRole,
    model: String,
    /// Always empty at start (no content blocks emitted yet).
    content: [(); 0],
    stop_reason: Option<String>,
    stop_sequence: Option<String>,
    usage: MessageStartUsage,
}

/// `message_start` data (minus the helper-injected `"type"`).
#[derive(Debug, Serialize)]
struct MessageStartData {
    message: MessageStartMessage,
}

/// The empty text content block opened by `content_block_start`.
#[derive(Debug, Serialize)]
struct ContentBlockStartBlock {
    #[serde(rename = "type")]
    block_type: AnthropicResponseTextBlockType,
    text: &'static str,
}

/// `content_block_start` data.
#[derive(Debug, Serialize)]
struct ContentBlockStartData {
    index: u32,
    content_block: ContentBlockStartBlock,
}

/// Pins the literal `"text_delta"` discriminant on a streamed text delta.
#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum TextDeltaType {
    TextDelta,
}

/// The `delta` object inside `content_block_delta` (a streamed text fragment).
#[derive(Debug, Serialize)]
struct ContentBlockTextDelta {
    #[serde(rename = "type")]
    delta_type: TextDeltaType,
    text: String,
}

/// `content_block_delta` data.
#[derive(Debug, Serialize)]
struct ContentBlockDeltaData {
    index: u32,
    delta: ContentBlockTextDelta,
}

/// `content_block_stop` data.
#[derive(Debug, Serialize)]
struct ContentBlockStopData {
    index: u32,
}

/// The `delta` object inside `message_delta` (carries the final stop reason).
#[derive(Debug, Serialize)]
struct MessageDeltaDelta {
    stop_reason: String,
    stop_sequence: Option<String>,
}

/// `message_delta` data.
#[derive(Debug, Serialize)]
struct MessageDeltaData {
    delta: MessageDeltaDelta,
    usage: MessageDeltaUsage,
}

/// An empty-bodied event payload (`ping` / `message_stop` carry only the
/// helper-injected `"type"`). A unit struct serializes to `{}`, into which the
/// helper's flatten injects `"type"`.
#[derive(Debug, Serialize)]
struct EmptyEventData {}

/// The `error` object inside an `error` event. The message is ALWAYS a generic,
/// gateway-authored string — never the raw provider error (S3 redaction).
#[derive(Debug, Serialize)]
struct StreamErrorBody {
    #[serde(rename = "type")]
    error_type: &'static str,
    message: &'static str,
}

/// `error` event data.
#[derive(Debug, Serialize)]
struct StreamErrorData {
    error: StreamErrorBody,
}

/// Build one Anthropic SSE event with BOTH the `event:` name line AND a `data:`
/// JSON line, as required by `@anthropic-ai/sdk` (the data JSON's `"type"` MUST
/// equal the event name).
///
/// Both the `event:` name AND the `"type"` field injected into `data` are
/// derived from the SAME `kind` value: the helper serializes `kind` for the
/// `"type"` key (callers cannot forget or mistype it) and uses `kind.event_name()`
/// for the `event:` line. Name/type divergence is therefore impossible.
///
/// `data` is any `Serialize` struct WITHOUT a `type` field; the helper injects
/// `"type": <kind>` exactly once. The final `data:` string is produced by a
/// SINGLE `serde_json::to_string` over a temporary tagged view of the typed
/// payload — the prior code first built a `serde_json::Value` tree via `json!`
/// and then re-walked it with `.to_string()`; this serializes the typed struct
/// straight to the string with no hand-built `Value` tree. `kind` is
/// gateway-controlled (never client/provider-derived), so the `event:` line can
/// never carry injected bytes.
///
/// Serialization is infallible here (every payload is plain strings/numbers/
/// nulls); on the impossible error we fail closed to an empty object rather than
/// emitting a partial frame.
fn anthropic_sse_event<T: Serialize>(kind: AnthropicStreamEventKind, data: T) -> sse::Event {
    /// A transparent tagging view: serializes `data`'s fields inline alongside a
    /// `"type"` key sourced from `kind`. Serde flattens `data`, so the output is
    /// one object with `type` + the payload's fields — produced in a single pass.
    #[derive(Serialize)]
    struct Tagged<T: Serialize> {
        #[serde(rename = "type")]
        kind: AnthropicStreamEventKind,
        #[serde(flatten)]
        data: T,
    }

    let json = serde_json::to_string(&Tagged { kind, data }).unwrap_or_else(|e| {
        // Unreachable in practice (payloads are plain data). Never emit a
        // half-built frame — fall closed to a syntactically valid empty object.
        error!(error = %e, "failed to serialize Anthropic SSE event data");
        "{}".to_string()
    });

    sse::Event::default().event(kind.event_name()).data(json)
}

/// State machine for translating the internal OpenAI `ChatChunk` stream into the
/// Anthropic Messages SSE event sequence.
///
/// The phases drive a deterministic event order:
/// `message_start` → `content_block_start` → (`content_block_delta` | `ping`)\*
/// → `content_block_stop` → `message_delta` → `message_stop`.
///
/// A mid-stream error chunk emits a single Anthropic `error` event with a
/// GENERIC message (never the raw provider error — mirrors the S3 redaction in
/// `execute_streaming_call`) and then ends the stream cleanly (no trailing
/// `content_block_stop`/`message_delta`/`message_stop` — the Anthropic SDK treats
/// an `error` event as terminal).
enum FramePhase {
    /// Have not yet emitted `message_start` + `content_block_start`.
    Preamble,
    /// Streaming `content_block_delta`/`ping` events; consuming upstream items.
    Streaming,
    /// Upstream ended normally; emit the trailer
    /// (`content_block_stop`/`message_delta`/`message_stop`).
    Trailer,
    /// Terminal — nothing more to emit.
    Done,
}

/// Per-stream framing state carried through `futures::stream::unfold`.
///
/// `async-stream` is deliberately NOT a dependency (supply-chain governance), so
/// this is a hand-rolled state machine over `unfold` with a small pending-event
/// buffer (`pending`) for phases that emit more than one event per poll.
///
/// Generic over the upstream item-stream (rather than the concrete
/// `HeartbeatStream<ChatStream>`) so the state machine can be driven by a
/// synthetic `stream::iter` of `HeartbeatItem`s in tests — deterministically
/// covering the chunk / keep-alive / error-chunk cases without timer flakiness.
/// In production, provider.rs passes the real `HeartbeatStream<ChatStream>`,
/// which IS a `Stream<Item = HeartbeatItem>`.
struct FrameState<S> {
    /// The upstream heartbeat-wrapped provider stream.
    upstream: S,
    phase: FramePhase,
    /// Buffered events not yet returned (e.g. the preamble emits two events).
    pending: VecDeque<sse::Event>,
    /// `message_start.message.id` — a freshly generated `msg_<uuid>`.
    message_id: String,
    /// The resolved request model echoed in `message_start` / `message_delta`.
    model: String,
    /// `message_start.usage.input_tokens` — the SAME input-token estimate the
    /// streaming billing path records (`estimate_input_tokens`); see the
    /// money-path note on `frame_anthropic_stream`.
    input_tokens: u32,
    /// `message_delta.usage.output_tokens` — the completion-token CEILING the
    /// billing estimate reserves (`completion_token_ceiling`). The stream carries
    /// no usage, and this is the figure the wallet is actually billed for on the
    /// streaming path, so it is the honest number to surface here.
    output_tokens: u32,
    /// The last non-null `finish_reason` seen across upstream chunks; maps to the
    /// Anthropic `stop_reason` in the trailer.
    last_finish_reason: Option<String>,
}

/// Translate a heartbeat-wrapped internal `ChatChunk` stream into the Anthropic
/// Messages SSE event sequence, passing the stream THROUGH (no buffering of the
/// whole stream).
///
/// MONEY-PATH NOTE (consulted `solvela-fintech` §3): streaming settles via the
/// ESTIMATE (the `EstimateFallback` arm in `chat/mod.rs` bills `estimated_cost`).
/// The internal `ChatChunk` stream has NO usage field, so the usage surfaced here
/// MUST be consistent with what the wallet is billed:
/// - `input_tokens` = `estimate_input_tokens(&req)` — exactly the input figure
///   the estimate prices and the streaming spend row records.
/// - `output_tokens` = `completion_token_ceiling(req.max_tokens, model_info)` —
///   the completion-token count the billing estimate RESERVES and the wallet is
///   billed for on the streaming path. It is an integer `u32` (no f64 on the
///   money path). This does not contradict the charge; it reports the reserved
///   ceiling the estimate bills, not a fabricated post-hoc count.
///
/// The caller (provider.rs) computes both with those exact functions and passes
/// them in (the adapter has no `model_info`).
///
/// `model` is the resolved request model. `message_id` is generated here as
/// `msg_<uuid>` so each streamed message carries a unique id.
///
/// `upstream` is any `Stream<Item = HeartbeatItem>` — in production the real
/// `HeartbeatStream<ChatStream>` from `fallback::stream_*`; in tests a synthetic
/// `stream::iter` of `HeartbeatItem`s.
pub fn frame_anthropic_stream<S>(
    upstream: S,
    model: String,
    input_tokens: u32,
    output_tokens: u32,
) -> impl Stream<Item = Result<sse::Event, Infallible>> + Send
where
    S: Stream<Item = HeartbeatItem> + Send + Unpin,
{
    let message_id = format!("msg_{}", uuid::Uuid::new_v4().simple());
    let state = FrameState {
        upstream,
        phase: FramePhase::Preamble,
        pending: VecDeque::new(),
        message_id,
        model,
        input_tokens,
        output_tokens,
        last_finish_reason: None,
    };

    futures::stream::unfold(state, |mut state| async move {
        loop {
            // Drain any buffered events first (preamble/trailer emit several).
            if let Some(event) = state.pending.pop_front() {
                return Some((Ok(event), state));
            }

            match state.phase {
                FramePhase::Preamble => {
                    // Emit `message_start` then `content_block_start`, BEFORE
                    // consuming any upstream chunk. `message_start.usage` carries
                    // the input-token estimate (billed) and `output_tokens: 0`.
                    let message_start = anthropic_sse_event(
                        AnthropicStreamEventKind::MessageStart,
                        MessageStartData {
                            message: MessageStartMessage {
                                id: state.message_id.clone(),
                                message_type: AnthropicMessageType::Message,
                                role: AnthropicAssistantRole::Assistant,
                                model: state.model.clone(),
                                content: [],
                                stop_reason: None,
                                stop_sequence: None,
                                // `output_tokens` is always 0 at start (no tokens
                                // generated yet); `input_tokens` is the billed
                                // input estimate.
                                usage: MessageStartUsage {
                                    input_tokens: state.input_tokens,
                                    output_tokens: 0,
                                },
                            },
                        },
                    );
                    let content_block_start = anthropic_sse_event(
                        AnthropicStreamEventKind::ContentBlockStart,
                        ContentBlockStartData {
                            index: ANTHROPIC_TEXT_BLOCK_INDEX,
                            content_block: ContentBlockStartBlock {
                                block_type: AnthropicResponseTextBlockType::Text,
                                text: "",
                            },
                        },
                    );
                    state.pending.push_back(content_block_start);
                    state.phase = FramePhase::Streaming;
                    return Some((Ok(message_start), state));
                }

                FramePhase::Streaming => match state.upstream.next().await {
                    Some(HeartbeatItem::Chunk(Ok(chunk))) => {
                        // Track the last non-null finish_reason for the trailer.
                        if let Some(reason) =
                            chunk.choices.first().and_then(|c| c.finish_reason.clone())
                        {
                            state.last_finish_reason = Some(reason);
                        }
                        // Emit a `content_block_delta` for non-empty text only.
                        // An empty/absent delta (e.g. the final finish-reason-only
                        // chunk) produces no event — loop to the next item.
                        match delta_text(&chunk) {
                            Some(text) if !text.is_empty() => {
                                let event = anthropic_sse_event(
                                    AnthropicStreamEventKind::ContentBlockDelta,
                                    ContentBlockDeltaData {
                                        index: ANTHROPIC_TEXT_BLOCK_INDEX,
                                        delta: ContentBlockTextDelta {
                                            delta_type: TextDeltaType::TextDelta,
                                            text: text.to_string(),
                                        },
                                    },
                                );
                                return Some((Ok(event), state));
                            }
                            _ => continue,
                        }
                    }
                    Some(HeartbeatItem::Chunk(Err(e))) => {
                        // S3 redaction: NEVER reflect raw provider errors. Emit a
                        // generic Anthropic `error` event and end the stream. The
                        // message is a fixed, gateway-authored string (typed below),
                        // so a provider error string can never reach the client.
                        error!(
                            error = %e,
                            "stream chunk error on /v1/messages (details redacted from client)"
                        );
                        let event = anthropic_sse_event(
                            AnthropicStreamEventKind::Error,
                            StreamErrorData {
                                error: StreamErrorBody {
                                    error_type: "api_error",
                                    message: "stream processing error",
                                },
                            },
                        );
                        state.phase = FramePhase::Done;
                        return Some((Ok(event), state));
                    }
                    Some(HeartbeatItem::KeepAlive) => {
                        // Translate the internal heartbeat into an Anthropic ping.
                        let event =
                            anthropic_sse_event(AnthropicStreamEventKind::Ping, EmptyEventData {});
                        return Some((Ok(event), state));
                    }
                    None => {
                        // Upstream ended normally → emit the trailer.
                        state.phase = FramePhase::Trailer;
                        continue;
                    }
                },

                FramePhase::Trailer => {
                    // `content_block_stop` → `message_delta` → `message_stop`.
                    // `stop_reason` defaults to "end_turn" when the upstream never
                    // reported a finish_reason.
                    let stop_reason =
                        finish_reason_to_stop_reason(state.last_finish_reason.as_deref())
                            .unwrap_or_else(|| "end_turn".to_string());

                    let content_block_stop = anthropic_sse_event(
                        AnthropicStreamEventKind::ContentBlockStop,
                        ContentBlockStopData {
                            index: ANTHROPIC_TEXT_BLOCK_INDEX,
                        },
                    );
                    let message_delta = anthropic_sse_event(
                        AnthropicStreamEventKind::MessageDelta,
                        MessageDeltaData {
                            delta: MessageDeltaDelta {
                                stop_reason,
                                stop_sequence: None,
                            },
                            // `message_delta.usage` carries ONLY `output_tokens`
                            // (the billed completion ceiling) — never `input_tokens`.
                            usage: MessageDeltaUsage {
                                output_tokens: state.output_tokens,
                            },
                        },
                    );
                    let message_stop = anthropic_sse_event(
                        AnthropicStreamEventKind::MessageStop,
                        EmptyEventData {},
                    );
                    state.pending.push_back(message_delta);
                    state.pending.push_back(message_stop);
                    state.phase = FramePhase::Done;
                    return Some((Ok(content_block_stop), state));
                }

                FramePhase::Done => return None,
            }
        }
    })
}

/// Extract the assistant text delta from a streaming `ChatChunk`, if any.
///
/// PR2 is text-only: only `choices[0].delta.content` is surfaced. Tool-call
/// deltas are ignored here (tools are rejected at the request boundary, so a
/// well-behaved stream never carries them; ignoring is defense-in-depth, not a
/// silent money-path drop).
fn delta_text(chunk: &ChatChunk) -> Option<&str> {
    chunk
        .choices
        .first()
        .and_then(|c| c.delta.content.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    use solvela_protocol::{ChatChoice, Usage};

    // -----------------------------------------------------------------------
    // Inbound request translation (golden wire vectors)
    // -----------------------------------------------------------------------

    /// A real Claude-Code-shaped request: `system` as an ARRAY of text blocks,
    /// a multi-turn user/assistant/user conversation with string content. The
    /// asserted internal `ChatRequest` is the golden vector.
    #[test]
    fn claude_code_shaped_request_maps_to_expected_chat_request() {
        let body = r#"{
            "model": "claude-sonnet-4-6",
            "max_tokens": 1024,
            "temperature": 0.7,
            "system": [
                {"type": "text", "text": "You are a coding assistant."},
                {"type": "text", "text": "Always be concise."}
            ],
            "messages": [
                {"role": "user", "content": "Write a haiku."},
                {"role": "assistant", "content": [{"type": "text", "text": "Sure."}]},
                {"role": "user", "content": [{"type": "text", "text": "About the sea."}]}
            ]
        }"#;

        let parsed: AnthropicMessagesRequest = serde_json::from_str(body).unwrap();
        let chat = anthropic_request_to_chat(parsed).unwrap();

        assert_eq!(chat.model, "claude-sonnet-4-6");
        assert_eq!(chat.max_tokens, Some(1024));
        assert_eq!(chat.temperature, Some(0.7));
        assert!(!chat.stream);

        // System (array-of-blocks) → a single leading System message, joined
        // with the "\n\n" separator the outbound adapter also uses.
        assert_eq!(chat.messages.len(), 4);
        assert_eq!(chat.messages[0].role, Role::System);
        assert_eq!(
            chat.messages[0].content.as_text(),
            "You are a coding assistant.\n\nAlways be concise."
        );

        assert_eq!(chat.messages[1].role, Role::User);
        assert_eq!(chat.messages[1].content.as_text(), "Write a haiku.");

        assert_eq!(chat.messages[2].role, Role::Assistant);
        assert_eq!(chat.messages[2].content.as_text(), "Sure.");

        assert_eq!(chat.messages[3].role, Role::User);
        assert_eq!(chat.messages[3].content.as_text(), "About the sea.");
    }

    /// `system` as a BARE STRING is accepted (the older / simpler shape).
    #[test]
    fn system_as_bare_string_maps_to_system_message() {
        let body = r#"{
            "model": "claude-sonnet-4-6",
            "max_tokens": 100,
            "system": "You are helpful.",
            "messages": [{"role": "user", "content": "hi"}]
        }"#;
        let parsed: AnthropicMessagesRequest = serde_json::from_str(body).unwrap();
        let chat = anthropic_request_to_chat(parsed).unwrap();
        assert_eq!(chat.messages.len(), 2);
        assert_eq!(chat.messages[0].role, Role::System);
        assert_eq!(chat.messages[0].content.as_text(), "You are helpful.");
        assert_eq!(chat.messages[1].role, Role::User);
    }

    /// No `system` field → no System message is prepended.
    #[test]
    fn no_system_field_omits_system_message() {
        let body = r#"{
            "model": "claude-sonnet-4-6",
            "max_tokens": 100,
            "messages": [{"role": "user", "content": "hi"}]
        }"#;
        let parsed: AnthropicMessagesRequest = serde_json::from_str(body).unwrap();
        let chat = anthropic_request_to_chat(parsed).unwrap();
        assert_eq!(chat.messages.len(), 1);
        assert_eq!(chat.messages[0].role, Role::User);
    }

    /// An empty/whitespace `system` is dropped, not prepended as an empty
    /// System message.
    #[test]
    fn empty_system_is_dropped() {
        let body = r#"{
            "model": "claude-sonnet-4-6",
            "max_tokens": 100,
            "system": "   ",
            "messages": [{"role": "user", "content": "hi"}]
        }"#;
        let parsed: AnthropicMessagesRequest = serde_json::from_str(body).unwrap();
        let chat = anthropic_request_to_chat(parsed).unwrap();
        assert_eq!(chat.messages.len(), 1);
        assert_eq!(chat.messages[0].role, Role::User);
    }

    /// Multiple text blocks in a single user message are joined with a space.
    #[test]
    fn multiple_text_blocks_join_with_space() {
        let body = r#"{
            "model": "claude-sonnet-4-6",
            "max_tokens": 100,
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": "first"},
                {"type": "text", "text": "second"}
            ]}]
        }"#;
        let parsed: AnthropicMessagesRequest = serde_json::from_str(body).unwrap();
        let chat = anthropic_request_to_chat(parsed).unwrap();
        assert_eq!(chat.messages[0].content.as_text(), "first second");
    }

    /// An image content block is rejected (PR1 is text-only) rather than
    /// silently dropped.
    #[test]
    fn image_block_is_rejected_not_dropped() {
        let body = r#"{
            "model": "claude-sonnet-4-6",
            "max_tokens": 100,
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": "what is this?"},
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "iVBOR"}}
            ]}]
        }"#;
        let parsed: AnthropicMessagesRequest = serde_json::from_str(body).unwrap();
        let err = anthropic_request_to_chat(parsed).unwrap_err();
        assert!(matches!(err, AnthropicInboundError::ImageUnsupported));
    }

    /// A `tool_use`/`tool_result` (any non-text) block is rejected (OUT OF
    /// SCOPE for PR1) rather than silently dropped — and with the TOOL-specific
    /// error, not the misleading image error.
    #[test]
    fn tool_block_is_rejected() {
        let body = r#"{
            "model": "claude-sonnet-4-6",
            "max_tokens": 100,
            "messages": [{"role": "assistant", "content": [
                {"type": "tool_use", "id": "t1", "name": "f", "input": {}}
            ]}]
        }"#;
        let parsed: AnthropicMessagesRequest = serde_json::from_str(body).unwrap();
        let err = anthropic_request_to_chat(parsed).unwrap_err();
        assert!(
            matches!(err, AnthropicInboundError::ToolUseUnsupported),
            "a tool content block must yield ToolUseUnsupported, not the image error; got {err:?}"
        );
        // The diagnostic must talk about tools, not images.
        let msg = err.to_string();
        assert!(
            msg.contains("tool use is not yet supported"),
            "tool-block error message must reference tools, got: {msg}"
        );
        assert!(
            !msg.contains("image"),
            "tool-block error message must not mislead about images, got: {msg}"
        );
    }

    /// A `tool_result` content block (the other tool block shape) is likewise
    /// rejected with the tool-specific error.
    #[test]
    fn tool_result_block_is_rejected() {
        let body = r#"{
            "model": "claude-sonnet-4-6",
            "max_tokens": 100,
            "messages": [{"role": "user", "content": [
                {"type": "tool_result", "tool_use_id": "t1", "content": "ok"}
            ]}]
        }"#;
        let parsed: AnthropicMessagesRequest = serde_json::from_str(body).unwrap();
        let err = anthropic_request_to_chat(parsed).unwrap_err();
        assert!(matches!(err, AnthropicInboundError::ToolUseUnsupported));
    }

    /// A request carrying TOP-LEVEL `tools` definitions is rejected loudly
    /// (PR1 has no tool support) rather than silently dropping the tools and
    /// billing the agent for a tool-blind text answer. Claude Code attaches
    /// these to nearly every request, so this is the common case.
    #[test]
    fn tools_definition_is_rejected() {
        let body = r#"{
            "model": "claude-sonnet-4-6",
            "max_tokens": 100,
            "tools": [
                {"name": "get_weather", "description": "Get weather",
                 "input_schema": {"type": "object", "properties": {}}}
            ],
            "messages": [{"role": "user", "content": "What is the weather?"}]
        }"#;
        let parsed: AnthropicMessagesRequest = serde_json::from_str(body).unwrap();
        // The tools array MUST deserialize into the field, not be discarded.
        assert!(
            parsed.tools.is_some(),
            "top-level `tools` must deserialize into the modeled field, not be ignored"
        );
        let err = anthropic_request_to_chat(parsed).unwrap_err();
        assert!(
            matches!(err, AnthropicInboundError::ToolUseUnsupported),
            "a top-level `tools` array must be rejected with ToolUseUnsupported; got {err:?}"
        );
    }

    /// A top-level `tool_choice` (without `tools`) is also rejected — a present
    /// value still signals the client expects tool-calling behavior PR1 does not
    /// support.
    #[test]
    fn tool_choice_is_rejected() {
        let body = r#"{
            "model": "claude-sonnet-4-6",
            "max_tokens": 100,
            "tool_choice": {"type": "auto"},
            "messages": [{"role": "user", "content": "hi"}]
        }"#;
        let parsed: AnthropicMessagesRequest = serde_json::from_str(body).unwrap();
        assert!(parsed.tool_choice.is_some());
        let err = anthropic_request_to_chat(parsed).unwrap_err();
        assert!(matches!(err, AnthropicInboundError::ToolUseUnsupported));
    }

    /// A request with NO `tools`/`tool_choice` is unaffected — the fields
    /// default to `None` and the request translates normally.
    #[test]
    fn no_tools_field_translates_normally() {
        let body = r#"{
            "model": "claude-sonnet-4-6",
            "max_tokens": 100,
            "messages": [{"role": "user", "content": "hi"}]
        }"#;
        let parsed: AnthropicMessagesRequest = serde_json::from_str(body).unwrap();
        assert!(parsed.tools.is_none());
        assert!(parsed.tool_choice.is_none());
        let chat = anthropic_request_to_chat(parsed).unwrap();
        assert_eq!(chat.messages.len(), 1);
        assert!(chat.tools.is_none());
        assert!(chat.tool_choice.is_none());
    }

    /// A `stream: true` request is now ACCEPTED (PR2) and carries the streaming
    /// flag through to the internal `ChatRequest`, so the shared core frames the
    /// Anthropic SSE event sequence. (Replaces the PR1 rejection test.)
    #[test]
    fn streaming_request_carries_through() {
        let body = r#"{
            "model": "claude-sonnet-4-6",
            "max_tokens": 100,
            "stream": true,
            "messages": [{"role": "user", "content": "hi"}]
        }"#;
        let parsed: AnthropicMessagesRequest = serde_json::from_str(body).unwrap();
        let chat = anthropic_request_to_chat(parsed).unwrap();
        assert!(
            chat.stream,
            "stream:true must carry through to the internal ChatRequest"
        );
        assert_eq!(chat.messages.len(), 1);
    }

    /// A `stream: false` (or absent) request still produces a non-streaming
    /// `ChatRequest` — the default path is unchanged.
    #[test]
    fn non_streaming_request_does_not_set_stream() {
        let body = r#"{
            "model": "claude-sonnet-4-6",
            "max_tokens": 100,
            "messages": [{"role": "user", "content": "hi"}]
        }"#;
        let parsed: AnthropicMessagesRequest = serde_json::from_str(body).unwrap();
        let chat = anthropic_request_to_chat(parsed).unwrap();
        assert!(!chat.stream, "absent stream must default to non-streaming");
    }

    /// Unknown top-level fields (e.g. `metadata`, `anthropic_beta`) are ignored
    /// — forward-compat with newer Claude Code clients.
    #[test]
    fn unknown_top_level_fields_are_ignored() {
        let body = r#"{
            "model": "claude-sonnet-4-6",
            "max_tokens": 100,
            "metadata": {"user_id": "abc"},
            "messages": [{"role": "user", "content": "hi"}]
        }"#;
        let parsed: AnthropicMessagesRequest = serde_json::from_str(body).unwrap();
        let chat = anthropic_request_to_chat(parsed).unwrap();
        assert_eq!(chat.model, "claude-sonnet-4-6");
        assert_eq!(chat.messages.len(), 1);
    }

    // -----------------------------------------------------------------------
    // Outbound response translation (byte-asserted golden wire vectors)
    // -----------------------------------------------------------------------

    fn chat_response(
        id: &str,
        model: &str,
        text: &str,
        finish_reason: Option<&str>,
        usage: Option<Usage>,
    ) -> ChatResponse {
        ChatResponse {
            id: id.to_string(),
            object: "chat.completion".to_string(),
            created: 1_700_000_000,
            model: model.to_string(),
            choices: vec![ChatChoice {
                index: 0,
                message: ChatMessage {
                    role: Role::Assistant,
                    content: text.into(),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                finish_reason: finish_reason.map(String::from),
            }],
            usage,
        }
    }

    /// Byte-exact wire shape of the Anthropic response, including the
    /// `stop` → `end_turn` stop_reason mapping and `usage` fields Claude Code
    /// reads.
    #[test]
    fn chat_response_serializes_to_expected_anthropic_shape() {
        let resp = chat_response(
            "msg_abc",
            "claude-sonnet-4-6",
            "Hello there.",
            Some("stop"),
            Some(Usage::new(10, 8)),
        );
        let anthropic = chat_response_to_anthropic(&resp);
        let v = serde_json::to_value(&anthropic).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "id": "msg_abc",
                "type": "message",
                "role": "assistant",
                "model": "claude-sonnet-4-6",
                "content": [{"type": "text", "text": "Hello there."}],
                "stop_reason": "end_turn",
                "stop_sequence": null,
                "usage": {"input_tokens": 10, "output_tokens": 8}
            })
        );
    }

    /// `finish_reason: "length"` maps to `stop_reason: "max_tokens"` (the
    /// inverse of the outbound `max_tokens` → `length` map).
    #[test]
    fn length_finish_reason_maps_to_max_tokens_stop_reason() {
        let resp = chat_response(
            "msg_len",
            "claude-sonnet-4-6",
            "truncated",
            Some("length"),
            Some(Usage::new(5, 100)),
        );
        let anthropic = chat_response_to_anthropic(&resp);
        assert_eq!(anthropic.stop_reason, Some("max_tokens".to_string()));
    }

    /// An absent `finish_reason` (provider omitted) emits `stop_reason: null`,
    /// which Claude Code tolerates.
    #[test]
    fn absent_finish_reason_emits_null_stop_reason() {
        let resp = chat_response(
            "msg_none",
            "claude-sonnet-4-6",
            "partial",
            None,
            Some(Usage::new(1, 1)),
        );
        let anthropic = chat_response_to_anthropic(&resp);
        assert_eq!(anthropic.stop_reason, None);
        let v = serde_json::to_value(&anthropic).unwrap();
        assert!(v["stop_reason"].is_null());
    }

    /// An unknown internal finish_reason passes through unchanged
    /// (forward-compat) rather than being lost.
    #[test]
    fn unknown_finish_reason_passes_through() {
        let resp = chat_response(
            "msg_x",
            "claude-sonnet-4-6",
            "x",
            Some("content_filter"),
            Some(Usage::new(1, 1)),
        );
        let anthropic = chat_response_to_anthropic(&resp);
        assert_eq!(anthropic.stop_reason, Some("content_filter".to_string()));
    }

    /// A response with NO usage emits `input_tokens: 0, output_tokens: 0`
    /// rather than failing — Claude Code reads the fields but tolerates zeros.
    #[test]
    fn missing_usage_emits_zero_token_counts() {
        let resp = chat_response("msg_nousage", "claude-sonnet-4-6", "x", Some("stop"), None);
        let anthropic = chat_response_to_anthropic(&resp);
        assert_eq!(anthropic.usage.input_tokens, 0);
        assert_eq!(anthropic.usage.output_tokens, 0);
    }

    // -----------------------------------------------------------------------
    // Streaming framer (ChatChunk stream → Anthropic SSE event sequence)
    // -----------------------------------------------------------------------

    use solvela_protocol::{ChatChunk, ChatChunkChoice, ChatDelta};

    use crate::providers::heartbeat::HeartbeatItem;
    use crate::providers::ProviderError;

    /// A text-delta chunk carrying `content` and an optional `finish_reason`.
    fn text_chunk(content: &str, finish_reason: Option<&str>) -> ChatChunk {
        ChatChunk {
            id: "chunk".to_string(),
            object: "chat.completion.chunk".to_string(),
            created: 0,
            model: "openai/gpt-4o".to_string(),
            choices: vec![ChatChunkChoice {
                index: 0,
                delta: ChatDelta {
                    role: None,
                    content: Some(content.to_string()),
                    tool_calls: None,
                },
                finish_reason: finish_reason.map(String::from),
            }],
        }
    }

    /// A finish-reason-only chunk (no content) — the common final OpenAI chunk.
    fn finish_only_chunk(finish_reason: &str) -> ChatChunk {
        ChatChunk {
            id: "chunk".to_string(),
            object: "chat.completion.chunk".to_string(),
            created: 0,
            model: "openai/gpt-4o".to_string(),
            choices: vec![ChatChunkChoice {
                index: 0,
                delta: ChatDelta {
                    role: None,
                    content: None,
                    tool_calls: None,
                },
                finish_reason: Some(finish_reason.to_string()),
            }],
        }
    }

    /// Collect a framed stream into `(event_name, data_json)` pairs.
    ///
    /// `sse::Event` exposes no public wire-rendering API, so we render through
    /// the REAL `Sse::into_response()` path (exactly as production does), collect
    /// the response body, and parse the `event:`/`data:` SSE lines. This asserts
    /// BOTH lines are present (the @anthropic-ai/sdk requirement) on the actual
    /// transmitted bytes, not on an introspected struct.
    async fn frame_to_pairs(
        items: Vec<HeartbeatItem>,
        model: &str,
        input_tokens: u32,
        output_tokens: u32,
    ) -> Vec<(String, serde_json::Value)> {
        use axum::response::{IntoResponse, Sse};

        let upstream = futures::stream::iter(items);
        let framed =
            frame_anthropic_stream(upstream, model.to_string(), input_tokens, output_tokens);
        let response = Sse::new(framed).into_response();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("collect SSE body");
        let wire = String::from_utf8(bytes.to_vec()).expect("SSE body is UTF-8");

        // SSE frames are separated by a blank line. Each frame here has an
        // `event:` line and a `data:` line.
        wire.split("\n\n")
            .filter(|frame| !frame.trim().is_empty())
            .map(|frame| {
                let mut name = None;
                let mut data = None;
                for line in frame.lines() {
                    if let Some(rest) = line.strip_prefix("event:") {
                        name = Some(rest.trim().to_string());
                    } else if let Some(rest) = line.strip_prefix("data:") {
                        data = Some(rest.trim().to_string());
                    }
                }
                let name = name.expect("every Anthropic SSE event must set an event: name line");
                let data = data.expect("every Anthropic SSE event must set a data: line");
                let json: serde_json::Value =
                    serde_json::from_str(&data).expect("data line must be valid JSON");
                (name, json)
            })
            .collect()
    }

    /// A single text chunk with a `stop` finish_reason produces the full, ordered
    /// Anthropic event sequence with the exact field shapes the SDK requires.
    #[tokio::test]
    async fn framer_emits_full_ordered_sequence_for_text_stream() {
        let items = vec![HeartbeatItem::Chunk(Ok(text_chunk("Hello", Some("stop"))))];
        let pairs = frame_to_pairs(items, "openai/gpt-4o", 7, 9).await;

        let names: Vec<&str> = pairs.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "message_start",
                "content_block_start",
                "content_block_delta",
                "content_block_stop",
                "message_delta",
                "message_stop",
            ],
            "Anthropic streaming event order must match the SDK contract"
        );

        // The data JSON's "type" must equal the event name on every frame.
        for (name, data) in &pairs {
            assert_eq!(
                data["type"], *name,
                "data.type must equal the event name for {name}"
            );
        }

        // message_start: usage carries the input-token estimate + output 0.
        let (_, ms) = &pairs[0];
        assert_eq!(ms["message"]["role"], "assistant");
        assert_eq!(ms["message"]["model"], "openai/gpt-4o");
        assert_eq!(ms["message"]["content"], serde_json::json!([]));
        assert!(ms["message"]["stop_reason"].is_null());
        assert_eq!(ms["message"]["usage"]["input_tokens"], 7);
        assert_eq!(ms["message"]["usage"]["output_tokens"], 0);
        let id = ms["message"]["id"].as_str().unwrap();
        assert!(
            id.starts_with("msg_"),
            "message id must be msg_<uuid>, got {id}"
        );

        // content_block_start: empty text block at index 0.
        let (_, cbs) = &pairs[1];
        assert_eq!(cbs["index"], 0);
        assert_eq!(
            cbs["content_block"],
            serde_json::json!({"type":"text","text":""})
        );

        // content_block_delta: the text_delta carries the chunk content.
        let (_, cbd) = &pairs[2];
        assert_eq!(cbd["index"], 0);
        assert_eq!(
            cbd["delta"],
            serde_json::json!({"type":"text_delta","text":"Hello"})
        );

        // message_delta: mapped stop_reason ("stop" → "end_turn") + output ceiling.
        let (_, md) = &pairs[4];
        assert_eq!(md["delta"]["stop_reason"], "end_turn");
        assert!(md["delta"]["stop_sequence"].is_null());
        assert_eq!(
            md["usage"]["output_tokens"], 9,
            "message_delta must report the billed completion-token ceiling"
        );
    }

    /// Multiple content chunks each yield one content_block_delta, in order; a
    /// trailing finish-reason-only chunk yields NO delta but sets the stop_reason.
    #[tokio::test]
    async fn framer_streams_multiple_deltas_and_maps_length_stop_reason() {
        let items = vec![
            HeartbeatItem::Chunk(Ok(text_chunk("Hel", None))),
            HeartbeatItem::Chunk(Ok(text_chunk("lo", None))),
            HeartbeatItem::Chunk(Ok(finish_only_chunk("length"))),
        ];
        let pairs = frame_to_pairs(items, "openai/gpt-4o", 3, 100).await;

        let deltas: Vec<&str> = pairs
            .iter()
            .filter(|(n, _)| n == "content_block_delta")
            .map(|(_, d)| d["delta"]["text"].as_str().unwrap())
            .collect();
        assert_eq!(
            deltas,
            vec!["Hel", "lo"],
            "each non-empty content chunk yields one ordered text_delta; \
             the finish-only chunk yields none"
        );

        let (_, md) = pairs.iter().find(|(n, _)| n == "message_delta").unwrap();
        assert_eq!(
            md["delta"]["stop_reason"], "max_tokens",
            "length finish_reason must map to the max_tokens stop_reason"
        );
    }

    /// A keep-alive heartbeat is translated into an Anthropic `ping` event,
    /// interleaved in order between deltas.
    #[tokio::test]
    async fn framer_translates_keepalive_to_ping() {
        let items = vec![
            HeartbeatItem::Chunk(Ok(text_chunk("a", None))),
            HeartbeatItem::KeepAlive,
            HeartbeatItem::Chunk(Ok(text_chunk("b", Some("stop")))),
        ];
        let pairs = frame_to_pairs(items, "openai/gpt-4o", 1, 5).await;
        let names: Vec<&str> = pairs.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "message_start",
                "content_block_start",
                "content_block_delta", // "a"
                "ping",                // keep-alive
                "content_block_delta", // "b"
                "content_block_stop",
                "message_delta",
                "message_stop",
            ]
        );
        let (_, ping) = pairs.iter().find(|(n, _)| n == "ping").unwrap();
        assert_eq!(*ping, serde_json::json!({"type": "ping"}));
    }

    /// A mid-stream error chunk emits a single Anthropic `error` event with a
    /// GENERIC, REDACTED message (never the raw provider error) and ends the
    /// stream cleanly (no trailing content_block_stop/message_delta/message_stop).
    #[tokio::test]
    async fn framer_redacts_error_chunk_and_ends_stream() {
        let raw = "OpenAI 500: api key sk-secret-LEAK rate exceeded at https://internal";
        let err: ProviderError = raw.into();
        let items = vec![
            HeartbeatItem::Chunk(Ok(text_chunk("partial", None))),
            HeartbeatItem::Chunk(Err(err)),
            // Anything after the error must never be emitted.
            HeartbeatItem::Chunk(Ok(text_chunk("should-not-appear", Some("stop")))),
        ];
        let pairs = frame_to_pairs(items, "openai/gpt-4o", 1, 5).await;
        let names: Vec<&str> = pairs.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "message_start",
                "content_block_start",
                "content_block_delta",
                "error",
            ],
            "an error chunk must terminate the stream after a generic error event"
        );

        let (_, err) = pairs.last().unwrap();
        assert_eq!(err["error"]["type"], "api_error");
        assert_eq!(err["error"]["message"], "stream processing error");
        // The redacted message must NOT leak any of the raw provider error.
        let serialized = err.to_string();
        assert!(
            !serialized.contains("sk-secret-LEAK")
                && !serialized.contains("internal")
                && !serialized.contains("500"),
            "the error event must not reflect the raw provider error: {serialized}"
        );
        // The post-error chunk must not have produced a delta.
        assert!(
            !pairs
                .iter()
                .any(|(_, d)| d["delta"]["text"] == "should-not-appear"),
            "no events may be emitted after the terminal error event"
        );
    }

    /// An empty upstream (no chunks at all) still produces a well-formed sequence
    /// with the default `end_turn` stop_reason (no finish_reason was seen).
    #[tokio::test]
    async fn framer_empty_stream_defaults_to_end_turn() {
        let pairs = frame_to_pairs(vec![], "openai/gpt-4o", 4, 8).await;
        let names: Vec<&str> = pairs.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "message_start",
                "content_block_start",
                "content_block_stop",
                "message_delta",
                "message_stop",
            ]
        );
        let (_, md) = pairs.iter().find(|(n, _)| n == "message_delta").unwrap();
        assert_eq!(
            md["delta"]["stop_reason"], "end_turn",
            "absent finish_reason must default the stop_reason to end_turn"
        );
        assert_eq!(md["usage"]["output_tokens"], 8);
    }

    /// T6: an UNKNOWN provider `finish_reason` (e.g. `content_filter`) on the
    /// STREAMING path passes through unchanged into the trailer
    /// `message_delta.stop_reason` — the streaming-side lock for the same
    /// provider-boundary passthrough the non-streaming
    /// `unknown_finish_reason_passes_through` test pins. The provider boundary is
    /// trusted (registry-controlled adapters), so an unrecognized reason is
    /// forwarded verbatim rather than coerced or dropped.
    #[tokio::test]
    async fn framer_unknown_finish_reason_passes_through() {
        let items = vec![HeartbeatItem::Chunk(Ok(text_chunk(
            "x",
            Some("content_filter"),
        )))];
        let pairs = frame_to_pairs(items, "openai/gpt-4o", 1, 5).await;
        let (_, md) = pairs.iter().find(|(n, _)| n == "message_delta").unwrap();
        assert_eq!(
            md["delta"]["stop_reason"], "content_filter",
            "an unknown finish_reason must pass through to stop_reason unchanged \
             (provider boundary is trusted/registry-controlled)"
        );
    }

    /// An error as the FIRST upstream item (no preceding Ok chunk) must STILL be
    /// preceded by the `message_start` + `content_block_start` preamble before the
    /// redacted `error` event — the `@anthropic-ai/sdk` requires the preamble
    /// ahead of any error event, even when the upstream fails immediately.
    #[tokio::test]
    async fn framer_error_on_first_chunk_still_emits_preamble() {
        let err: ProviderError = "OpenAI 500: sk-secret-LEAK".into();
        let items = vec![HeartbeatItem::Chunk(Err(err))];
        let pairs = frame_to_pairs(items, "openai/gpt-4o", 1, 5).await;
        let names: Vec<&str> = pairs.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            vec!["message_start", "content_block_start", "error"],
            "an immediate error must still be preceded by the preamble events"
        );
        // The error stays generic/redacted even on the first-item path.
        let (_, err_ev) = pairs.last().unwrap();
        assert_eq!(err_ev["type"], "error");
        assert_eq!(err_ev["error"]["message"], "stream processing error");
        assert!(
            !err_ev.to_string().contains("sk-secret-LEAK"),
            "the error event must not leak the raw provider error"
        );
    }

    /// Byte-shape lock for `anthropic_sse_event`: the helper derives BOTH the
    /// `event:` name AND the data JSON's `"type"` from the SAME kind, injecting
    /// `"type"` exactly once. This pins the exact emitted SSE wire bytes of a
    /// representative frame so a future change to the typed payloads / helper that
    /// alters the wire shape is caught. Asserting on the literal bytes (not parsed
    /// JSON) is what makes name/type single-source-of-truth and the typed usage
    /// shapes observable at the wire level.
    #[tokio::test]
    async fn framer_message_start_and_delta_have_exact_typed_wire_shape() {
        use axum::response::{IntoResponse, Sse};

        let items = vec![HeartbeatItem::Chunk(Ok(text_chunk("Hi", Some("stop"))))];
        let upstream = futures::stream::iter(items);
        let framed = frame_anthropic_stream(upstream, "openai/gpt-4o".to_string(), 7, 9);
        let response = Sse::new(framed).into_response();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("collect SSE body");
        let wire = String::from_utf8(bytes.to_vec()).expect("SSE body is UTF-8");

        // The `event:` name and the data `"type"` are produced from the same kind
        // — assert both the name line and the matching `"type"` appear together
        // for message_start, with the typed usage shape (input + output:0).
        assert!(
            wire.contains("event: message_start\n")
                && wire.contains("data: {\"type\":\"message_start\",\"message\":{\"id\":"),
            "message_start frame must carry the event name line and a type-tagged \
             data line:\n{wire}"
        );
        // message_start.usage is the {input_tokens, output_tokens} typed shape
        // with output always 0 at start.
        assert!(
            wire.contains("\"usage\":{\"input_tokens\":7,\"output_tokens\":0}}}"),
            "message_start.usage must be the typed {{input_tokens,output_tokens}} \
             shape with output 0:\n{wire}"
        );
        // message_delta.usage is the DISTINCT {output_tokens}-only typed shape
        // (no input_tokens), carrying the billed completion ceiling.
        assert!(
            wire.contains("event: message_delta\n")
                && wire.contains("\"usage\":{\"output_tokens\":9}"),
            "message_delta.usage must be the {{output_tokens}}-only typed shape:\n{wire}"
        );
        // The message_delta data must NOT carry input_tokens (separate usage type).
        let md_frame = wire
            .split("\n\n")
            .find(|f| f.contains("event: message_delta"))
            .expect("message_delta frame present");
        assert!(
            !md_frame.contains("input_tokens"),
            "message_delta.usage must omit input_tokens:\n{md_frame}"
        );
    }
}
