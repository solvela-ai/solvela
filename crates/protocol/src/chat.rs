use serde::{Deserialize, Serialize};

use crate::tools::{ToolCall, ToolDefinition};
use crate::vision::MessageContent;

/// Role of a message participant.
///
/// `Unknown` is a forward-compat catch-all on the **deserialize** side: a
/// provider response that carries a role string we haven't enumerated
/// (e.g., a future OpenAI-introduced variant) lands as `Role::Unknown`
/// instead of failing the whole message parse and surfacing a 500.
/// `#[serde(other)]` only affects deserialization; serialization of
/// `Role::Unknown` produces the literal `"unknown"` string. Provider
/// adapters should not emit `Unknown` to upstream APIs — translate it
/// to a sensible default (typically `"user"`) before serializing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
    Developer,
    #[serde(other)]
    Unknown,
}

/// Deserialize the `content` field, mapping both an absent field and an
/// explicit JSON `null` to the [`MessageContent`] default (`Text("")`).
///
/// OpenAI emits `"content": null` on assistant turns that carry only
/// `tool_calls`. The `#[serde(untagged)]` `MessageContent` enum cannot match
/// `null` against either variant, so without this it would hard-fail
/// deserialization — surfacing a 500 *after* payment on the chat path. This
/// keeps a missing/null content tolerant and lossless.
fn deserialize_content<'de, D>(de: D) -> Result<MessageContent, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<MessageContent>::deserialize(de)?.unwrap_or_default())
}

/// A single message in a chat conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    #[serde(default, deserialize_with = "deserialize_content")]
    pub content: MessageContent,
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

impl Usage {
    /// Construct a `Usage` from prompt and completion counts, computing
    /// `total_tokens = prompt_tokens.saturating_add(completion_tokens)`.
    ///
    /// Provider adapters should prefer this constructor over building the
    /// struct directly: a plain `prompt + completion` panics in debug
    /// builds on overflow and silently wraps in release. Even though
    /// today's models stay well below `u32::MAX` (Claude 3.5's 200K
    /// context + 8K output is ~0.005% of `u32::MAX`), keeping the
    /// arithmetic saturating means the gateway billing path
    /// (`cap_usage_to_request_limits` in `routes/chat/cost.rs`) never
    /// reads a wrapped value.
    pub fn new(prompt_tokens: u32, completion_tokens: u32) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens.saturating_add(completion_tokens),
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{FunctionCall, FunctionDefinitionInner, ToolCall, ToolDefinition};
    use crate::vision::ContentPart;

    #[test]
    fn test_chat_message_deserializes_content_array() {
        // CowAgent-style payload: content is an array of content parts.
        let json = r#"{"role":"user","content":[{"type":"text","text":"Hello!"}]}"#;
        let msg: ChatMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.role, Role::User);
        assert!(matches!(msg.content, MessageContent::Parts(_)));
        assert_eq!(msg.content.as_text(), "Hello!");
    }

    #[test]
    fn test_chat_message_deserializes_content_string() {
        let json = r#"{"role":"user","content":"Hello!"}"#;
        let msg: ChatMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg.content, MessageContent::Text(_)));
        assert_eq!(msg.content.as_text(), "Hello!");
    }

    #[test]
    fn test_chat_message_string_content_serializes_as_json_string() {
        // Wire-compat regression guard: string content must serialize back
        // out as a bare JSON string, never an array/object.
        let msg = ChatMessage {
            role: Role::User,
            content: "hi".into(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["content"], serde_json::json!("hi"));
        assert!(json["content"].is_string());
    }

    #[test]
    fn test_chat_message_multi_text_parts_flatten() {
        let json =
            r#"{"role":"user","content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]}"#;
        let msg: ChatMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.content.as_text(), "a b");
    }

    #[test]
    fn test_chat_message_null_content_defaults_to_empty() {
        // OpenAI emits `"content": null` on tool-call assistant turns.
        let json = r#"{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","type":"function","function":{"name":"f","arguments":"{}"}}]}"#;
        let msg: ChatMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.role, Role::Assistant);
        assert_eq!(msg.content.as_text(), "");
        assert!(msg.content.is_empty());
        assert!(msg.tool_calls.is_some());
    }

    #[test]
    fn test_chat_message_absent_content_defaults_to_empty() {
        let json = r#"{"role":"assistant","tool_calls":[{"id":"call_1","type":"function","function":{"name":"f","arguments":"{}"}}]}"#;
        let msg: ChatMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.content.as_text(), "");
        assert!(msg.content.is_empty());
    }

    #[test]
    fn test_chat_message_number_content_rejected() {
        // A JSON number is not a valid content shape — must fail to
        // deserialize (surfaced as a 4xx at the HTTP boundary), never panic.
        let json = r#"{"role":"user","content":42}"#;
        let result: Result<ChatMessage, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn test_chat_message_object_content_rejected() {
        // REGRESSION GUARD: an unknown JSON OBJECT content (e.g. a future
        // `{"type":"audio",...}`) must be rejected, not silently coerced to
        // `Text("")`. `deserialize_content` only maps JSON `null`/absent to the
        // default; any other non-string/non-array value (here an object)
        // matches neither untagged `MessageContent` variant, so the inner
        // deserialize errors and propagates. Without this, an object would be
        // billed as an empty prompt while the structured content is dropped.
        let json = r#"{"role":"user","content":{"type":"audio","data":"x"}}"#;
        let result: Result<ChatMessage, _> = serde_json::from_str(json);
        assert!(
            result.is_err(),
            "object content must be rejected, not coerced to empty text"
        );
    }

    #[test]
    fn test_chat_message_mixed_text_image_returns_only_text() {
        let json = r#"{"role":"user","content":[{"type":"text","text":"look"},{"type":"image_url","image_url":{"url":"https://example.com/i.png"}}]}"#;
        let msg: ChatMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg.content, MessageContent::Parts(_)));
        assert_eq!(msg.content.as_text(), "look");
        // Sanity: the parsed parts actually include the image part.
        if let MessageContent::Parts(ref parts) = msg.content {
            assert_eq!(parts.len(), 2);
            assert!(parts
                .iter()
                .any(|p| matches!(p, ContentPart::ImageUrl { .. })));
        }
    }

    #[test]
    fn test_chat_request_serialization() {
        let req = ChatRequest {
            model: "openai/gpt-4o".to_string(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: "Hello!".into(),
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
            content: MessageContent::Text(String::new()),
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
            content: r#"{"temp":72}"#.into(),
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
        assert_eq!(msg.content.as_text(), "Hello!");
        assert!(msg.tool_calls.is_none());
        assert!(msg.tool_call_id.is_none());
        assert!(msg.name.is_none());
    }

    #[test]
    fn test_backward_compat_request_no_tools() {
        let json = r#"{"model":"openai/gpt-4o","messages":[{"role":"user","content":"Hi"}],"stream":false}"#;
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
                content: "What's the weather?".into(),
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
                    description: Some("Get weather for a location".to_string()),
                    parameters: Some(
                        serde_json::json!({"type":"object","properties":{"location":{"type":"string"}}}),
                    ),
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
