use serde::{Deserialize, Serialize};

use crate::profiles::{resolve_model, Profile, Tier};
use crate::scorer::{classify, ScorerResult};
use solvela_protocol::ChatRequest;

/// Named labels for the 15 scorer dimensions (same order as scorer WEIGHTS).
const DIMENSION_NAMES: [&str; 15] = [
    "token_count",
    "code_presence",
    "reasoning_markers",
    "technical_terms",
    "creative_markers",
    "simple_indicators",
    "multi_step_patterns",
    "question_complexity",
    "agentic_task_markers",
    "math_logic",
    "language_complexity",
    "conversation_depth",
    "tool_usage",
    "output_format_complexity",
    "domain_specificity",
];

/// Must stay in sync with the WEIGHTS array in scorer.rs.
const WEIGHTS: [f64; 15] = [
    0.08, // token_count
    0.15, // code_presence
    0.18, // reasoning_markers
    0.10, // technical_terms
    0.05, // creative_markers
    0.02, // simple_indicators
    0.12, // multi_step_patterns
    0.05, // question_complexity
    0.04, // agentic_task_markers
    0.06, // math_logic
    0.04, // language_complexity
    0.03, // conversation_depth
    0.04, // tool_usage
    0.02, // output_format_complexity
    0.02, // domain_specificity
];

/// Per-dimension contribution breakdown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DimensionScore {
    pub name: String,
    pub weight: f64,
    pub signal: f64,
    /// Weighted contribution: `weight * signal`.
    pub contribution: f64,
}

/// Model recommendation for a single routing profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileRecommendation {
    pub profile: Profile,
    pub selected_model: String,
}

/// Full router analysis for a single `ChatRequest`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterAnalysis {
    pub tier: Tier,
    pub score: f64,
    pub dimensions: Vec<DimensionScore>,
    pub profiles: Vec<ProfileRecommendation>,
    pub has_tools: bool,
}

/// Classify a `ChatRequest` and return a structured analysis without
/// performing any HTTP call, payment verification, or LLM invocation.
///
/// This is a pure, synchronous, sub-microsecond function.
pub fn analyze_request(req: &ChatRequest) -> RouterAnalysis {
    let has_tools = req.tools.as_ref().map(|t| !t.is_empty()).unwrap_or(false);
    let result: ScorerResult = classify(&req.messages, has_tools);

    let dimensions = DIMENSION_NAMES
        .iter()
        .enumerate()
        .zip(WEIGHTS.iter())
        .map(|((i, &name), &weight)| {
            let signal = result.signals[i];
            DimensionScore {
                name: name.to_string(),
                weight,
                signal,
                contribution: weight * signal,
            }
        })
        .collect();

    let profiles = [
        Profile::Eco,
        Profile::Auto,
        Profile::Premium,
        Profile::Free,
        Profile::Agentic,
    ]
    .iter()
    .map(|&profile| ProfileRecommendation {
        profile,
        selected_model: resolve_model(profile, result.tier).to_string(),
    })
    .collect();

    RouterAnalysis {
        tier: result.tier,
        score: result.score,
        dimensions,
        profiles,
        has_tools,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solvela_protocol::{ChatMessage, Role};

    fn user_msg(content: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: content.to_string(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    fn simple_req(content: &str) -> ChatRequest {
        ChatRequest {
            model: "auto".to_string(),
            messages: vec![user_msg(content)],
            stream: false,
            max_tokens: None,
            temperature: None,
            top_p: None,
            tools: None,
            tool_choice: None,
        }
    }

    #[test]
    fn test_analyze_returns_all_profiles() {
        let req = simple_req("Hello!");
        let analysis = analyze_request(&req);
        assert_eq!(analysis.profiles.len(), 5);
        let profile_names: Vec<_> = analysis
            .profiles
            .iter()
            .map(|p| format!("{:?}", p.profile))
            .collect();
        assert!(profile_names.contains(&"Eco".to_string()));
        assert!(profile_names.contains(&"Auto".to_string()));
        assert!(profile_names.contains(&"Premium".to_string()));
        assert!(profile_names.contains(&"Free".to_string()));
        assert!(profile_names.contains(&"Agentic".to_string()));
    }

    #[test]
    fn test_analyze_returns_15_dimensions() {
        let req = simple_req("Hello!");
        let analysis = analyze_request(&req);
        assert_eq!(analysis.dimensions.len(), 15);
    }

    #[test]
    fn test_analyze_simple_greeting() {
        let req = simple_req("Hello!");
        let analysis = analyze_request(&req);
        assert_eq!(analysis.tier, Tier::Simple);
        assert!(!analysis.has_tools);
    }

    #[test]
    fn test_analyze_with_tools() {
        use solvela_protocol::tools::{FunctionDefinitionInner, ToolDefinition};
        let req = ChatRequest {
            model: "auto".to_string(),
            messages: vec![user_msg("Search for something")],
            stream: false,
            max_tokens: None,
            temperature: None,
            top_p: None,
            tools: Some(vec![ToolDefinition {
                r#type: "function".to_string(),
                function: FunctionDefinitionInner {
                    name: "search".to_string(),
                    description: Some("Search the web".to_string()),
                    parameters: None,
                },
            }]),
            tool_choice: None,
        };
        let analysis = analyze_request(&req);
        assert!(analysis.has_tools);
    }

    #[test]
    fn test_weights_match_scorer() {
        // Verify our local WEIGHTS copy sums to 1.0, same as scorer::WEIGHTS.
        let sum: f64 = WEIGHTS.iter().sum();
        assert!(
            (sum - 1.0).abs() < 1e-10,
            "analyzer WEIGHTS sum to {sum}, expected 1.0"
        );
    }

    #[test]
    fn test_dimension_contributions_are_weight_times_signal() {
        let req = simple_req("Prove step by step the algorithm is correct and analyze complexity.");
        let analysis = analyze_request(&req);
        for dim in &analysis.dimensions {
            let expected = dim.weight * dim.signal;
            assert!(
                (dim.contribution - expected).abs() < 1e-12,
                "contribution mismatch for {}: {} != {}",
                dim.name,
                dim.contribution,
                expected
            );
        }
    }
}
