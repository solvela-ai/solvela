use tracing::debug;

use crate::profiles::Tier;

/// Weights for the 15-dimension request scorer.
/// Each dimension contributes a weighted signal to classify request complexity.
const WEIGHTS: [f64; 15] = [
    0.08, // 1.  Token count
    0.15, // 2.  Code presence
    0.18, // 3.  Reasoning markers
    0.10, // 4.  Technical terms
    0.05, // 5.  Creative markers
    0.02, // 6.  Simple indicators (negative)
    0.12, // 7.  Multi-step patterns
    0.05, // 8.  Question complexity
    0.04, // 9.  Agentic task markers
    0.06, // 10. Math/logic
    0.04, // 11. Language complexity
    0.03, // 12. Conversation depth
    0.04, // 13. Tool usage
    0.02, // 14. Output format complexity
    0.02, // 15. Domain specificity
];

/// Score a request across 15 dimensions and return a complexity tier.
///
/// The scorer is purely rule-based with zero external calls, targeting
/// <1 microsecond per classification.
pub fn classify(messages: &[rcr_common::types::ChatMessage], has_tools: bool) -> ScorerResult {
    let text = concatenate_user_content(messages);
    let word_count = text.split_whitespace().count();
    let msg_count = messages.len();

    let mut signals = [0.0_f64; 15];

    // 1. Token count — short messages are simpler
    signals[0] = if word_count < 15 {
        -0.5
    } else if word_count < 50 {
        -0.2
    } else if word_count > 200 {
        0.5
    } else {
        0.0
    };

    // 2. Code presence
    signals[1] = score_code_presence(&text);

    // 3. Reasoning markers
    signals[2] = score_keyword_density(
        &text,
        &[
            "prove",
            "theorem",
            "step by step",
            "reason",
            "analyze",
            "evaluate",
            "compare and contrast",
            "think through",
            "explain why",
        ],
    );

    // 4. Technical terms
    signals[3] = score_keyword_density(
        &text,
        &[
            "algorithm",
            "kubernetes",
            "database",
            "architecture",
            "distributed",
            "concurrent",
            "protocol",
            "optimization",
            "benchmark",
        ],
    );

    // 5. Creative markers
    signals[4] = score_keyword_density(
        &text,
        &[
            "story",
            "poem",
            "brainstorm",
            "creative",
            "imagine",
            "fiction",
            "narrative",
        ],
    );

    // 6. Simple indicators (negative signal — pushes score down)
    signals[5] = -score_keyword_density(
        &text,
        &[
            "what is",
            "define",
            "translate",
            "hello",
            "hi",
            "thanks",
            "yes",
            "no",
        ],
    );

    // 7. Multi-step patterns
    signals[6] = score_keyword_density(
        &text,
        &[
            "first", "then", "next", "finally", "step 1", "step 2", "1.", "2.", "3.",
        ],
    );

    // 8. Question complexity — multiple questions suggest complexity
    let question_marks = text.matches('?').count();
    signals[7] = match question_marks {
        0 => 0.0,
        1 => 0.1,
        2..=3 => 0.4,
        _ => 0.8,
    };

    // 9. Agentic task markers
    signals[8] = score_keyword_density(
        &text,
        &[
            "read file",
            "write file",
            "edit",
            "deploy",
            "execute",
            "run command",
            "install",
        ],
    );

    // 10. Math/logic
    signals[9] = score_math_presence(&text);

    // 11. Language complexity — average word length as proxy
    let avg_word_len = if word_count > 0 {
        text.split_whitespace().map(|w| w.len() as f64).sum::<f64>() / word_count as f64
    } else {
        0.0
    };
    signals[10] = if avg_word_len > 7.0 {
        0.6
    } else if avg_word_len > 5.5 {
        0.2
    } else {
        0.0
    };

    // 12. Conversation depth
    signals[11] = match msg_count {
        0..=2 => 0.0,
        3..=5 => 0.3,
        6..=10 => 0.6,
        _ => 1.0,
    };

    // 13. Tool usage
    signals[12] = if has_tools { 0.8 } else { 0.0 };

    // 14. Output format complexity
    signals[13] = score_keyword_density(&text, &["json", "csv", "xml", "markdown", "structured"]);

    // 15. Domain specificity
    signals[14] = score_keyword_density(
        &text,
        &[
            "medical",
            "legal",
            "scientific",
            "clinical",
            "regulatory",
            "compliance",
            "diagnosis",
        ],
    );

    // Weighted sum
    let score: f64 = signals.iter().zip(WEIGHTS.iter()).map(|(s, w)| s * w).sum();

    let tier = Tier::from_score(score);

    debug!(
        score = format!("{score:.4}"),
        tier = ?tier,
        word_count,
        msg_count,
        "request classified"
    );

    ScorerResult {
        score,
        tier,
        signals,
    }
}

/// Result of the 15-dimension scorer.
#[derive(Debug, Clone)]
pub struct ScorerResult {
    pub score: f64,
    pub tier: Tier,
    pub signals: [f64; 15],
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Concatenate all user message content into a single lowercase string.
fn concatenate_user_content(messages: &[rcr_common::types::ChatMessage]) -> String {
    messages
        .iter()
        .filter(|m| m.role == rcr_common::types::Role::User)
        .map(|m| m.content.as_str())
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Score keyword density: returns 0.0-1.0 based on how many keywords are found.
fn score_keyword_density(text: &str, keywords: &[&str]) -> f64 {
    let matches = keywords.iter().filter(|k| text.contains(**k)).count();
    match matches {
        0 => 0.0,
        1 => 0.3,
        2 => 0.6,
        _ => 1.0,
    }
}

/// Score code presence: backticks, common keywords, indentation patterns.
fn score_code_presence(text: &str) -> f64 {
    let mut score = 0.0;
    if text.contains("```") || text.contains('`') {
        score += 0.4;
    }
    let code_keywords = [
        "function", "class", "def ", "fn ", "impl ", "struct ", "const ", "let ", "var ", "import",
        "return", "async", "await",
    ];
    let matches = code_keywords.iter().filter(|k| text.contains(**k)).count();
    score += (matches as f64 * 0.15).min(0.6);
    score.min(1.0)
}

/// Score math/logic presence: equations, formal notation.
fn score_math_presence(text: &str) -> f64 {
    let mut score = 0.0;
    let math_indicators = ["=", "+", "-", "*", "/", "∑", "∫", "∀", "∃", "≥", "≤"];
    let matches = math_indicators
        .iter()
        .filter(|k| text.contains(**k))
        .count();
    score += (matches as f64 * 0.15).min(0.5);
    if text.contains("equation") || text.contains("formula") || text.contains("calculate") {
        score += 0.4;
    }
    score.min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcr_common::types::{ChatMessage, Role};

    fn user_msg(content: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: content.to_string(),
            name: None,
            tool_calls: None,
            tool_call_id: None,
        }
    }

    #[test]
    fn test_simple_greeting() {
        let messages = vec![user_msg("Hello!")];
        let result = classify(&messages, false);
        assert_eq!(result.tier, Tier::Simple);
    }

    #[test]
    fn test_code_request() {
        let messages = vec![user_msg(
            "Write a function that implements a distributed consensus algorithm with async/await",
        )];
        let result = classify(&messages, false);
        assert!(
            result.tier == Tier::Complex || result.tier == Tier::Medium,
            "got {:?}",
            result.tier
        );
    }

    #[test]
    fn test_reasoning_request() {
        let messages = vec![user_msg(
            "Prove step by step that the algorithm is correct. Analyze the time complexity and evaluate whether it's optimal. \
             Compare and contrast with alternative approaches, then explain why the chosen algorithm is better. \
             Think through edge cases and reason about correctness guarantees.",
        )];
        let result = classify(&messages, false);
        assert!(
            result.tier == Tier::Reasoning || result.tier == Tier::Complex,
            "got {:?} with score {:.4}",
            result.tier,
            result.score
        );
    }

    #[test]
    fn test_tool_usage_boosts_score() {
        let messages = vec![user_msg("Search the web for recent news")];
        let without_tools = classify(&messages, false);
        let with_tools = classify(&messages, true);
        assert!(with_tools.score > without_tools.score);
    }

    #[test]
    fn test_weights_sum_to_one() {
        let sum: f64 = WEIGHTS.iter().sum();
        assert!(
            (sum - 1.0).abs() < 1e-10,
            "weights sum to {sum}, expected 1.0"
        );
    }
}
