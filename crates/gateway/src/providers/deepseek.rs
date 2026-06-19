use async_trait::async_trait;
use serde::Deserialize;
use tracing::warn;

use solvela_protocol::{ChatRequest, ChatResponse, ModelRegistration};

use super::cache_usage::CacheUsage;
use super::{ChatStream, LLMProvider, ProviderError};

const DEEPSEEK_PREFIX: &str = "deepseek/";
const DEEPSEEK_URL: &str = "https://api.deepseek.com/chat/completions";
/// Provider label for cache-token metering counters.
const PROVIDER_LABEL: &str = "deepseek";

/// Internal, parse-only view of DeepSeek's cache-usage fields.
///
/// DeepSeek reports cache READS via `usage.prompt_cache_hit_tokens` (and the
/// uncached remainder via `prompt_cache_miss_tokens`, which we do not need)
/// — verified against api-docs.deepseek.com/guides/kv_cache 2026-06-17. There
/// is no separate cache-WRITE count, so the metering write counter stays 0.
/// `#[serde(default)]` makes a response without the field yield 0 (fail-safe).
/// OBSERVABILITY ONLY — never reaches billing or the public `Usage` type.
#[derive(Debug, Default, Deserialize)]
struct DeepSeekUsage {
    #[serde(default)]
    prompt_cache_hit_tokens: u32,
}

/// Extract the cache-read token count from a DeepSeek response body.
///
/// Returns `CacheUsage::default()` (0/0) when `usage` or the field is absent —
/// fail-safe, never an error.
fn cache_usage_from_deepseek_body(body: &serde_json::Value) -> CacheUsage {
    // Absent `usage` (or no-cache) ⇒ 0 silently — the normal fail-safe. A
    // `usage` block that is PRESENT but fails to deserialize is a real error
    // (e.g. the provider sends `prompt_cache_hit_tokens` as a string/float/null,
    // or renames the field): log it before falling back to 0 so the counter
    // reading 0 is not silent (Finding 1). Only an actual deserialize error
    // warns; the absent/no-cache path stays quiet.
    let read = match body.get("usage") {
        None => 0,
        Some(usage) => match serde_json::from_value::<DeepSeekUsage>(usage.clone()) {
            Ok(parsed) => parsed.prompt_cache_hit_tokens,
            Err(e) => {
                warn!(
                    error = %e,
                    "deepseek: failed to parse usage details for cache metering; cache counts will be 0"
                );
                0
            }
        },
    };
    CacheUsage {
        cache_read_tokens: read,
        cache_write_tokens: 0,
    }
}

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
        let original_model = req.model.clone();
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

        // Read the body once as a Value to drive OBSERVABILITY-ONLY cache
        // metering (`usage.prompt_cache_hit_tokens`), then deserialize the
        // public `ChatResponse` for the agent. Billing is untouched.
        let value = response
            .error_for_status()?
            .json::<serde_json::Value>()
            .await?;
        let cache_usage = cache_usage_from_deepseek_body(&value);
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
    fn build_chat_body_passes_text_parts_through_as_array() {
        use solvela_protocol::vision::{ContentPart, MessageContent};
        let mut req = sample_request("deepseek/deepseek-chat");
        req.messages[0].content = MessageContent::Parts(vec![
            ContentPart::Text {
                text: "Hello!".to_string(),
            },
            ContentPart::Text {
                text: "world".to_string(),
            },
        ]);
        let body = build_chat_body(&req).expect("body must serialize");
        // DeepSeek is OpenAI-compatible: array content passes through natively
        // rather than being flattened — the upstream API consumes the parts.
        assert!(
            body["messages"][0]["content"].is_array(),
            "text Parts content must serialize as a JSON array for OpenAI passthrough"
        );
        assert_eq!(body["messages"][0]["content"][0]["type"], "text");
        assert_eq!(body["messages"][0]["content"][0]["text"], "Hello!");
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

    // -----------------------------------------------------------------------
    // PR-2 cache-token metering (OBSERVABILITY ONLY).
    // -----------------------------------------------------------------------

    use crate::cache::test_metrics::{counter_value_filtered, install_test_recorder};

    /// A body WITH `usage.prompt_cache_hit_tokens` yields a `CacheUsage` with
    /// that count as the READ (DeepSeek has no separate cache-write count).
    #[test]
    fn cache_usage_from_deepseek_body_reads_hit_tokens() {
        let body = serde_json::json!({
            "usage": {
                "prompt_tokens": 1000,
                "completion_tokens": 20,
                "prompt_cache_hit_tokens": 640,
                "prompt_cache_miss_tokens": 360
            }
        });
        let cu = cache_usage_from_deepseek_body(&body);
        assert_eq!(cu.cache_read_tokens, 640);
        assert_eq!(cu.cache_write_tokens, 0);
    }

    /// A body WITHOUT the cache field (or without `usage`) yields
    /// `CacheUsage::default()` (0/0) — never a parse error.
    #[test]
    fn cache_usage_from_deepseek_body_defaults_when_absent() {
        let body = serde_json::json!({
            "usage": { "prompt_tokens": 1000, "completion_tokens": 20 }
        });
        assert_eq!(cache_usage_from_deepseek_body(&body), CacheUsage::default());

        let body = serde_json::json!({ "id": "x", "choices": [] });
        assert_eq!(cache_usage_from_deepseek_body(&body), CacheUsage::default());
    }

    /// A `usage` block whose `prompt_cache_hit_tokens` has the WRONG type
    /// (string / negative) is a real deserialize error: the parser must fall
    /// back to `CacheUsage::default()` (0/0) — never panic — while flagging the
    /// failure (Finding 1: the old `.ok()` swallowed this silently). Falsifiable:
    /// an `unwrap` on the parse Result would panic here.
    #[test]
    fn cache_usage_from_deepseek_body_falls_back_on_parse_error() {
        let body = serde_json::json!({
            "usage": { "prompt_tokens": 1000, "prompt_cache_hit_tokens": "not-a-number" }
        });
        assert_eq!(cache_usage_from_deepseek_body(&body), CacheUsage::default());

        let body = serde_json::json!({
            "usage": { "prompt_cache_hit_tokens": -7 }
        });
        assert_eq!(cache_usage_from_deepseek_body(&body), CacheUsage::default());
    }

    /// Emitting the parsed `CacheUsage` increments the read counter by the hit
    /// count and the denominator by 1.
    #[test]
    fn deepseek_emit_increments_read_and_denominator() {
        let handle = install_test_recorder();
        let model = "deepseek/metering-unique-model";
        let key = format!("model=\"{model}\"");
        let read_before =
            counter_value_filtered(&handle, "solvela_provider_cache_read_tokens_total", &key);
        let req_before = counter_value_filtered(&handle, "solvela_provider_requests_total", &key);

        let body = serde_json::json!({
            "usage": { "prompt_cache_hit_tokens": 128 }
        });
        cache_usage_from_deepseek_body(&body).emit(PROVIDER_LABEL, model);

        let read_after =
            counter_value_filtered(&handle, "solvela_provider_cache_read_tokens_total", &key);
        let req_after = counter_value_filtered(&handle, "solvela_provider_requests_total", &key);
        assert_eq!(read_after - read_before, 128);
        assert_eq!(req_after - req_before, 1);
    }
}
