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
//! SCOPE (PR1): text-only, non-streaming. Streaming (SSE), tools (top-level
//! `tools`/`tool_choice` definitions AND `tool_use`/`tool_result` content
//! blocks), `count_tokens`, and images are OUT OF SCOPE. Every one of these is
//! rejected LOUDLY rather than silently dropped: a dropped image or a discarded
//! tool definition would change the prompt's meaning (or strip tool-calling)
//! while still billing the agent — the same fail-loud posture the outbound
//! adapter takes for system/tool-role images. Tool definitions in particular
//! must be modeled on the wire (`AnthropicMessagesRequest::tools`), not left to
//! serde's ignore-unknown-fields default, because Claude Code attaches them to
//! nearly every request.

use serde::{Deserialize, Serialize};

use solvela_protocol::{ChatMessage, ChatRequest, ChatResponse, MessageContent, Role};

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
    if req.stream {
        return Err(AnthropicInboundError::InvalidBody(
            "streaming responses are not yet supported on POST /v1/messages; \
             set \"stream\": false (SSE support is planned for a later release)"
                .to_string(),
        ));
    }

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
        stream: false,
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

    /// A `stream: true` request is rejected (PR1 is non-streaming) rather than
    /// silently served as a single JSON body.
    #[test]
    fn streaming_request_is_rejected() {
        let body = r#"{
            "model": "claude-sonnet-4-6",
            "max_tokens": 100,
            "stream": true,
            "messages": [{"role": "user", "content": "hi"}]
        }"#;
        let parsed: AnthropicMessagesRequest = serde_json::from_str(body).unwrap();
        let err = anthropic_request_to_chat(parsed).unwrap_err();
        assert!(matches!(err, AnthropicInboundError::InvalidBody(_)));
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
}
