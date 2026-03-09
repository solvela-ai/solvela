use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use rcr_common::types::{
    ChatChoice, ChatChunk, ChatChunkChoice, ChatDelta, ChatMessage, ChatRequest, ChatResponse,
    ModelInfo, Role, Usage,
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
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            client: reqwest::Client::new(),
        }
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

#[derive(Debug, Serialize, Deserialize)]
struct AnthropicMessage {
    role: String,
    content: String,
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

/// Convert an OpenAI-format request to Anthropic Messages API format.
fn to_anthropic_request(req: &ChatRequest) -> AnthropicRequest {
    // Extract system message(s) — Anthropic takes system as a separate param
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

    // Filter to user/assistant messages only
    let messages: Vec<AnthropicMessage> = req
        .messages
        .iter()
        .filter(|m| m.role == Role::User || m.role == Role::Assistant)
        .map(|m| AnthropicMessage {
            role: match m.role {
                Role::User => "user".to_string(),
                Role::Assistant => "assistant".to_string(),
                _ => "user".to_string(), // shouldn't happen due to filter
            },
            content: m.content.clone(),
        })
        .collect();

    // Extract model_id part (e.g., "anthropic/claude-sonnet-4.6" → "claude-sonnet-4.6")
    let model = req
        .model
        .strip_prefix("anthropic/")
        .unwrap_or(&req.model)
        .to_string();

    AnthropicRequest {
        model,
        max_tokens: req.max_tokens.unwrap_or(4096),
        system,
        messages,
        temperature: req.temperature,
        top_p: req.top_p,
        stream: None,
    }
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
                content,
                name: None,
                tool_calls: None,
                tool_call_id: None,
            },
            finish_reason,
        }],
        usage: Some(Usage {
            prompt_tokens: resp.usage.input_tokens,
            completion_tokens: resp.usage.output_tokens,
            total_tokens: resp.usage.input_tokens + resp.usage.output_tokens,
        }),
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
                        buffer = buffer[pos + 2..].to_string();

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
                                if let Ok(msg) =
                                    serde_json::from_str::<AnthropicMessageStart>(&data)
                                {
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
                            }
                            "content_block_delta" => {
                                if let Ok(cbd) =
                                    serde_json::from_str::<AnthropicContentBlockDelta>(&data)
                                {
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
                            }
                            "message_delta" => {
                                if let Ok(md) = serde_json::from_str::<AnthropicMessageDelta>(&data)
                                {
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

    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![]
    }

    async fn chat_completion(
        &self,
        req: ChatRequest,
    ) -> Result<ChatResponse, Box<dyn std::error::Error + Send + Sync>> {
        let original_model = req.model.clone();
        let anthropic_req = to_anthropic_request(&req);

        let response = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&anthropic_req)
            .send()
            .await?;

        let anthropic_resp = response
            .error_for_status()?
            .json::<AnthropicResponse>()
            .await?;

        Ok(from_anthropic_response(anthropic_resp, &original_model))
    }

    async fn chat_completion_stream(&self, req: ChatRequest) -> Result<ChatStream, ProviderError> {
        let model = req.model.clone();
        let mut anthropic_req = to_anthropic_request(&req);
        anthropic_req.stream = Some(true);

        let response = self
            .client
            .post("https://api.anthropic.com/v1/messages")
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

    #[test]
    fn test_system_message_extraction() {
        let req = ChatRequest {
            model: "anthropic/claude-sonnet-4.6".to_string(),
            messages: vec![
                ChatMessage {
                    role: Role::System,
                    content: "You are a helpful assistant.".to_string(),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                ChatMessage {
                    role: Role::User,
                    content: "Hello!".to_string(),
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

        let anthropic_req = to_anthropic_request(&req);
        assert_eq!(
            anthropic_req.system,
            Some("You are a helpful assistant.".to_string())
        );
        assert_eq!(anthropic_req.messages.len(), 1);
        assert_eq!(anthropic_req.messages[0].role, "user");
        assert_eq!(anthropic_req.model, "claude-sonnet-4.6");
    }

    #[test]
    fn test_developer_role_extracted_as_system() {
        let req = ChatRequest {
            model: "anthropic/claude-sonnet-4.6".to_string(),
            messages: vec![
                ChatMessage {
                    role: Role::System,
                    content: "You are a helpful assistant.".to_string(),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                ChatMessage {
                    role: Role::Developer,
                    content: "Always respond in JSON.".to_string(),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                ChatMessage {
                    role: Role::User,
                    content: "Hello!".to_string(),
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

        let anthropic_req = to_anthropic_request(&req);

        // Both System and Developer messages should be extracted into the system param
        assert_eq!(
            anthropic_req.system,
            Some("You are a helpful assistant.\n\nAlways respond in JSON.".to_string())
        );
        // Only the User message should remain in messages
        assert_eq!(anthropic_req.messages.len(), 1);
        assert_eq!(anthropic_req.messages[0].role, "user");
        assert_eq!(anthropic_req.messages[0].content, "Hello!");
    }

    #[test]
    fn test_response_translation() {
        let anthropic_resp = AnthropicResponse {
            id: "msg_123".to_string(),
            model: "claude-sonnet-4.6".to_string(),
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

        let chat_resp = from_anthropic_response(anthropic_resp, "anthropic/claude-sonnet-4.6");
        assert_eq!(chat_resp.object, "chat.completion");
        assert_eq!(chat_resp.choices.len(), 1);
        assert_eq!(
            chat_resp.choices[0].message.content,
            "Hello! How can I help you?"
        );
        assert_eq!(chat_resp.choices[0].finish_reason, Some("stop".to_string()));
        assert_eq!(chat_resp.usage.as_ref().unwrap().total_tokens, 18);
    }
}
