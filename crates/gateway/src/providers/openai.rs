use async_trait::async_trait;
use serde::Deserialize;
use tracing::warn;

use solvela_protocol::{ChatRequest, ChatResponse, ModelRegistration};

use super::cache_usage::CacheUsage;
use super::{ChatStream, LLMProvider, ProviderError};

const OPENAI_PREFIX: &str = "openai/";
const OPENAI_URL: &str = "https://api.openai.com/v1/chat/completions";
/// Provider label for cache-token metering counters.
const PROVIDER_LABEL: &str = "openai";

/// Internal, parse-only view of the OpenAI `usage.prompt_tokens_details`
/// sub-object purely to read `cached_tokens` for metering. This deliberately
/// does NOT extend the frozen public `solvela_protocol::Usage`: the response is
/// still deserialized into `ChatResponse` for the agent, and this struct reads
/// the same JSON body a SECOND time only to pull the nested cache field for a
/// Prometheus counter. OBSERVABILITY ONLY — never reaches billing or the wire.
///
/// OpenAI reports cache READS via `usage.prompt_tokens_details.cached_tokens`
/// (verified against developers.openai.com prompt-caching docs 2026-06-17).
/// There is no separate cache-WRITE count, so the metering write counter stays
/// 0 for OpenAI. Both layers are `Option`/`#[serde(default)]` so a response
/// without the nested object yields `CacheUsage::default()` (0/0), never an
/// error.
#[derive(Debug, Default, Deserialize)]
struct OpenAiExtendedUsage {
    #[serde(default)]
    prompt_tokens_details: Option<OpenAiPromptTokensDetails>,
}

#[derive(Debug, Default, Deserialize)]
struct OpenAiPromptTokensDetails {
    #[serde(default)]
    cached_tokens: u32,
}

/// Extract the cache-read token count from an OpenAI-family response body.
///
/// Reads the top-level `usage.prompt_tokens_details.cached_tokens`. Returns
/// `CacheUsage::default()` (0/0) when `usage` or the nested object is absent —
/// fail-safe, never an error. Shared by OpenAI and xAI (xAI mirrors OpenAI's
/// usage shape).
pub(super) fn cache_usage_from_openai_body(body: &serde_json::Value) -> CacheUsage {
    // Absent `usage` (or no-cache) ⇒ 0 silently — that is the normal fail-safe.
    // A `usage` block that is PRESENT but fails to deserialize is a real error
    // (e.g. the provider sends `cached_tokens` as a string/float/null, or
    // renames the field shape): log it before falling back to 0 so the counter
    // reading 0 is not silent (Finding 1). Only an actual deserialize error
    // warns; the absent/no-cache path stays quiet.
    let read = match body.get("usage") {
        None => 0,
        Some(usage) => match serde_json::from_value::<OpenAiExtendedUsage>(usage.clone()) {
            Ok(parsed) => parsed
                .prompt_tokens_details
                .map(|d| d.cached_tokens)
                .unwrap_or(0),
            Err(e) => {
                warn!(
                    error = %e,
                    "openai: failed to parse usage details for cache metering; cache counts will be 0"
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

fn strip_model_prefix(model: &str) -> String {
    model
        .strip_prefix(OPENAI_PREFIX)
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
impl LLMProvider for OpenAIProvider {
    fn name(&self) -> &str {
        "openai"
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
                .post(OPENAI_URL)
                .timeout(super::PROVIDER_REQUEST_TIMEOUT)
                .bearer_auth(&self.api_key)
                .json(&req_body)
                .send()
        })
        .await?;

        // Read the body once as a Value so we can BOTH (a) deserialize the
        // public `ChatResponse` for the agent and (b) read the nested
        // `usage.prompt_tokens_details.cached_tokens` for OBSERVABILITY-ONLY
        // metering — without extending the frozen public `Usage` type. The
        // billing path is untouched: `ChatResponse` is built exactly as before.
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
            .post(OPENAI_URL)
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
        assert_eq!(strip_model_prefix("openai/gpt-4o"), "gpt-4o");
    }

    #[test]
    fn strip_model_prefix_leaves_other_models_unchanged() {
        assert_eq!(strip_model_prefix("gpt-4o-mini"), "gpt-4o-mini");
        assert_eq!(strip_model_prefix("anthropic/claude"), "anthropic/claude");
    }

    #[test]
    fn strip_model_prefix_handles_empty_remainder() {
        // Defensive: a bare prefix should produce an empty model name rather than panic.
        assert_eq!(strip_model_prefix("openai/"), "");
    }

    #[test]
    fn build_chat_body_strips_prefix_and_serializes() {
        let req = sample_request("openai/gpt-4o");
        let body = build_chat_body(&req).expect("body must serialize");
        assert_eq!(body["model"], "gpt-4o");
        assert_eq!(body["messages"][0]["content"], "hi");
        // Non-streaming bodies must not set stream=true (would corrupt downstream parsing).
        assert!(body.get("stream").is_none() || body["stream"] == serde_json::Value::Bool(false));
    }

    #[test]
    fn build_chat_body_preserves_unprefixed_model() {
        let req = sample_request("gpt-4o-mini");
        let body = build_chat_body(&req).expect("body must serialize");
        assert_eq!(body["model"], "gpt-4o-mini");
    }

    #[test]
    fn build_chat_body_passes_text_parts_through_as_array() {
        use solvela_protocol::vision::{ContentPart, MessageContent};
        let mut req = sample_request("openai/gpt-4o");
        req.messages[0].content = MessageContent::Parts(vec![
            ContentPart::Text {
                text: "Hello!".to_string(),
            },
            ContentPart::Text {
                text: "world".to_string(),
            },
        ]);
        let body = build_chat_body(&req).expect("body must serialize");
        // OpenAI-format providers pass array content through natively rather
        // than flattening it — the upstream API consumes the parts directly.
        assert!(
            body["messages"][0]["content"].is_array(),
            "text Parts content must serialize as a JSON array for OpenAI passthrough"
        );
        assert_eq!(body["messages"][0]["content"][0]["type"], "text");
        assert_eq!(body["messages"][0]["content"][0]["text"], "Hello!");
    }

    #[test]
    fn build_chat_body_passes_image_parts_through_as_array() {
        use solvela_protocol::vision::{ContentPart, ImageUrl, MessageContent};
        let mut req = sample_request("openai/gpt-4o");
        req.messages[0].content = MessageContent::Parts(vec![
            ContentPart::Text {
                text: "what is this?".to_string(),
            },
            ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: "https://example.com/cat.png".to_string(),
                    detail: Some("high".to_string()),
                },
            },
        ]);
        let body = build_chat_body(&req).expect("body must serialize");
        // The OpenAI passthrough must serialize `content` as a JSON ARRAY (not
        // a stringified/dropped value) so the image reaches the upstream API.
        assert!(
            body["messages"][0]["content"].is_array(),
            "image content must serialize as a JSON array, not stringified/dropped"
        );
        assert_eq!(body["messages"][0]["content"][1]["type"], "image_url");
        assert_eq!(
            body["messages"][0]["content"][1]["image_url"]["url"],
            "https://example.com/cat.png"
        );
        assert_eq!(
            body["messages"][0]["content"][1]["image_url"]["detail"],
            "high"
        );
    }

    #[test]
    fn build_stream_body_injects_stream_true() {
        let req = sample_request("openai/gpt-4o");
        let body = build_stream_body(&req).expect("body must serialize");
        assert_eq!(body["model"], "gpt-4o");
        assert_eq!(body["stream"], serde_json::Value::Bool(true));
    }

    #[test]
    fn build_stream_body_does_not_mutate_caller() {
        let req = sample_request("openai/gpt-4o");
        let _ = build_stream_body(&req).expect("body must serialize");
        // The original request still carries the prefixed model — proves we cloned.
        assert_eq!(req.model, "openai/gpt-4o");
    }

    #[test]
    fn provider_name_is_stable() {
        let provider = OpenAIProvider::new(reqwest::Client::new(), "sk-test".to_string());
        assert_eq!(provider.name(), "openai");
    }

    #[test]
    fn supported_models_is_empty_by_design() {
        let provider = OpenAIProvider::new(reqwest::Client::new(), "sk-test".to_string());
        assert!(provider.supported_models().is_empty());
    }

    // -----------------------------------------------------------------------
    // PR-2 cache-token metering (OBSERVABILITY ONLY).
    // -----------------------------------------------------------------------

    use crate::cache::test_metrics::{counter_value_filtered, install_test_recorder};

    /// A response body WITH `usage.prompt_tokens_details.cached_tokens` yields a
    /// `CacheUsage` carrying the cached count as the READ (OpenAI has no
    /// separate cache-write count, so write stays 0).
    #[test]
    fn cache_usage_from_openai_body_reads_cached_tokens() {
        let body = serde_json::json!({
            "usage": {
                "prompt_tokens": 1000,
                "completion_tokens": 20,
                "total_tokens": 1020,
                "prompt_tokens_details": { "cached_tokens": 768 }
            }
        });
        let cu = cache_usage_from_openai_body(&body);
        assert_eq!(cu.cache_read_tokens, 768);
        assert_eq!(cu.cache_write_tokens, 0);
    }

    /// A response body WITHOUT the nested details (or without `usage`) yields
    /// `CacheUsage::default()` (0/0) — never a parse error (backward-compat).
    #[test]
    fn cache_usage_from_openai_body_defaults_when_absent() {
        // No prompt_tokens_details.
        let body = serde_json::json!({
            "usage": { "prompt_tokens": 1000, "completion_tokens": 20, "total_tokens": 1020 }
        });
        assert_eq!(cache_usage_from_openai_body(&body), CacheUsage::default());

        // No usage block at all.
        let body = serde_json::json!({ "id": "x", "choices": [] });
        assert_eq!(cache_usage_from_openai_body(&body), CacheUsage::default());
    }

    /// A `usage` block whose `prompt_tokens_details.cached_tokens` has the WRONG
    /// type (string instead of integer) is a real deserialize error: the parser
    /// must fall back to `CacheUsage::default()` (0/0) — never panic — while
    /// still flagging the failure (Finding 1: the old `.ok()` swallowed this
    /// silently, so the counter read 0 with no signal). Falsifiable: an `unwrap`
    /// on the parse Result would panic here.
    #[test]
    fn cache_usage_from_openai_body_falls_back_on_parse_error() {
        // `cached_tokens` as a string is not deserializable into u32.
        let body = serde_json::json!({
            "usage": {
                "prompt_tokens": 1000,
                "prompt_tokens_details": { "cached_tokens": "not-a-number" }
            }
        });
        assert_eq!(cache_usage_from_openai_body(&body), CacheUsage::default());

        // `cached_tokens` as a negative number also fails u32 deserialization.
        let body = serde_json::json!({
            "usage": { "prompt_tokens_details": { "cached_tokens": -5 } }
        });
        assert_eq!(cache_usage_from_openai_body(&body), CacheUsage::default());
    }

    /// Emitting the parsed `CacheUsage` increments the read counter by the
    /// cached-token count and the denominator by 1. (The `chat_completion` path
    /// wires `cache_usage_from_openai_body(...).emit(...)`; this proves the
    /// parse+emit composition end to end at the unit level.)
    #[test]
    fn openai_emit_increments_read_and_denominator() {
        let handle = install_test_recorder();
        let model = "openai/metering-unique-model";
        let key = format!("model=\"{model}\"");
        let read_before =
            counter_value_filtered(&handle, "solvela_provider_cache_read_tokens_total", &key);
        let req_before = counter_value_filtered(&handle, "solvela_provider_requests_total", &key);

        let body = serde_json::json!({
            "usage": { "prompt_tokens_details": { "cached_tokens": 512 } }
        });
        cache_usage_from_openai_body(&body).emit(PROVIDER_LABEL, model);

        let read_after =
            counter_value_filtered(&handle, "solvela_provider_cache_read_tokens_total", &key);
        let req_after = counter_value_filtered(&handle, "solvela_provider_requests_total", &key);
        assert_eq!(read_after - read_before, 512);
        assert_eq!(req_after - req_before, 1);
    }
}
