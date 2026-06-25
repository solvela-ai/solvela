use async_trait::async_trait;

use solvela_protocol::{ChatRequest, ChatResponse, ModelRegistration};

use super::{ChatStream, LLMProvider, ProviderError};

/// Solvela's canonical provider prefix for NVIDIA-routed models.
///
/// NOTE: this is the *Solvela* prefix, NOT a publisher prefix. The model the
/// gateway hands us is `nvidia/<publisher-qualified-id>` where the
/// publisher-qualified id is itself the string the NVIDIA NIM API expects
/// (e.g. `nvidia/llama-3.1-nemotron-nano-8b-v1`, `meta/llama-4-...`). We strip
/// exactly ONE leading `nvidia/` to recover that publisher-qualified id —
/// unlike xai.rs, we must not over-strip, because NVIDIA's own models are
/// themselves published under the `nvidia/` namespace.
const NVIDIA_PREFIX: &str = "nvidia/";

/// NVIDIA NIM OpenAI-compatible chat completions endpoint.
const NVIDIA_URL: &str = "https://integrate.api.nvidia.com/v1/chat/completions";

/// NVIDIA NIM provider adapter.
///
/// NVIDIA NIM exposes an OpenAI-compatible API at `integrate.api.nvidia.com`
/// (bearer auth, standard OpenAI request/response + SSE streaming). Requests
/// pass through with only the base URL changed and the outgoing `model` field
/// normalized back to the publisher-qualified id NVIDIA expects.
pub struct NvidiaProvider {
    api_key: String,
    client: reqwest::Client,
}

impl NvidiaProvider {
    pub fn new(client: reqwest::Client, api_key: String) -> Self {
        Self { api_key, client }
    }
}

/// Normalize a Solvela model string into the publisher-qualified id the NVIDIA
/// NIM API expects in the `model` field.
///
/// The NVIDIA NIM API addresses every model by a publisher-qualified id of the
/// form `<publisher>/<model>` — e.g. `nvidia/llama-3.1-nemotron-nano-8b-v1`,
/// `meta/llama-4-maverick-17b-128e-instruct`, `deepseek-ai/deepseek-r1`.
///
/// In Solvela's registry each NVIDIA model is stored with `provider = "nvidia"`
/// and `model_id = "<publisher>/<model>"` (the FULL publisher-qualified id), so
/// the canonical Solvela key built by the registry is
/// `nvidia/<publisher>/<model>` (see `crates/router/src/models.rs`:
/// `format!("{}/{}", provider, model_id)`).
///
/// What this function must produce, by the path `req.model` arrives through:
///
/// 1. Canonical key for an nvidia-published model
///    `"nvidia/nvidia/llama-3.1-nemotron-nano-8b-v1"`
///    → strip ONE `nvidia/` → `"nvidia/llama-3.1-nemotron-nano-8b-v1"`. CORRECT.
/// 2. Canonical key for a meta-published model
///    `"nvidia/meta/llama-4-maverick-17b-128e-instruct"`
///    → strip ONE `nvidia/` → `"meta/llama-4-maverick-17b-128e-instruct"`. CORRECT.
/// 3. Bare publisher-qualified id with a non-`nvidia` publisher
///    `"meta/llama-4-maverick-17b-128e-instruct"` (e.g. arriving from the
///    fallback-preference header path, which splits on the FIRST `/` and so
///    strips the Solvela `nvidia/` prefix off, leaving the publisher-qualified
///    id intact) → no leading `nvidia/` to strip → passes through unchanged.
///    CORRECT.
/// 4. Bare model name with NO publisher prefix at all (defensive)
///    `"llama-3.1-nemotron-nano-8b-v1"` — this can only arise from the
///    fallback-preference header path when the user wrote a Solvela canonical
///    key whose publisher happens to be `nvidia`
///    (`X-Solvela-Fallback-Preference: nvidia/llama-3.1-nemotron-nano-8b-v1`):
///    that parser splits on the first `/` and hands the adapter the bare
///    `llama-3.1-nemotron-nano-8b-v1` (the `nvidia` publisher segment is
///    consumed as the provider key). We pass it through UNCHANGED. We do NOT
///    re-prepend `nvidia/`: doing so would be a guess (the publisher is
///    unknowable from a bare name — it might be a meta/qwen/deepseek model) and
///    a wrong guess silently mis-routes to a different model. Passing the bare
///    name through means NVIDIA returns a clear "model not found" error rather
///    than silently serving — and billing for — the wrong model. The
///    fallback-preference header is therefore unsuitable for slash-containing
///    NVIDIA ids; address NVIDIA models by their canonical key or TOML key
///    (cases 1/2), which always preserve the full publisher-qualified id.
///
/// The strip is performed AT MOST ONCE (`strip_prefix`, not a loop) so a model
/// whose publisher is literally `nvidia` keeps its `nvidia/` publisher segment.
fn nvidia_model_id(model: &str) -> String {
    model
        .strip_prefix(NVIDIA_PREFIX)
        .unwrap_or(model)
        .to_string()
}

fn build_chat_body(req: &ChatRequest) -> Result<serde_json::Value, serde_json::Error> {
    let mut req = req.clone();
    req.model = nvidia_model_id(&req.model);
    serde_json::to_value(&req)
}

fn build_stream_body(req: &ChatRequest) -> Result<serde_json::Value, serde_json::Error> {
    let mut body = build_chat_body(req)?;
    body["stream"] = serde_json::Value::Bool(true);
    Ok(body)
}

#[async_trait]
impl LLMProvider for NvidiaProvider {
    fn name(&self) -> &str {
        "nvidia"
    }

    fn supported_models(&self) -> Vec<ModelRegistration> {
        // Models are loaded from config, not hardcoded here.
        vec![]
    }

    async fn chat_completion(
        &self,
        req: ChatRequest,
    ) -> Result<ChatResponse, Box<dyn std::error::Error + Send + Sync>> {
        let req_body = build_chat_body(&req)?;
        let response = super::retry_with_backoff(2, || {
            self.client
                .post(NVIDIA_URL)
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
            .post(NVIDIA_URL)
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

    // ── nvidia_model_id round-trip — the money-relevant correctness surface ──
    // A wrong outgoing model id silently routes to the wrong NVIDIA model (or
    // none), so each case asserts the EXACT string NVIDIA must receive.

    #[test]
    fn nvidia_model_id_canonical_key_nvidia_published() {
        // Case 1: canonical Solvela key for an nvidia-published model.
        // req.model = "nvidia/<model_id>" where model_id = "nvidia/llama-...".
        assert_eq!(
            nvidia_model_id("nvidia/nvidia/llama-3.1-nemotron-nano-8b-v1"),
            "nvidia/llama-3.1-nemotron-nano-8b-v1"
        );
    }

    #[test]
    fn nvidia_model_id_canonical_key_meta_published() {
        // Case 2: canonical Solvela key for a meta-published model.
        // req.model = "nvidia/<model_id>" where model_id = "meta/llama-4-...".
        assert_eq!(
            nvidia_model_id("nvidia/meta/llama-4-maverick-17b-128e-instruct"),
            "meta/llama-4-maverick-17b-128e-instruct"
        );
    }

    #[test]
    fn nvidia_model_id_canonical_key_other_publishers() {
        // Same case-2 shape for the other confirmed publishers.
        assert_eq!(
            nvidia_model_id("nvidia/deepseek-ai/deepseek-r1"),
            "deepseek-ai/deepseek-r1"
        );
        assert_eq!(
            nvidia_model_id("nvidia/qwen/qwen3-coder-480b-a35b-instruct"),
            "qwen/qwen3-coder-480b-a35b-instruct"
        );
        assert_eq!(
            nvidia_model_id("nvidia/mistralai/mistral-large-3-675b-instruct-2512"),
            "mistralai/mistral-large-3-675b-instruct-2512"
        );
        assert_eq!(
            nvidia_model_id("nvidia/minimaxai/minimax-m2.7"),
            "minimaxai/minimax-m2.7"
        );
    }

    #[test]
    fn nvidia_model_id_bare_publisher_qualified_passes_through() {
        // Case 3: a bare publisher-qualified id (publisher != "nvidia") with no
        // leading Solvela prefix — e.g. from the fallback-preference header
        // split — must pass through UNCHANGED.
        assert_eq!(
            nvidia_model_id("meta/llama-4-maverick-17b-128e-instruct"),
            "meta/llama-4-maverick-17b-128e-instruct"
        );
        assert_eq!(
            nvidia_model_id("deepseek-ai/deepseek-r1"),
            "deepseek-ai/deepseek-r1"
        );
    }

    #[test]
    fn nvidia_model_id_strips_only_one_nvidia_prefix() {
        // The strip is at-most-once: an nvidia-published model addressed by its
        // canonical key keeps its `nvidia/` PUBLISHER segment. (This is exactly
        // why we cannot reuse xai.rs's strip-the-provider-prefix shape.)
        assert_eq!(
            nvidia_model_id("nvidia/nvidia/nemotron-mini-4b-instruct"),
            "nvidia/nemotron-mini-4b-instruct"
        );
        // The result still begins with `nvidia/` (the publisher), proving we did
        // not over-strip.
        assert!(nvidia_model_id("nvidia/nvidia/nemotron-mini-4b-instruct").starts_with("nvidia/"));
    }

    #[test]
    fn nvidia_model_id_bare_name_no_publisher_passes_through_documented() {
        // Case 4 (defensive): a bare model name with no publisher prefix — the
        // residue when the fallback-preference parser splits `nvidia/llama-...`
        // on the first `/` and consumes the `nvidia` segment as the provider
        // key. We pass it through UNCHANGED and DO NOT re-prepend a publisher
        // (which would be an unrecoverable wrong guess). NVIDIA then returns a
        // clear not-found error rather than silently mis-billing.
        assert_eq!(
            nvidia_model_id("llama-3.1-nemotron-nano-8b-v1"),
            "llama-3.1-nemotron-nano-8b-v1"
        );
        // No silent re-prefixing: the bare name is NOT turned back into
        // "nvidia/llama-...".
        assert_ne!(
            nvidia_model_id("llama-3.1-nemotron-nano-8b-v1"),
            "nvidia/llama-3.1-nemotron-nano-8b-v1"
        );
    }

    #[test]
    fn nvidia_model_id_handles_empty_remainder() {
        // Degenerate "nvidia/" alone strips to empty (mirrors xai's behavior on
        // a malformed prefix-only model).
        assert_eq!(nvidia_model_id("nvidia/"), "");
    }

    // ── build_chat_body / build_stream_body ──────────────────────────────────

    #[test]
    fn build_chat_body_normalizes_model_and_serializes() {
        let req = sample_request("nvidia/meta/llama-4-maverick-17b-128e-instruct");
        let body = build_chat_body(&req).expect("body must serialize");
        assert_eq!(body["model"], "meta/llama-4-maverick-17b-128e-instruct");
        assert_eq!(body["messages"][0]["content"], "hi");
        assert!(body.get("stream").is_none() || body["stream"] == serde_json::Value::Bool(false));
    }

    #[test]
    fn build_chat_body_normalizes_nvidia_published_model() {
        let req = sample_request("nvidia/nvidia/llama-3.1-nemotron-nano-8b-v1");
        let body = build_chat_body(&req).expect("body must serialize");
        assert_eq!(body["model"], "nvidia/llama-3.1-nemotron-nano-8b-v1");
    }

    #[test]
    fn build_chat_body_preserves_bare_publisher_qualified_model() {
        let req = sample_request("meta/llama-3.3-70b-instruct");
        let body = build_chat_body(&req).expect("body must serialize");
        assert_eq!(body["model"], "meta/llama-3.3-70b-instruct");
    }

    #[test]
    fn build_chat_body_passes_text_parts_through_as_array() {
        use solvela_protocol::vision::{ContentPart, MessageContent};
        let mut req = sample_request("nvidia/meta/llama-4-scout-17b-16e-instruct");
        req.messages[0].content = MessageContent::Parts(vec![
            ContentPart::Text {
                text: "Hello!".to_string(),
            },
            ContentPart::Text {
                text: "world".to_string(),
            },
        ]);
        let body = build_chat_body(&req).expect("body must serialize");
        // NVIDIA NIM is OpenAI-compatible: array content passes through natively
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
        let req = sample_request("nvidia/deepseek-ai/deepseek-r1");
        let body = build_stream_body(&req).expect("body must serialize");
        assert_eq!(body["model"], "deepseek-ai/deepseek-r1");
        assert_eq!(body["stream"], serde_json::Value::Bool(true));
    }

    #[test]
    fn build_stream_body_does_not_mutate_caller() {
        let req = sample_request("nvidia/meta/llama-4-maverick-17b-128e-instruct");
        let _ = build_stream_body(&req).expect("body must serialize");
        // The caller's model string is untouched (normalization clones).
        assert_eq!(req.model, "nvidia/meta/llama-4-maverick-17b-128e-instruct");
    }

    #[test]
    fn provider_name_is_stable() {
        let provider = NvidiaProvider::new(reqwest::Client::new(), "nvidia-test".to_string());
        assert_eq!(provider.name(), "nvidia");
    }

    #[test]
    fn supported_models_is_empty_by_design() {
        let provider = NvidiaProvider::new(reqwest::Client::new(), "nvidia-test".to_string());
        assert!(provider.supported_models().is_empty());
    }
}
