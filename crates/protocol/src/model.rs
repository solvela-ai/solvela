use serde::{Deserialize, Serialize};

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_info_capability_fields() {
        let json = r#"{"id":"openai/gpt-4o","provider":"openai","model_id":"gpt-4o","display_name":"GPT-4o","input_cost_per_million":2.5,"output_cost_per_million":10.0,"context_window":128000,"supports_streaming":true,"supports_tools":true,"supports_vision":true,"reasoning":false,"supports_structured_output":true,"supports_batch":true,"max_output_tokens":16384}"#;
        let info: ModelInfo = serde_json::from_str(json).unwrap();
        assert!(info.supports_structured_output);
        assert!(info.supports_batch);
        assert_eq!(info.max_output_tokens, Some(16384));
    }

    #[test]
    fn test_model_info_backward_compat() {
        let json = r#"{"id":"openai/gpt-4o","provider":"openai","model_id":"gpt-4o","display_name":"GPT-4o","input_cost_per_million":2.5,"output_cost_per_million":10.0,"context_window":128000}"#;
        let info: ModelInfo = serde_json::from_str(json).unwrap();
        assert!(!info.supports_structured_output);
        assert!(!info.supports_batch);
        assert_eq!(info.max_output_tokens, None);
        assert!(!info.supports_streaming);
        assert!(!info.supports_tools);
        assert!(!info.supports_vision);
        assert!(!info.reasoning);
    }
}
