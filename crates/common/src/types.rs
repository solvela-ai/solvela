use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Chat completion types (OpenAI-compatible)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Tool call / function call types
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Vision / multi-modal content types
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Tool definition types (for requests)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Streaming types
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Streaming tool call delta types
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Model & pricing types
// ---------------------------------------------------------------------------

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

// Re-export cost breakdown types from x402-solana (canonical source).
pub use x402_solana::constants::{PLATFORM_FEE_MULTIPLIER, PLATFORM_FEE_PERCENT};
pub use x402_solana::types::CostBreakdown;

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_cost_breakdown() {
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

    #[test]
    fn test_developer_role_serde() {
        let role = Role::Developer;
        let json = serde_json::to_string(&role).unwrap();
        assert_eq!(json, "\"developer\"");
        let deser: Role = serde_json::from_str(&json).unwrap();
        assert_eq!(deser, Role::Developer);
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
        assert_eq!(deser, tc);
        // Verify "type" field name in JSON (not "r#type")
        let val: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(val.get("type").is_some());
        assert!(val.get("r#type").is_none());
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
        assert!(json.get("tool_call_id").is_none()); // None => skipped
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
        assert!(json.get("tool_calls").is_none()); // None => skipped
    }

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

    #[test]
    fn test_backward_compat_no_tool_fields() {
        // Old JSON without tool_calls or tool_call_id should still deserialize
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
        // Old JSON request without tools or tool_choice should still deserialize
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
        // Old JSON without new capability fields should still deserialize with defaults
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
        // Existing defaults should also work
        assert!(!info.supports_streaming);
        assert!(!info.supports_tools);
        assert!(!info.supports_vision);
        assert!(!info.reasoning);
    }
}
