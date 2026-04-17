use async_trait::async_trait;

use solvela_protocol::{ChatRequest, ChatResponse, ModelInfo};

use super::{ChatStream, LLMProvider, ProviderError};

/// OpenAI provider adapter.
///
/// OpenAI's API is the baseline format — requests are passed through
/// with minimal transformation.
pub struct OpenAIProvider {
    api_key: String,
    client: reqwest::Client,
}

impl OpenAIProvider {
    pub fn new(client: reqwest::Client, api_key: String) -> Self {
        Self { api_key, client }
    }
}

#[async_trait]
impl LLMProvider for OpenAIProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        // Models are loaded from config, not hardcoded here
        vec![]
    }

    async fn chat_completion(
        &self,
        mut req: ChatRequest,
    ) -> Result<ChatResponse, Box<dyn std::error::Error + Send + Sync>> {
        req.model = req
            .model
            .strip_prefix("openai/")
            .unwrap_or(&req.model)
            .to_string();
        let req_body = serde_json::to_value(&req)?;
        let response = super::retry_with_backoff(2, || {
            self.client
                .post("https://api.openai.com/v1/chat/completions")
                .timeout(std::time::Duration::from_secs(90))
                .bearer_auth(&self.api_key)
                .json(&req_body)
                .send()
        })
        .await?;

        let body = response.error_for_status()?.json::<ChatResponse>().await?;
        Ok(body)
    }

    async fn chat_completion_stream(
        &self,
        mut req: ChatRequest,
    ) -> Result<ChatStream, ProviderError> {
        req.model = req
            .model
            .strip_prefix("openai/")
            .unwrap_or(&req.model)
            .to_string();
        let mut body = serde_json::to_value(&req)?;
        body["stream"] = serde_json::Value::Bool(true);

        let response = self
            .client
            .post("https://api.openai.com/v1/chat/completions")
            .timeout(std::time::Duration::from_secs(90))
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?
            .error_for_status()?;

        Ok(super::spawn_openai_sse_parser(response))
    }
}
