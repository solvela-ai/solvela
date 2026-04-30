//! Demo provider — a zero-config echo provider used as the "first 5 minutes"
//! path so a fresh clone of Solvela can answer `/v1/chat/completions` without
//! any provider API keys configured.
//!
//! Activated when:
//!   1. `SOLVELA_DEMO_MODE=true` is set, OR
//!   2. No real providers are configured AND the resolved model is `demo`
//!      (or `solvela/demo`).
//!
//! The demo provider returns a canned echo of the user's prompt plus a hint
//! that real providers must be configured for production use. Token usage is
//! always zero, so cost computation produces $0 and any escrow claim is
//! skipped by the existing `claim_amount == 0` guard in the chat route.
//!
//! Pattern adapted from BlockRunAI/Franklin's local stub provider — see
//! `franklin/src/providers/stub.ts` for the original echo-style behavior.

use std::time::Duration;

use async_trait::async_trait;
use futures::stream;

use solvela_protocol::{
    ChatChoice, ChatChunk, ChatChunkChoice, ChatDelta, ChatMessage, ChatRequest, ChatResponse,
    ModelInfo, Role, Usage,
};

use super::{ChatStream, LLMProvider, ProviderError};

/// Maximum prompt characters echoed back. Keeps the canned response small
/// regardless of how large the inbound prompt is.
const PROMPT_ECHO_MAX_CHARS: usize = 100;

/// Stable model identifier exposed to callers. The slash form
/// (`solvela/demo`) is what the model registry stores; `demo` is also accepted
/// as an alias by [`is_demo_model`].
pub const DEMO_MODEL_ID: &str = "solvela/demo";

/// Provider name used in the registry.
pub const DEMO_PROVIDER_NAME: &str = "solvela";

/// Number of streaming chunks the demo provider emits before `[DONE]`.
const DEMO_STREAM_CHUNKS: usize = 3;

/// Returns `true` when `model` resolves to the demo provider's model.
///
/// Accepts the canonical `solvela/demo` ID, the bare alias `demo`, and
/// case-insensitive variants of either.
pub fn is_demo_model(model: &str) -> bool {
    let lower = model.to_ascii_lowercase();
    lower == "demo" || lower == DEMO_MODEL_ID
}

/// Build the canned demo response text from the inbound chat request.
fn build_demo_text(req: &ChatRequest) -> String {
    let last_user = req
        .messages
        .iter()
        .rev()
        .find(|m| m.role == Role::User)
        .map(|m| m.content.as_str())
        .unwrap_or("(no user message)");

    let truncated: String = last_user.chars().take(PROMPT_ECHO_MAX_CHARS).collect();

    format!(
        "Solvela demo mode — your prompt was: {truncated}. \
         Configure a real provider in .env to use this for production."
    )
}

/// Stable response/chunk ID for the demo provider.
fn demo_response_id() -> String {
    format!("solvela-demo-{}", chrono::Utc::now().timestamp_millis())
}

/// The demo provider — zero-config, returns canned echo responses.
///
/// Implements [`LLMProvider`] so it slots into the same registry/fallback
/// pipeline as real providers. See module docs for activation rules.
pub struct DemoProvider;

impl DemoProvider {
    pub fn new() -> Self {
        Self
    }
}

impl Default for DemoProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LLMProvider for DemoProvider {
    fn name(&self) -> &str {
        DEMO_PROVIDER_NAME
    }

    fn supported_models(&self) -> Vec<ModelInfo> {
        // Models are loaded from config/models.toml — keep this list empty
        // to match other adapters.
        vec![]
    }

    async fn chat_completion(&self, req: ChatRequest) -> Result<ChatResponse, ProviderError> {
        let text = build_demo_text(&req);
        let now = chrono::Utc::now().timestamp();

        Ok(ChatResponse {
            id: demo_response_id(),
            object: "chat.completion".to_string(),
            created: now,
            model: req.model,
            choices: vec![ChatChoice {
                index: 0,
                message: ChatMessage {
                    role: Role::Assistant,
                    content: text,
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: Some(Usage {
                prompt_tokens: 0,
                completion_tokens: 0,
                total_tokens: 0,
            }),
        })
    }

    async fn chat_completion_stream(&self, req: ChatRequest) -> Result<ChatStream, ProviderError> {
        let text = build_demo_text(&req);
        let id = demo_response_id();
        let model = req.model.clone();
        let now = chrono::Utc::now().timestamp();

        // Split the text into roughly-equal slices for `DEMO_STREAM_CHUNKS`
        // chunks so the SSE path is exercised end-to-end.
        let chunks = split_into_chunks(&text, DEMO_STREAM_CHUNKS);

        // Build a series of `ChatChunk`s. The last chunk carries
        // `finish_reason = "stop"` (per the OpenAI streaming contract).
        let mut events: Vec<Result<ChatChunk, ProviderError>> = Vec::new();
        let total = chunks.len();
        for (idx, slice) in chunks.into_iter().enumerate() {
            let is_last = idx + 1 == total;
            events.push(Ok(ChatChunk {
                id: id.clone(),
                object: "chat.completion.chunk".to_string(),
                created: now,
                model: model.clone(),
                choices: vec![ChatChunkChoice {
                    index: 0,
                    delta: ChatDelta {
                        role: if idx == 0 {
                            Some(Role::Assistant)
                        } else {
                            None
                        },
                        content: Some(slice),
                        tool_calls: None,
                    },
                    finish_reason: if is_last {
                        Some("stop".to_string())
                    } else {
                        None
                    },
                }],
            }));
        }

        // Tiny per-chunk delay so downstream heartbeat / SSE buffering can
        // drain individual events instead of fusing them into one frame.
        let stream = stream::unfold(events.into_iter(), |mut iter| async move {
            let next = iter.next()?;
            tokio::time::sleep(Duration::from_millis(5)).await;
            Some((next, iter))
        });

        Ok(Box::pin(stream))
    }
}

/// Split `text` into at most `count` non-empty slices of roughly equal size.
///
/// Returns `[text]` when `count <= 1` or `text` is empty.
fn split_into_chunks(text: &str, count: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    if count <= 1 {
        return vec![text.to_string()];
    }

    let bytes = text.as_bytes();
    let chunk_size = bytes.len().div_ceil(count);
    let mut out: Vec<String> = Vec::with_capacity(count);
    let mut cursor = 0usize;
    while cursor < bytes.len() {
        // Find a UTF-8 safe boundary at or before `cursor + chunk_size`.
        let mut end = (cursor + chunk_size).min(bytes.len());
        while end < bytes.len() && !text.is_char_boundary(end) {
            end += 1;
        }
        // safe: bounded by `is_char_boundary` checks above.
        out.push(text[cursor..end].to_string());
        cursor = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user_request(content: &str, model: &str) -> ChatRequest {
        ChatRequest {
            model: model.to_string(),
            messages: vec![ChatMessage {
                role: Role::User,
                content: content.to_string(),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            max_tokens: None,
            temperature: None,
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
        }
    }

    #[test]
    fn is_demo_model_accepts_aliases() {
        assert!(is_demo_model("demo"));
        assert!(is_demo_model("DEMO"));
        assert!(is_demo_model("solvela/demo"));
        assert!(is_demo_model("Solvela/Demo"));
        assert!(!is_demo_model("openai/gpt-4o"));
        assert!(!is_demo_model(""));
    }

    #[test]
    fn build_demo_text_truncates_long_prompt() {
        let long_prompt = "x".repeat(500);
        let req = user_request(&long_prompt, DEMO_MODEL_ID);
        let text = build_demo_text(&req);

        // The truncated prompt slice should be exactly `PROMPT_ECHO_MAX_CHARS`
        // characters long, regardless of input size.
        let echoed = text
            .strip_prefix("Solvela demo mode — your prompt was: ")
            .expect("response begins with the demo header");
        let echoed_prefix: String = echoed.chars().take(PROMPT_ECHO_MAX_CHARS).collect();
        assert_eq!(echoed_prefix.chars().count(), PROMPT_ECHO_MAX_CHARS);
    }

    #[test]
    fn build_demo_text_handles_no_user_message() {
        let req = ChatRequest {
            model: DEMO_MODEL_ID.to_string(),
            messages: vec![ChatMessage {
                role: Role::System,
                content: "you are helpful".to_string(),
                name: None,
                tool_calls: None,
                tool_call_id: None,
            }],
            max_tokens: None,
            temperature: None,
            top_p: None,
            stream: false,
            tools: None,
            tool_choice: None,
        };
        let text = build_demo_text(&req);
        assert!(text.contains("(no user message)"));
    }

    #[tokio::test]
    async fn chat_completion_returns_zero_token_usage() {
        let provider = DemoProvider::new();
        let req = user_request("hello world", DEMO_MODEL_ID);

        let response = provider.chat_completion(req).await.expect("demo succeeds");

        let usage = response.usage.expect("demo always reports usage");
        assert_eq!(usage.prompt_tokens, 0);
        assert_eq!(usage.completion_tokens, 0);
        assert_eq!(usage.total_tokens, 0);

        let choice = response.choices.first().expect("at least one choice");
        assert_eq!(choice.message.role, Role::Assistant);
        assert!(choice.message.content.contains("Solvela demo mode"));
        assert!(choice.message.content.contains("hello world"));
    }

    #[tokio::test]
    async fn chat_completion_stream_emits_chunks_and_terminates() {
        use futures::StreamExt;

        let provider = DemoProvider::new();
        let req = user_request("streaming please", DEMO_MODEL_ID);

        let mut stream = provider
            .chat_completion_stream(req)
            .await
            .expect("demo streams");

        let mut chunks: Vec<ChatChunk> = Vec::new();
        while let Some(item) = stream.next().await {
            chunks.push(item.expect("demo stream items are Ok"));
        }

        assert!(
            !chunks.is_empty() && chunks.len() <= DEMO_STREAM_CHUNKS,
            "expected 1..={} chunks, got {}",
            DEMO_STREAM_CHUNKS,
            chunks.len()
        );

        // The first chunk announces the assistant role.
        let first = chunks.first().expect("at least one chunk");
        assert_eq!(first.choices[0].delta.role, Some(Role::Assistant));

        // The last chunk carries finish_reason = "stop".
        let last = chunks.last().expect("at least one chunk");
        assert_eq!(last.choices[0].finish_reason.as_deref(), Some("stop"));

        // Reassembling the deltas reproduces the canned response prefix.
        let assembled: String = chunks
            .iter()
            .filter_map(|c| c.choices[0].delta.content.clone())
            .collect();
        assert!(assembled.contains("Solvela demo mode"));
        assert!(assembled.contains("streaming please"));
    }

    #[test]
    fn split_into_chunks_returns_at_most_count() {
        let text = "abcdefghij";
        let chunks = split_into_chunks(text, 3);
        assert!(chunks.len() <= 3);
        assert_eq!(chunks.concat(), text);
    }

    #[test]
    fn split_into_chunks_handles_empty() {
        let chunks = split_into_chunks("", 3);
        assert_eq!(chunks, vec![String::new()]);
    }

    #[test]
    fn split_into_chunks_count_one_returns_whole() {
        let chunks = split_into_chunks("hello", 1);
        assert_eq!(chunks, vec!["hello".to_string()]);
    }
}
