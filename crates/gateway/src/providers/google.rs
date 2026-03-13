use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tracing::warn;

use rustyclaw_protocol::{
    ChatChoice, ChatMessage, ChatRequest, ChatResponse, ModelInfo, Role, Usage,
};

use super::LLMProvider;

/// Google (Gemini) provider adapter.
///
/// Translates between OpenAI format and Google's Gemini API format.
/// Key differences:
/// - Uses `generateContent` endpoint with `contents` array
/// - System instruction is a separate `system_instruction` field
/// - Parts-based content model instead of string content
/// - Usage is returned as `usageMetadata`
pub struct GoogleProvider {
    api_key: String,
    client: reqwest::Client,
}

impl GoogleProvider {
    pub fn new(client: reqwest::Client, api_key: String) -> Self {
        Self { api_key, client }
    }
}

// ---------------------------------------------------------------------------
// Gemini API request/response types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system_instruction: Option<GeminiContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    generation_config: Option<GeminiGenerationConfig>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiContent {
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Serialize, Deserialize)]
struct GeminiPart {
    text: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct GeminiGenerationConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    top_p: Option<f32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiResponse {
    candidates: Option<Vec<GeminiCandidate>>,
    usage_metadata: Option<GeminiUsageMetadata>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiCandidate {
    content: GeminiContent,
    finish_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GeminiUsageMetadata {
    prompt_token_count: Option<u32>,
    candidates_token_count: Option<u32>,
    total_token_count: Option<u32>,
}

// ---------------------------------------------------------------------------
// Format translation
// ---------------------------------------------------------------------------

fn to_gemini_request(req: &ChatRequest) -> GeminiRequest {
    // Extract system instruction
    let system_instruction: Option<GeminiContent> = {
        let system_text: Vec<&str> = req
            .messages
            .iter()
            .filter(|m| m.role == Role::System || m.role == Role::Developer)
            .map(|m| m.content.as_str())
            .collect();

        if system_text.is_empty() {
            None
        } else {
            Some(GeminiContent {
                role: None,
                parts: vec![GeminiPart {
                    text: system_text.join("\n\n"),
                }],
            })
        }
    };

    // Convert messages (excluding system) to Gemini contents
    let contents: Vec<GeminiContent> = req
        .messages
        .iter()
        .filter(|m| m.role != Role::System && m.role != Role::Developer)
        .map(|m| GeminiContent {
            role: Some(match m.role {
                Role::User => "user".to_string(),
                Role::Assistant => "model".to_string(),
                Role::System | Role::Developer => "user".to_string(), // filtered above, but safe fallback
                Role::Tool => "user".to_string(), // Gemini uses "user" for tool results
            }),
            parts: vec![GeminiPart {
                text: m.content.clone(),
            }],
        })
        .collect();

    let generation_config =
        if req.max_tokens.is_some() || req.temperature.is_some() || req.top_p.is_some() {
            Some(GeminiGenerationConfig {
                max_output_tokens: req.max_tokens,
                temperature: req.temperature,
                top_p: req.top_p,
            })
        } else {
            None
        };

    GeminiRequest {
        contents,
        system_instruction,
        generation_config,
    }
}

fn from_gemini_response(resp: GeminiResponse, original_model: &str) -> ChatResponse {
    let (content, finish_reason) = match resp.candidates.as_ref().and_then(|c| c.first()) {
        Some(c) => {
            let text: String = c
                .content
                .parts
                .iter()
                .map(|p| p.text.as_str())
                .collect::<Vec<_>>()
                .join("");
            let reason = c.finish_reason.as_ref().map(|r| match r.as_str() {
                "STOP" => "stop".to_string(),
                "MAX_TOKENS" => "length".to_string(),
                "SAFETY" => "content_filter".to_string(),
                other => other.to_lowercase(),
            });
            (text, reason)
        }
        None => {
            warn!(
                model = %original_model,
                "Gemini response contained no candidates; likely content filter"
            );
            (String::new(), Some("content_filter".to_string()))
        }
    };

    let usage = match resp.usage_metadata {
        Some(u) => {
            if u.prompt_token_count.is_none()
                || u.candidates_token_count.is_none()
                || u.total_token_count.is_none()
            {
                warn!(
                    model = %original_model,
                    prompt_tokens = ?u.prompt_token_count,
                    completion_tokens = ?u.candidates_token_count,
                    total_tokens = ?u.total_token_count,
                    "Gemini usage_metadata has missing token count fields; defaulting to 0"
                );
            }
            Some(Usage {
                prompt_tokens: u.prompt_token_count.unwrap_or(0),
                completion_tokens: u.candidates_token_count.unwrap_or(0),
                total_tokens: u.total_token_count.unwrap_or(0),
            })
        }
        None => {
            warn!(
                model = %original_model,
                "Gemini response missing usage_metadata; token counts will be 0"
            );
            None
        }
    };

    ChatResponse {
        id: format!("gemini-{}", uuid::Uuid::new_v4()),
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
        usage,
    }
}

#[async_trait]
impl LLMProvider for GoogleProvider {
    fn name(&self) -> &str {
        "google"
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        vec![]
    }

    async fn chat_completion(
        &self,
        req: ChatRequest,
    ) -> Result<ChatResponse, Box<dyn std::error::Error + Send + Sync>> {
        let original_model = req.model.clone();

        // Extract Gemini model name (e.g., "google/gemini-2.5-flash" → "gemini-2.5-flash")
        let model_name = req.model.strip_prefix("google/").unwrap_or(&req.model);

        // API key sent as a header (not a URL query param) to prevent key leakage
        // in server logs, proxy logs, and browser history.
        let url = format!(
            "https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent",
            model_name
        );

        let gemini_req = to_gemini_request(&req);

        let req_body = serde_json::to_value(&gemini_req)?;
        let response = super::retry_with_backoff(2, || {
            self.client
                .post(&url)
                .timeout(std::time::Duration::from_secs(90))
                .header("content-type", "application/json")
                .header("x-goog-api-key", &self.api_key)
                .json(&req_body)
                .send()
        })
        .await?;

        let gemini_resp = response
            .error_for_status()?
            .json::<GeminiResponse>()
            .await?;

        Ok(from_gemini_response(gemini_resp, &original_model))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gemini_request_translation() {
        let req = ChatRequest {
            model: "google/gemini-2.5-flash".to_string(),
            messages: vec![
                ChatMessage {
                    role: Role::System,
                    content: "Be concise.".to_string(),
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
            temperature: Some(0.7),
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
        };

        let gemini_req = to_gemini_request(&req);

        assert!(gemini_req.system_instruction.is_some());
        assert_eq!(
            gemini_req.system_instruction.unwrap().parts[0].text,
            "Be concise."
        );
        assert_eq!(gemini_req.contents.len(), 1);
        assert_eq!(gemini_req.contents[0].role.as_deref(), Some("user"));
        assert_eq!(
            gemini_req
                .generation_config
                .as_ref()
                .unwrap()
                .max_output_tokens,
            Some(100)
        );
    }

    #[test]
    fn test_developer_role_extracted_as_system_instruction() {
        let req = ChatRequest {
            model: "google/gemini-2.5-flash".to_string(),
            messages: vec![
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

        let gemini_req = to_gemini_request(&req);

        // Developer message should be extracted as system_instruction
        assert!(gemini_req.system_instruction.is_some());
        assert_eq!(
            gemini_req.system_instruction.unwrap().parts[0].text,
            "Always respond in JSON."
        );
        // Only the User message should remain in contents
        assert_eq!(gemini_req.contents.len(), 1);
        assert_eq!(gemini_req.contents[0].role.as_deref(), Some("user"));
    }

    #[test]
    fn test_gemini_response_translation() {
        let gemini_resp = GeminiResponse {
            candidates: Some(vec![GeminiCandidate {
                content: GeminiContent {
                    role: Some("model".to_string()),
                    parts: vec![GeminiPart {
                        text: "Hi there!".to_string(),
                    }],
                },
                finish_reason: Some("STOP".to_string()),
            }]),
            usage_metadata: Some(GeminiUsageMetadata {
                prompt_token_count: Some(5),
                candidates_token_count: Some(3),
                total_token_count: Some(8),
            }),
        };

        let chat_resp = from_gemini_response(gemini_resp, "google/gemini-2.5-flash");
        assert_eq!(chat_resp.choices[0].message.content, "Hi there!");
        assert_eq!(chat_resp.choices[0].finish_reason, Some("stop".to_string()));
        assert_eq!(chat_resp.usage.as_ref().unwrap().total_tokens, 8);
    }
}
