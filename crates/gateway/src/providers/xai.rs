use async_trait::async_trait;

use solvela_protocol::{ChatRequest, ChatResponse, ModelRegistration};

use super::openai::cache_usage_from_openai_body;
use super::{ChatStream, LLMProvider, ProviderError};

const XAI_PREFIX: &str = "xai/";
const XAI_URL: &str = "https://api.x.ai/v1/chat/completions";
/// Provider label for cache-token metering counters.
const PROVIDER_LABEL: &str = "xai";

/// xAI (Grok) provider adapter.
///
/// xAI's API is OpenAI-compatible — requests pass through with
/// only the base URL changed.
pub struct XAIProvider {
    api_key: String,
    client: reqwest::Client,
}

impl XAIProvider {
    pub fn new(client: reqwest::Client, api_key: String) -> Self {
        Self { api_key, client }
    }
}

fn strip_model_prefix(model: &str) -> String {
    model.strip_prefix(XAI_PREFIX).unwrap_or(model).to_string()
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
impl LLMProvider for XAIProvider {
    fn name(&self) -> &str {
        "xai"
    }

    fn supported_models(&self) -> Vec<ModelRegistration> {
        // Models are loaded from config, not hardcoded here
        vec![]
    }

    async fn chat_completion(
        &self,
        req: ChatRequest,
    ) -> Result<ChatResponse, Box<dyn std::error::Error + Send + Sync>> {
        let original_model = req.model.clone();
        let req_body = build_chat_body(&req)?;
        let response = super::retry_with_backoff(2, || {
            self.client
                .post(XAI_URL)
                .timeout(super::PROVIDER_REQUEST_TIMEOUT)
                .bearer_auth(&self.api_key)
                .json(&req_body)
                .send()
        })
        .await?;

        // xAI mirrors OpenAI's usage shape, including
        // `usage.prompt_tokens_details.cached_tokens`. Read the body once as a
        // Value to drive OBSERVABILITY-ONLY cache metering, then deserialize the
        // public `ChatResponse` for the agent. Billing is untouched.
        let value = response
            .error_for_status()?
            .json::<serde_json::Value>()
            .await?;
        let cache_usage = cache_usage_from_openai_body(&value);
        let body: ChatResponse = serde_json::from_value(value)?;
        // Emit metering only after a successful ChatResponse deserialize, so the
        // request denominator never counts a 200-OK-but-unparseable body.
        cache_usage.emit(PROVIDER_LABEL, &original_model);
        Ok(body)
    }

    async fn chat_completion_stream(&self, req: ChatRequest) -> Result<ChatStream, ProviderError> {
        let body = build_stream_body(&req)?;

        let response = self
            .client
            .post(XAI_URL)
            .timeout(super::PROVIDER_REQUEST_TIMEOUT)
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
        assert_eq!(strip_model_prefix("xai/grok-4"), "grok-4");
    }

    #[test]
    fn strip_model_prefix_leaves_other_models_unchanged() {
        assert_eq!(strip_model_prefix("grok-4-fast"), "grok-4-fast");
        assert_eq!(strip_model_prefix("openai/gpt-4o"), "openai/gpt-4o");
    }

    #[test]
    fn strip_model_prefix_handles_empty_remainder() {
        assert_eq!(strip_model_prefix("xai/"), "");
    }

    #[test]
    fn build_chat_body_strips_prefix_and_serializes() {
        let req = sample_request("xai/grok-4");
        let body = build_chat_body(&req).expect("body must serialize");
        assert_eq!(body["model"], "grok-4");
        assert_eq!(body["messages"][0]["content"], "hi");
        assert!(body.get("stream").is_none() || body["stream"] == serde_json::Value::Bool(false));
    }

    #[test]
    fn build_chat_body_preserves_unprefixed_model() {
        let req = sample_request("grok-4-fast");
        let body = build_chat_body(&req).expect("body must serialize");
        assert_eq!(body["model"], "grok-4-fast");
    }

    #[test]
    fn build_chat_body_passes_text_parts_through_as_array() {
        use solvela_protocol::vision::{ContentPart, MessageContent};
        let mut req = sample_request("xai/grok-4");
        req.messages[0].content = MessageContent::Parts(vec![
            ContentPart::Text {
                text: "Hello!".to_string(),
            },
            ContentPart::Text {
                text: "world".to_string(),
            },
        ]);
        let body = build_chat_body(&req).expect("body must serialize");
        // xAI is OpenAI-compatible: array content passes through natively rather
        // than being flattened — the upstream API consumes the parts directly.
        assert!(
            body["messages"][0]["content"].is_array(),
            "text Parts content must serialize as a JSON array for OpenAI passthrough"
        );
        assert_eq!(body["messages"][0]["content"][0]["type"], "text");
        assert_eq!(body["messages"][0]["content"][0]["text"], "Hello!");
    }

    #[test]
    fn build_stream_body_injects_stream_true() {
        let req = sample_request("xai/grok-4");
        let body = build_stream_body(&req).expect("body must serialize");
        assert_eq!(body["model"], "grok-4");
        assert_eq!(body["stream"], serde_json::Value::Bool(true));
    }

    #[test]
    fn build_stream_body_does_not_mutate_caller() {
        let req = sample_request("xai/grok-4");
        let _ = build_stream_body(&req).expect("body must serialize");
        assert_eq!(req.model, "xai/grok-4");
    }

    #[test]
    fn provider_name_is_stable() {
        let provider = XAIProvider::new(reqwest::Client::new(), "xai-test".to_string());
        assert_eq!(provider.name(), "xai");
    }

    #[test]
    fn supported_models_is_empty_by_design() {
        let provider = XAIProvider::new(reqwest::Client::new(), "xai-test".to_string());
        assert!(provider.supported_models().is_empty());
    }

    // -----------------------------------------------------------------------
    // PR-2 cache-token metering (OBSERVABILITY ONLY).
    // -----------------------------------------------------------------------

    use crate::cache::test_metrics::{counter_value_filtered, install_test_recorder};
    use crate::providers::cache_usage::CacheUsage;

    /// xAI shares OpenAI's usage shape, so it reuses
    /// `cache_usage_from_openai_body` to read `cached_tokens` as the read count.
    #[test]
    fn xai_reads_cached_tokens_via_shared_helper() {
        let body = serde_json::json!({
            "usage": { "prompt_tokens_details": { "cached_tokens": 333 } }
        });
        let cu = cache_usage_from_openai_body(&body);
        assert_eq!(cu.cache_read_tokens, 333);
        assert_eq!(cu.cache_write_tokens, 0);

        // Absent → 0/0, no error.
        let empty = serde_json::json!({ "usage": { "prompt_tokens": 5 } });
        assert_eq!(cache_usage_from_openai_body(&empty), CacheUsage::default());
    }

    /// Emitting under the xAI provider label increments the read counter and the
    /// denominator.
    #[test]
    fn xai_emit_increments_read_and_denominator() {
        let handle = install_test_recorder();
        let model = "xai/metering-unique-model";
        let key = format!("model=\"{model}\"");
        let read_before =
            counter_value_filtered(&handle, "solvela_provider_cache_read_tokens_total", &key);
        let req_before = counter_value_filtered(&handle, "solvela_provider_requests_total", &key);

        let body = serde_json::json!({
            "usage": { "prompt_tokens_details": { "cached_tokens": 256 } }
        });
        cache_usage_from_openai_body(&body).emit(PROVIDER_LABEL, model);

        let read_after =
            counter_value_filtered(&handle, "solvela_provider_cache_read_tokens_total", &key);
        let req_after = counter_value_filtered(&handle, "solvela_provider_requests_total", &key);
        assert_eq!(read_after - read_before, 256);
        assert_eq!(req_after - req_before, 1);
    }
}
