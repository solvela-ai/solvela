use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::warn;

use solvela_protocol::{
    ChatChoice, ChatChunk, ChatChunkChoice, ChatDelta, ChatMessage, ChatRequest, ChatResponse,
    MessageContent, ModelRegistration, ParseImageError, ParsedImage, Role, Usage,
};

use super::{ChatStream, LLMProvider, ProviderError};

/// Anthropic provider adapter.
///
/// Translates between OpenAI format and Anthropic's Messages API format.
/// Key differences:
/// - System message is a separate top-level `system` parameter
/// - Messages array only contains `user` and `assistant` roles
/// - Response has `content` array with text blocks instead of a single string
/// - Model ID uses Anthropic naming (e.g., "claude-sonnet-4-20250514")
pub struct AnthropicProvider {
    api_key: String,
    client: reqwest::Client,
}

impl AnthropicProvider {
    pub fn new(client: reqwest::Client, api_key: String) -> Self {
        Self { api_key, client }
    }
}

// ---------------------------------------------------------------------------
// Anthropic Messages API request/response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream: Option<bool>,
}

/// Anthropic message content: a list of content blocks.
///
/// Anthropic's Messages API accepts `content` as either a bare string or an
/// array of typed content blocks. We always emit the array form so a single
/// code path carries both text-only and multimodal messages. For text-only
/// messages this is a one-element `[{"type":"text","text":...}]`, which the
/// API treats identically to a bare string.
///
/// Wire schema verified against the Anthropic Messages API docs
/// (platform.claude.com/docs/en/api/messages, fetched 2026-06-03):
///   text:  {"type":"text","text": "..."}
///   image (base64): {"type":"image","source":{"type":"base64",
///                     "media_type":"image/png","data":"<b64>"}}
///   image (url):    {"type":"image","source":{"type":"url",
///                     "url":"https://..."}}
/// Outbound-only (request) — no `Deserialize`.
#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: String,
    content: Vec<AnthropicContentBlockOut>,
}

/// Outbound-only (request) — no `Deserialize`.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicContentBlockOut {
    Text { text: String },
    Image { source: AnthropicImageSource },
}

/// Outbound-only (request) — no `Deserialize`.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicImageSource {
    Base64 { media_type: String, data: String },
    Url { url: String },
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    id: String,
    #[allow(dead_code)]
    model: String,
    content: Vec<AnthropicContentBlock>,
    stop_reason: Option<String>,
    usage: AnthropicUsage,
}

#[derive(Debug, Deserialize)]
struct AnthropicContentBlock {
    #[serde(rename = "type")]
    content_type: String,
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
}

// ---------------------------------------------------------------------------
// Format translation
// ---------------------------------------------------------------------------

/// Translate one OpenAI [`MessageContent`] into Anthropic content blocks.
///
/// Text-only content becomes a single text block (the API treats a
/// one-element text array identically to a bare string). Multimodal content
/// preserves part ORDER — interleaving of text and images is meaningful to
/// the model — and translates each image via [`ImageUrl::parse`]
/// (`data:` URI → base64 source; http(s) → url source). A malformed image
/// URL returns `Err` so the request is rejected rather than silently dropping
/// the image.
fn content_to_anthropic_blocks(
    content: &MessageContent,
) -> Result<Vec<AnthropicContentBlockOut>, String> {
    match content {
        MessageContent::Text(s) => Ok(vec![AnthropicContentBlockOut::Text { text: s.clone() }]),
        MessageContent::Parts(parts) => {
            let mut blocks = Vec::with_capacity(parts.len());
            for part in parts {
                match part {
                    solvela_protocol::ContentPart::Text { text } => {
                        blocks.push(AnthropicContentBlockOut::Text { text: text.clone() });
                    }
                    solvela_protocol::ContentPart::ImageUrl { image_url } => {
                        let source = match image_url
                            .parse()
                            .map_err(|e: ParseImageError| e.to_string())?
                        {
                            ParsedImage::Base64 { media_type, data } => {
                                AnthropicImageSource::Base64 { media_type, data }
                            }
                            ParsedImage::Url { url } => AnthropicImageSource::Url { url },
                        };
                        blocks.push(AnthropicContentBlockOut::Image { source });
                    }
                }
            }
            Ok(blocks)
        }
    }
}

/// Convert an OpenAI-format request to Anthropic Messages API format.
///
/// Fallible: a malformed image data URI in any user/assistant message
/// surfaces as `Err` rather than dropping the image. System messages are
/// always flattened to text (Anthropic's `system` param is a plain string).
fn to_anthropic_request(req: &ChatRequest) -> Result<AnthropicRequest, String> {
    // Extract system message(s) — Anthropic takes system as a separate param
    // (a plain string), so it cannot carry image blocks. An image in a
    // system/developer message would be silently dropped by `as_text()` while
    // the vision gate still accepts the request — the agent pays but the model
    // never sees the image. Reject it explicitly instead.
    let system: Option<String> = {
        let mut system_msgs: Vec<String> = Vec::new();
        for m in req
            .messages
            .iter()
            .filter(|m| m.role == Role::System || m.role == Role::Developer)
        {
            if m.content.has_image_parts() {
                return Err(
                    "image content is not supported in system/developer messages; \
                     place images in a user message"
                        .to_string(),
                );
            }
            system_msgs.push(m.content.as_text().into_owned());
        }

        if system_msgs.is_empty() {
            None
        } else {
            Some(system_msgs.join("\n\n"))
        }
    };

    // Filter to user/assistant messages only
    let mut messages: Vec<AnthropicMessage> = Vec::new();
    for m in req
        .messages
        .iter()
        .filter(|m| m.role == Role::User || m.role == Role::Assistant)
    {
        let role = match m.role {
            Role::User => "user".to_string(),
            Role::Assistant => "assistant".to_string(),
            _ => "user".to_string(), // shouldn't happen due to filter
        };
        let content = content_to_anthropic_blocks(&m.content)?;
        messages.push(AnthropicMessage { role, content });
    }

    // Extract model_id part (e.g., "anthropic/claude-sonnet-4-20250514" → "claude-sonnet-4-20250514")
    let model = req
        .model
        .strip_prefix("anthropic/")
        .unwrap_or(&req.model)
        .to_string();

    Ok(AnthropicRequest {
        model,
        max_tokens: req.max_tokens.unwrap_or(4096),
        system,
        messages,
        temperature: req.temperature,
        top_p: req.top_p,
        stream: None,
    })
}

/// Convert an Anthropic response to OpenAI-format ChatResponse.
fn from_anthropic_response(resp: AnthropicResponse, original_model: &str) -> ChatResponse {
    // Concatenate all text content blocks
    let content: String = resp
        .content
        .iter()
        .filter(|b| b.content_type == "text")
        .filter_map(|b| b.text.as_deref())
        .collect::<Vec<_>>()
        .join("");

    let finish_reason = resp.stop_reason.map(|r| match r.as_str() {
        "end_turn" => "stop".to_string(),
        "max_tokens" => "length".to_string(),
        "stop_sequence" => "stop".to_string(),
        other => other.to_string(),
    });

    ChatResponse {
        id: resp.id,
        object: "chat.completion".to_string(),
        created: chrono::Utc::now().timestamp(),
        model: original_model.to_string(),
        choices: vec![ChatChoice {
            index: 0,
            message: ChatMessage {
                role: Role::Assistant,
                content: content.into(),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            },
            finish_reason,
        }],
        // `Usage::new` does the saturating add for total_tokens. A plain
        // `prompt + completion` panics in debug builds on overflow and
        // silently wraps in release; the gateway billing path
        // (`cap_usage_to_request_limits`) reads `total_tokens` directly,
        // so a wrapped value would propagate into spend tracking.
        usage: Some(Usage::new(
            resp.usage.input_tokens,
            resp.usage.output_tokens,
        )),
    }
}

// ---------------------------------------------------------------------------
// Anthropic SSE event types for streaming
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct AnthropicMessageStart {
    message: AnthropicMessageStartBody,
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageStartBody {
    id: String,
    model: String,
}

#[derive(Debug, Deserialize)]
struct AnthropicContentBlockDelta {
    delta: AnthropicTextDelta,
}

#[derive(Debug, Deserialize)]
struct AnthropicTextDelta {
    #[serde(default)]
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageDelta {
    delta: AnthropicMessageDeltaBody,
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageDeltaBody {
    stop_reason: Option<String>,
}

/// Spawn an SSE parser for Anthropic streaming responses.
///
/// Anthropic SSE events use both `event:` and `data:` lines. This parser
/// translates them into OpenAI-format `ChatChunk` events.
fn spawn_anthropic_sse_parser(response: reqwest::Response, model: String) -> ChatStream {
    let (mut tx, rx) = futures::channel::mpsc::channel::<Result<ChatChunk, ProviderError>>(32);
    tokio::spawn(async move {
        use futures::{SinkExt, StreamExt};

        let mut byte_stream = response.bytes_stream();
        let mut buffer = String::new();
        let mut message_id = String::new();
        let created = chrono::Utc::now().timestamp();

        while let Some(chunk_result) = byte_stream.next().await {
            match chunk_result {
                Ok(bytes) => {
                    buffer.push_str(&String::from_utf8_lossy(&bytes));

                    while let Some(pos) = buffer.find("\n\n") {
                        let event_block = buffer[..pos].to_string();
                        buffer.drain(..pos + 2);

                        let mut event_type = None;
                        let mut data_str = None;

                        for line in event_block.lines() {
                            if let Some(et) = line.strip_prefix("event: ") {
                                event_type = Some(et.trim().to_string());
                            } else if let Some(d) = line.strip_prefix("data: ") {
                                data_str = Some(d.trim().to_string());
                            }
                        }

                        let (Some(event_type), Some(data)) = (event_type, data_str) else {
                            continue;
                        };

                        match event_type.as_str() {
                            "message_start" => {
                                match serde_json::from_str::<AnthropicMessageStart>(&data) {
                                    Ok(msg) => {
                                        message_id = msg.message.id.clone();
                                        let chunk = ChatChunk {
                                            id: msg.message.id,
                                            object: "chat.completion.chunk".to_string(),
                                            created,
                                            model: msg.message.model,
                                            choices: vec![ChatChunkChoice {
                                                index: 0,
                                                delta: ChatDelta {
                                                    role: Some(Role::Assistant),
                                                    content: None,
                                                    tool_calls: None,
                                                },
                                                finish_reason: None,
                                            }],
                                        };
                                        if tx.send(Ok(chunk)).await.is_err() {
                                            return;
                                        }
                                    }
                                    Err(e) => {
                                        let truncated: String = data.chars().take(200).collect();
                                        warn!(
                                            error = %e,
                                            raw_data = %truncated,
                                            "anthropic_stream_parse_error: failed to parse message_start event"
                                        );
                                    }
                                }
                            }
                            "content_block_delta" => {
                                match serde_json::from_str::<AnthropicContentBlockDelta>(&data) {
                                    Ok(cbd) => {
                                        let chunk = ChatChunk {
                                            id: message_id.clone(),
                                            object: "chat.completion.chunk".to_string(),
                                            created,
                                            model: model.clone(),
                                            choices: vec![ChatChunkChoice {
                                                index: 0,
                                                delta: ChatDelta {
                                                    role: None,
                                                    content: cbd.delta.text,
                                                    tool_calls: None,
                                                },
                                                finish_reason: None,
                                            }],
                                        };
                                        if tx.send(Ok(chunk)).await.is_err() {
                                            return;
                                        }
                                    }
                                    Err(e) => {
                                        // Content delta parse failure — forward a best-effort
                                        // chunk with the raw data so the client receives
                                        // something rather than a silent gap in the stream.
                                        let truncated: String = data.chars().take(200).collect();
                                        warn!(
                                            error = %e,
                                            raw_data = %truncated,
                                            "anthropic_stream_parse_error: failed to parse content_block_delta, forwarding raw text"
                                        );
                                        let chunk = ChatChunk {
                                            id: message_id.clone(),
                                            object: "chat.completion.chunk".to_string(),
                                            created,
                                            model: model.clone(),
                                            choices: vec![ChatChunkChoice {
                                                index: 0,
                                                delta: ChatDelta {
                                                    role: None,
                                                    content: Some(
                                                        "[parse error: content may be incomplete]"
                                                            .to_string(),
                                                    ),
                                                    tool_calls: None,
                                                },
                                                finish_reason: None,
                                            }],
                                        };
                                        if tx.send(Ok(chunk)).await.is_err() {
                                            return;
                                        }
                                    }
                                }
                            }
                            "message_delta" => {
                                match serde_json::from_str::<AnthropicMessageDelta>(&data) {
                                    Ok(md) => {
                                        let finish_reason =
                                            md.delta.stop_reason.map(|r| match r.as_str() {
                                                "end_turn" => "stop".to_string(),
                                                "max_tokens" => "length".to_string(),
                                                "stop_sequence" => "stop".to_string(),
                                                other => other.to_string(),
                                            });
                                        let chunk = ChatChunk {
                                            id: message_id.clone(),
                                            object: "chat.completion.chunk".to_string(),
                                            created,
                                            model: model.clone(),
                                            choices: vec![ChatChunkChoice {
                                                index: 0,
                                                delta: ChatDelta {
                                                    role: None,
                                                    content: None,
                                                    tool_calls: None,
                                                },
                                                finish_reason,
                                            }],
                                        };
                                        if tx.send(Ok(chunk)).await.is_err() {
                                            return;
                                        }
                                    }
                                    Err(e) => {
                                        let truncated: String = data.chars().take(200).collect();
                                        warn!(
                                            error = %e,
                                            raw_data = %truncated,
                                            "anthropic_stream_parse_error: failed to parse message_delta event"
                                        );
                                    }
                                }
                            }
                            "message_stop" => {
                                return;
                            }
                            _ => {
                                // Ignore other event types (ping, content_block_start, etc.)
                            }
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(Box::new(e) as ProviderError)).await;
                    return;
                }
            }
        }
    });
    Box::pin(rx)
}

#[async_trait]
impl LLMProvider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn supported_models(&self) -> Vec<ModelRegistration> {
        vec![]
    }

    async fn chat_completion(
        &self,
        req: ChatRequest,
    ) -> Result<ChatResponse, Box<dyn std::error::Error + Send + Sync>> {
        let original_model = req.model.clone();
        let anthropic_req = to_anthropic_request(&req)
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() })?;

        let req_body = serde_json::to_value(&anthropic_req)?;
        let response = super::retry_with_backoff(2, || {
            self.client
                .post("https://api.anthropic.com/v1/messages")
                .timeout(std::time::Duration::from_secs(90))
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .header("content-type", "application/json")
                .json(&req_body)
                .send()
        })
        .await?;

        let anthropic_resp = response
            .error_for_status()?
            .json::<AnthropicResponse>()
            .await?;

        Ok(from_anthropic_response(anthropic_resp, &original_model))
    }

    async fn chat_completion_stream(&self, req: ChatRequest) -> Result<ChatStream, ProviderError> {
        let model = req.model.clone();
        let mut anthropic_req = to_anthropic_request(&req).map_err(|e| -> ProviderError {
            // Surface the malformed-image error rather than dropping the image.
            e.into()
        })?;
        anthropic_req.stream = Some(true);

        let response = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .timeout(std::time::Duration::from_secs(90))
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&anthropic_req)
            .send()
            .await?
            .error_for_status()?;

        Ok(spawn_anthropic_sse_parser(response, model))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: the text of a single text block, or panic.
    fn block_text(block: &AnthropicContentBlockOut) -> &str {
        match block {
            AnthropicContentBlockOut::Text { text } => text,
            AnthropicContentBlockOut::Image { .. } => panic!("expected a text block"),
        }
    }

    #[test]
    fn test_user_text_parts_become_separate_text_blocks() {
        use solvela_protocol::vision::{ContentPart, MessageContent};
        let req = ChatRequest {
            model: "anthropic/claude-sonnet-4-20250514".to_string(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Parts(vec![
                    ContentPart::Text {
                        text: "first".to_string(),
                    },
                    ContentPart::Text {
                        text: "second".to_string(),
                    },
                ]),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            max_tokens: Some(100),
            temperature: None,
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
        };

        let anthropic_req = to_anthropic_request(&req).unwrap();
        assert_eq!(anthropic_req.messages.len(), 1);
        // Each text part maps to its own text block, preserving order.
        assert_eq!(anthropic_req.messages[0].content.len(), 2);
        assert_eq!(block_text(&anthropic_req.messages[0].content[0]), "first");
        assert_eq!(block_text(&anthropic_req.messages[0].content[1]), "second");
    }

    #[test]
    fn test_text_only_string_message_is_single_text_block() {
        let req = ChatRequest {
            model: "anthropic/claude-sonnet-4-20250514".to_string(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: "hello".into(),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            max_tokens: Some(100),
            temperature: None,
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
        };
        let anthropic_req = to_anthropic_request(&req).unwrap();
        // Wire-shape pin: a one-element text array, which the API treats
        // identically to a bare string.
        let v = serde_json::to_value(&anthropic_req.messages[0]).unwrap();
        assert_eq!(
            v["content"],
            serde_json::json!([{"type":"text","text":"hello"}])
        );
    }

    #[test]
    fn test_user_image_data_uri_becomes_base64_block() {
        use solvela_protocol::vision::{ContentPart, ImageUrl, MessageContent};
        let req = ChatRequest {
            model: "anthropic/claude-sonnet-4-6".to_string(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Parts(vec![
                    ContentPart::Text {
                        text: "what is this?".to_string(),
                    },
                    ContentPart::ImageUrl {
                        image_url: ImageUrl {
                            url: "data:image/png;base64,iVBORw0KGgo=".to_string(),
                            detail: None,
                        },
                    },
                ]),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            max_tokens: Some(100),
            temperature: None,
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
        };
        let anthropic_req = to_anthropic_request(&req).unwrap();
        // Exact wire shape per Anthropic Messages API (verified 2026-06-03).
        let v = serde_json::to_value(&anthropic_req.messages[0]).unwrap();
        assert_eq!(
            v["content"],
            serde_json::json!([
                {"type":"text","text":"what is this?"},
                {"type":"image","source":{
                    "type":"base64","media_type":"image/png","data":"iVBORw0KGgo="
                }}
            ])
        );
    }

    #[test]
    fn test_user_image_http_url_becomes_url_block() {
        use solvela_protocol::vision::{ContentPart, ImageUrl, MessageContent};
        let req = ChatRequest {
            model: "anthropic/claude-sonnet-4-6".to_string(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Parts(vec![ContentPart::ImageUrl {
                    image_url: ImageUrl {
                        url: "https://example.com/cat.png".to_string(),
                        detail: Some("high".to_string()),
                    },
                }]),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            max_tokens: Some(100),
            temperature: None,
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
        };
        let anthropic_req = to_anthropic_request(&req).unwrap();
        let v = serde_json::to_value(&anthropic_req.messages[0]).unwrap();
        assert_eq!(
            v["content"],
            serde_json::json!([
                {"type":"image","source":{
                    "type":"url","url":"https://example.com/cat.png"
                }}
            ])
        );
    }

    #[test]
    fn test_malformed_image_data_uri_is_rejected_not_dropped() {
        use solvela_protocol::vision::{ContentPart, ImageUrl, MessageContent};
        let req = ChatRequest {
            model: "anthropic/claude-sonnet-4-6".to_string(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Parts(vec![ContentPart::ImageUrl {
                    image_url: ImageUrl {
                        // No scheme — neither data: nor http(s).
                        url: "not-a-valid-image".to_string(),
                        detail: None,
                    },
                }]),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            max_tokens: Some(100),
            temperature: None,
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
        };
        assert!(
            to_anthropic_request(&req).is_err(),
            "a malformed image URL must reject the request, not silently drop the image"
        );
    }

    #[test]
    fn test_data_uri_without_base64_rejected_at_adapter() {
        use solvela_protocol::vision::{ContentPart, ImageUrl, MessageContent};
        // A `data:` URI lacking the `;base64` token (URL-encoded inline text)
        // must be rejected at the ADAPTER level, not only the protocol unit
        // test — forwarding it would corrupt the image source.
        let req = ChatRequest {
            model: "anthropic/claude-sonnet-4-6".to_string(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: MessageContent::Parts(vec![ContentPart::ImageUrl {
                    image_url: ImageUrl {
                        url: "data:image/png,rawnotbase64".to_string(),
                        detail: None,
                    },
                }]),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            max_tokens: Some(100),
            temperature: None,
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
        };
        assert!(
            to_anthropic_request(&req).is_err(),
            "a non-base64 data URI must be rejected at the adapter"
        );
    }

    #[test]
    fn test_image_in_system_message_is_rejected_not_dropped() {
        use solvela_protocol::vision::{ContentPart, ImageUrl, MessageContent};
        // A WELL-FORMED image in a SYSTEM message. Anthropic's `system` param is
        // a plain string, so `as_text()` would silently drop the image while the
        // request still settles — the agent pays, the model never sees it. Must
        // reject instead.
        let req = ChatRequest {
            model: "anthropic/claude-sonnet-4-6".to_string(),
            messages: vec![
                ChatMessage {
                    role: Role::System,
                    content: MessageContent::Parts(vec![
                        ContentPart::Text {
                            text: "context".to_string(),
                        },
                        ContentPart::ImageUrl {
                            image_url: ImageUrl {
                                url: "data:image/png;base64,iVBORw0KGgo=".to_string(),
                                detail: None,
                            },
                        },
                    ]),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                ChatMessage {
                    role: Role::User,
                    content: "hi".into(),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
            ],
            max_tokens: Some(100),
            temperature: None,
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
        };
        assert!(
            to_anthropic_request(&req).is_err(),
            "an image in a system message must reject the request, not be silently dropped"
        );
    }

    #[test]
    fn test_system_message_extraction() {
        let req = ChatRequest {
            model: "anthropic/claude-sonnet-4-20250514".to_string(),
            messages: vec![
                ChatMessage {
                    role: Role::System,
                    content: "You are a helpful assistant.".into(),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                ChatMessage {
                    role: Role::User,
                    content: "Hello!".into(),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
            ],
            max_tokens: Some(100),
            temperature: None,
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
        };

        let anthropic_req = to_anthropic_request(&req).unwrap();
        assert_eq!(
            anthropic_req.system,
            Some("You are a helpful assistant.".to_string())
        );
        assert_eq!(anthropic_req.messages.len(), 1);
        assert_eq!(anthropic_req.messages[0].role, "user");
        assert_eq!(anthropic_req.model, "claude-sonnet-4-20250514");
    }

    #[test]
    fn test_developer_role_extracted_as_system() {
        let req = ChatRequest {
            model: "anthropic/claude-sonnet-4-20250514".to_string(),
            messages: vec![
                ChatMessage {
                    role: Role::System,
                    content: "You are a helpful assistant.".into(),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                ChatMessage {
                    role: Role::Developer,
                    content: "Always respond in JSON.".into(),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                ChatMessage {
                    role: Role::User,
                    content: "Hello!".into(),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
            ],
            max_tokens: Some(100),
            temperature: None,
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
        };

        let anthropic_req = to_anthropic_request(&req).unwrap();

        // Both System and Developer messages should be extracted into the system param
        assert_eq!(
            anthropic_req.system,
            Some("You are a helpful assistant.\n\nAlways respond in JSON.".to_string())
        );
        // Only the User message should remain in messages
        assert_eq!(anthropic_req.messages.len(), 1);
        assert_eq!(anthropic_req.messages[0].role, "user");
        assert_eq!(anthropic_req.messages[0].content.len(), 1);
        assert_eq!(block_text(&anthropic_req.messages[0].content[0]), "Hello!");
    }

    #[test]
    fn test_content_block_delta_parse_failure_is_detected() {
        // Verify that malformed JSON for content_block_delta actually fails to parse,
        // which is the condition that triggers our warn! + best-effort forwarding.
        let malformed = r#"{"delta": {"not_text": "hello"}}"#;
        let result = serde_json::from_str::<AnthropicContentBlockDelta>(malformed);
        // This should parse OK because `text` is Option with #[serde(default)],
        // but verify the text field is None (which would result in an empty delta).
        assert!(result.is_ok());
        assert!(result.unwrap().delta.text.is_none());

        // Truly malformed JSON should fail to parse
        let truly_malformed = r#"{"delta": not valid json"#;
        let result = serde_json::from_str::<AnthropicContentBlockDelta>(truly_malformed);
        assert!(result.is_err(), "truly malformed JSON must fail to parse");
    }

    #[test]
    fn test_message_start_parse_failure_is_detected() {
        let malformed = r#"{"not_message": {}}"#;
        let result = serde_json::from_str::<AnthropicMessageStart>(malformed);
        assert!(
            result.is_err(),
            "missing 'message' field must fail to parse"
        );
    }

    #[test]
    fn test_message_delta_parse_failure_is_detected() {
        let malformed = r#"not json at all"#;
        let result = serde_json::from_str::<AnthropicMessageDelta>(malformed);
        assert!(result.is_err(), "invalid JSON must fail to parse");
    }

    #[test]
    fn test_response_translation() {
        let anthropic_resp = AnthropicResponse {
            id: "msg_123".to_string(),
            model: "claude-sonnet-4-20250514".to_string(),
            content: vec![AnthropicContentBlock {
                content_type: "text".to_string(),
                text: Some("Hello! How can I help you?".to_string()),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: AnthropicUsage {
                input_tokens: 10,
                output_tokens: 8,
            },
        };

        let chat_resp =
            from_anthropic_response(anthropic_resp, "anthropic/claude-sonnet-4-20250514");
        assert_eq!(chat_resp.object, "chat.completion");
        assert_eq!(chat_resp.choices.len(), 1);
        assert_eq!(
            chat_resp.choices[0].message.content.as_text(),
            "Hello! How can I help you?"
        );
        assert_eq!(chat_resp.choices[0].finish_reason, Some("stop".to_string()));
        assert_eq!(chat_resp.usage.as_ref().unwrap().total_tokens, 18);
    }
}
