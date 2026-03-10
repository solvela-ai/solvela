pub mod anthropic;
pub mod deepseek;
pub mod fallback;
pub mod google;
pub mod health;
pub mod heartbeat;
pub mod openai;
pub mod xai;

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::Stream;

use rustyclaw_protocol::{ChatChunk, ChatRequest, ChatResponse, ModelInfo};

/// Error type for provider operations.
pub type ProviderError = Box<dyn std::error::Error + Send + Sync>;

/// A boxed stream of chat completion chunks.
pub type ChatStream = Pin<Box<dyn Stream<Item = Result<ChatChunk, ProviderError>> + Send>>;

/// Trait for LLM provider adapters.
///
/// Each provider translates between the OpenAI-compatible gateway format
/// and the provider's native API format. Implementations must be Send + Sync
/// for use in Axum's concurrent request handling.
#[async_trait]
pub trait LLMProvider: Send + Sync {
    /// Provider name (e.g., "openai", "anthropic").
    fn name(&self) -> &str;

    /// List of models this provider supports.
    fn supported_models(&self) -> Vec<ModelInfo>;

    /// Execute a chat completion request.
    async fn chat_completion(
        &self,
        req: ChatRequest,
    ) -> Result<ChatResponse, Box<dyn std::error::Error + Send + Sync>>;

    /// Execute a streaming chat completion request.
    /// Returns a stream of ChatChunk events.
    /// Default implementation returns an error (not all providers support streaming).
    async fn chat_completion_stream(&self, req: ChatRequest) -> Result<ChatStream, ProviderError> {
        let _ = req;
        Err("streaming not supported by this provider".into())
    }
}

/// Spawn an SSE parser for OpenAI-compatible streaming responses.
///
/// Buffers incoming bytes, splits on `\n\n` boundaries, extracts `data: ` lines,
/// and parses each as a `ChatChunk`. Skips `[DONE]` sentinel events.
///
/// This is shared by all OpenAI-compatible providers (OpenAI, DeepSeek, xAI).
pub fn spawn_openai_sse_parser(response: reqwest::Response) -> ChatStream {
    let (mut tx, rx) = futures::channel::mpsc::channel::<Result<ChatChunk, ProviderError>>(32);
    tokio::spawn(async move {
        use futures::{SinkExt, StreamExt};

        let mut byte_stream = response.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk_result) = byte_stream.next().await {
            match chunk_result {
                Ok(bytes) => {
                    buffer.push_str(&String::from_utf8_lossy(&bytes));

                    while let Some(pos) = buffer.find("\n\n") {
                        let event = buffer[..pos].to_string();
                        buffer = buffer[pos + 2..].to_string();

                        for line in event.lines() {
                            if let Some(data) = line.strip_prefix("data: ") {
                                let data = data.trim();
                                if data == "[DONE]" {
                                    return;
                                }
                                match serde_json::from_str::<ChatChunk>(data) {
                                    Ok(chunk) => {
                                        if tx.send(Ok(chunk)).await.is_err() {
                                            return;
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(error = %e, "failed to parse SSE chunk");
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    let _ = tx.send(Err(Box::new(e) as ProviderError)).await;
                    return;
                }
            }
        }
    });
    Box::pin(rx)
}

/// Registry of configured LLM provider adapters.
///
/// Maps provider names (e.g., "openai", "anthropic") to their adapter
/// implementations. Only providers with configured API keys are registered.
pub struct ProviderRegistry {
    providers: HashMap<String, Arc<dyn LLMProvider>>,
}

impl ProviderRegistry {
    /// Create a new provider registry from environment-configured API keys.
    ///
    /// Only providers with valid API keys are registered. If no API keys
    /// are configured, the registry will be empty and all requests will
    /// return stub responses.
    pub fn from_env() -> Self {
        let mut providers: HashMap<String, Arc<dyn LLMProvider>> = HashMap::new();

        if let Ok(key) = std::env::var("OPENAI_API_KEY") {
            if !key.is_empty() {
                providers.insert(
                    "openai".to_string(),
                    Arc::new(openai::OpenAIProvider::new(key)),
                );
            }
        }

        if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
            if !key.is_empty() {
                providers.insert(
                    "anthropic".to_string(),
                    Arc::new(anthropic::AnthropicProvider::new(key)),
                );
            }
        }

        if let Ok(key) = std::env::var("GOOGLE_API_KEY") {
            if !key.is_empty() {
                providers.insert(
                    "google".to_string(),
                    Arc::new(google::GoogleProvider::new(key)),
                );
            }
        }

        if let Ok(key) = std::env::var("XAI_API_KEY") {
            if !key.is_empty() {
                providers.insert("xai".to_string(), Arc::new(xai::XAIProvider::new(key)));
            }
        }

        if let Ok(key) = std::env::var("DEEPSEEK_API_KEY") {
            if !key.is_empty() {
                providers.insert(
                    "deepseek".to_string(),
                    Arc::new(deepseek::DeepSeekProvider::new(key)),
                );
            }
        }

        Self { providers }
    }

    /// Look up a provider by name.
    pub fn get(&self, provider_name: &str) -> Option<&Arc<dyn LLMProvider>> {
        self.providers.get(provider_name)
    }

    /// List all configured provider names.
    pub fn configured_providers(&self) -> Vec<&str> {
        self.providers.keys().map(|s| s.as_str()).collect()
    }
}
