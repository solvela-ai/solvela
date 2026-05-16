use std::collections::HashMap;

use solvela_protocol::ChatResponse;

/// Reason a response is considered degraded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DegradedReason {
    EmptyContent,
    RepetitiveLoop,
    TruncatedMidWord,
    KnownErrorPhrase,
}

const KNOWN_ERROR_PHRASES: &[&str] = &["i cannot", "as an ai", "i'm sorry, but i"];
const REPETITION_WINDOW: usize = 3;
const REPETITION_THRESHOLD: usize = 5;
const TRUNCATION_MIN_LEN: usize = 100;

/// Inspect a `ChatResponse` and return a reason if it appears degraded.
///
/// Detection rules are checked in order; the first match is returned.
pub(crate) fn is_degraded(response: &ChatResponse) -> Option<DegradedReason> {
    let content = aggregate_content(response);

    // 1. Empty content
    if content.trim().is_empty() {
        return Some(DegradedReason::EmptyContent);
    }

    // 2. Known error phrases (case-insensitive)
    let lower = content.to_lowercase();
    for phrase in KNOWN_ERROR_PHRASES {
        if lower.contains(phrase) {
            return Some(DegradedReason::KnownErrorPhrase);
        }
    }

    // 3. Repetitive loop: any 3-word phrase repeated 5+ times
    if has_repetitive_loop(&content) {
        return Some(DegradedReason::RepetitiveLoop);
    }

    // 4. Truncated mid-word: >100 chars and ends with alphanumeric
    if content.len() > TRUNCATION_MIN_LEN {
        if let Some(last_char) = content.chars().last() {
            if last_char.is_alphanumeric() {
                return Some(DegradedReason::TruncatedMidWord);
            }
        }
    }

    None
}

/// Concatenate all choice contents into a single string.
fn aggregate_content(response: &ChatResponse) -> String {
    response
        .choices
        .iter()
        .map(|c| c.message.content.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Check if any 3-word window repeats 5+ times.
fn has_repetitive_loop(content: &str) -> bool {
    let words: Vec<&str> = content.split_whitespace().collect();
    if words.len() < REPETITION_WINDOW {
        return false;
    }

    let mut counts: HashMap<String, usize> = HashMap::new();
    for window in words.windows(REPETITION_WINDOW) {
        let key = window.join(" ");
        let count = counts.entry(key).or_insert(0);
        *count += 1;
        if *count >= REPETITION_THRESHOLD {
            return true;
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use solvela_protocol::{ChatChoice, ChatMessage, ChatResponse, Role, Usage};

    use super::*;

    fn make_response(content: &str) -> ChatResponse {
        ChatResponse {
            id: "test-id".to_string(),
            object: "chat.completion".to_string(),
            created: 0,
            model: "test-model".to_string(),
            choices: vec![ChatChoice {
                index: 0,
                message: ChatMessage {
                    role: Role::Assistant,
                    content: content.to_string(),
                    name: None,
                    tool_calls: None,
                    tool_call_id: None,
                },
                finish_reason: Some("stop".to_string()),
            }],
            usage: Some(Usage {
                prompt_tokens: 10,
                completion_tokens: 20,
                total_tokens: 30,
            }),
        }
    }

    #[test]
    fn normal_response_returns_none() {
        let resp = make_response("Hello! How can I help you today?");
        assert_eq!(is_degraded(&resp), None);
    }

    #[test]
    fn empty_content_detected() {
        let resp = make_response("");
        assert_eq!(is_degraded(&resp), Some(DegradedReason::EmptyContent));
    }

    #[test]
    fn whitespace_only_content_detected_as_empty() {
        let resp = make_response("   \n\t  ");
        assert_eq!(is_degraded(&resp), Some(DegradedReason::EmptyContent));
    }

    #[test]
    fn known_error_phrase_detected_case_insensitive() {
        let resp = make_response("Well, AS AN AI language model, I think...");
        assert_eq!(is_degraded(&resp), Some(DegradedReason::KnownErrorPhrase));

        let resp2 = make_response("I'm sorry, but I cannot do that.");
        assert_eq!(is_degraded(&resp2), Some(DegradedReason::KnownErrorPhrase));
    }

    #[test]
    fn repetitive_loop_detected() {
        // "the quick brown" repeated 6 times (well over 5 threshold)
        let repeated = "the quick brown ".repeat(6);
        let content = format!("Start of response. {repeated}End.");
        let resp = make_response(&content);
        assert_eq!(is_degraded(&resp), Some(DegradedReason::RepetitiveLoop));
    }

    #[test]
    fn truncated_mid_word_detected() {
        // >100 chars ending in alphanumeric, no repetitive patterns
        let content = "The quick brown fox jumps over the lazy dog near the river bank and the tall oak tree beside the old stone wall with gre";
        assert!(content.len() > TRUNCATION_MIN_LEN);
        let resp = make_response(content);
        assert_eq!(is_degraded(&resp), Some(DegradedReason::TruncatedMidWord));
    }

    #[test]
    fn short_content_ending_alphanumeric_not_truncated() {
        let resp = make_response("short text ending abrupt");
        assert!(resp.choices[0].message.content.len() <= TRUNCATION_MIN_LEN);
        assert_eq!(is_degraded(&resp), None);
    }
}
