use async_trait::async_trait;

use solvela_protocol::{ChatRequest, ChatResponse, ModelRegistration};

use super::{ChatStream, LLMProvider, ProviderError};

const DEEPSEEK_PREFIX: &str = "deepseek/";
const DEEPSEEK_URL: &str = "https://api.deepseek.com/chat/completions";

/// DeepSeek provider adapter.
///
/// DeepSeek's API is OpenAI-compatible — requests pass through with
/// only the base URL changed.
pub struct DeepSeekProvider {
    api_key: String,
    client: reqwest::Client,
}

impl DeepSeekProvider {
    pub fn new(client: reqwest::Client, api_key: String) -> Self {
        Self { api_key, client }
    }
}

fn strip_model_prefix(model: &str) -> String {
    model
        .strip_prefix(DEEPSEEK_PREFIX)
        .unwrap_or(model)
        .to_string()
}

fn build_chat_body(req: &ChatRequest) -> Result<serde_json::Value, serde_json::Error> {
    let mut req = req.clone();
    req.model = strip_model_prefix(&req.model);
    serde_json::to_value(&req)
}

fn build_stream_body(req: &ChatRequest) -> Result<serde_json::Value, serde_json::Error> {
    let mut body = build_chat_body(req)?;
    body["stream"] = serde_json::Value::Bool(true);
    Ok(body)
}

#[async_trait]
impl LLMProvider for DeepSeekProvider {
    fn name(&self) -> &str {
        "deepseek"
    }

    fn supported_models(&self) -> Vec<ModelRegistration> {
        vec![]
    }

    async fn chat_completion(
        &self,
        req: ChatRequest,
    ) -> Result<ChatResponse, Box<dyn std::error::Error + Send + Sync>> {
        let req_body = build_chat_body(&req)?;
        let response = super::retry_with_backoff(2, || {
            self.client
                .post(DEEPSEEK_URL)
                .timeout(std::time::Duration::from_secs(90))
                .bearer_auth(&self.api_key)
                .json(&req_body)
                .send()
        })
        .await?;

        let body = response.error_for_status()?.json::<ChatResponse>().await?;
        Ok(body)
    }

    async fn chat_completion_stream(&self, req: ChatRequest) -> Result<ChatStream, ProviderError> {
        let body = build_stream_body(&req)?;

        let response = self
            .client
            .post(DEEPSEEK_URL)
            .timeout(std::time::Duration::from_secs(90))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?
            .error_for_status()?;

        Ok(super::spawn_openai_sse_parser(response))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solvela_protocol::{ChatMessage, Role};

    fn sample_request(model: &str) -> ChatRequest {
        ChatRequest {
            model: model.to_string(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: "hi".into(),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            max_tokens: Some(64),
            temperature: Some(0.7),
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
        }
    }

    #[test]
    fn strip_model_prefix_removes_known_prefix() {
        assert_eq!(
            strip_model_prefix("deepseek/deepseek-chat"),
            "deepseek-chat"
        );
    }

    #[test]
    fn strip_model_prefix_leaves_other_models_unchanged() {
        assert_eq!(strip_model_prefix("deepseek-reasoner"), "deepseek-reasoner");
        assert_eq!(strip_model_prefix("openai/gpt-4o"), "openai/gpt-4o");
    }

    #[test]
    fn strip_model_prefix_handles_empty_remainder() {
        assert_eq!(strip_model_prefix("deepseek/"), "");
    }

    #[test]
    fn build_chat_body_strips_prefix_and_serializes() {
        let req = sample_request("deepseek/deepseek-chat");
        let body = build_chat_body(&req).expect("body must serialize");
        assert_eq!(body["model"], "deepseek-chat");
        assert_eq!(body["messages"][0]["content"], "hi");
        assert!(body.get("stream").is_none() || body["stream"] == serde_json::Value::Bool(false));
    }

    #[test]
    fn build_chat_body_preserves_unprefixed_model() {
        let req = sample_request("deepseek-reasoner");
        let body = build_chat_body(&req).expect("body must serialize");
        assert_eq!(body["model"], "deepseek-reasoner");
    }

    #[test]
    fn build_stream_body_injects_stream_true() {
        let req = sample_request("deepseek/deepseek-chat");
        let body = build_stream_body(&req).expect("body must serialize");
        assert_eq!(body["model"], "deepseek-chat");
        assert_eq!(body["stream"], serde_json::Value::Bool(true));
    }

    #[test]
    fn build_stream_body_does_not_mutate_caller() {
        let req = sample_request("deepseek/deepseek-chat");
        let _ = build_stream_body(&req).expect("body must serialize");
        assert_eq!(req.model, "deepseek/deepseek-chat");
    }

    #[test]
    fn provider_name_is_stable() {
        let provider = DeepSeekProvider::new(reqwest::Client::new(), "ds-test".to_string());
        assert_eq!(provider.name(), "deepseek");
    }

    #[test]
    fn supported_models_is_empty_by_design() {
        let provider = DeepSeekProvider::new(reqwest::Client::new(), "ds-test".to_string());
        assert!(provider.supported_models().is_empty());
    }
}
