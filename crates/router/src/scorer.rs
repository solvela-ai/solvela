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

// ---------------------------------------------------------------------------
// Keyword tables
//
// Every keyword is stored PRE-NORMALIZED and PRE-PADDED (" prove ", " step by
// step "), so matching is a bare `normalized.contains(kw)` — no per-call
// `format!`, no per-keyword allocation. `padded_keywords_match_the_normalizer`
// asserts each const is byte-identical to `normalize_for_keywords()` of its
// unpadded form, so the two representations cannot drift.
//
// These are the WORD-BOUNDARY tables only. Code presence, math/logic and
// numeric list markers read RAW text and are not listed here.
// ---------------------------------------------------------------------------

/// 3. Reasoning markers.
const REASONING_MARKERS: &[&str] = &[
    " prove ",
    " theorem ",
    " step by step ",
    " reason ",
    " analyze ",
    " evaluate ",
    " compare and contrast ",
    " think through ",
    " explain why ",
];

/// 4. Technical terms.
const TECHNICAL_TERMS: &[&str] = &[
    " algorithm ",
    " kubernetes ",
    " database ",
    " architecture ",
    " distributed ",
    " concurrent ",
    " protocol ",
    " optimization ",
    " benchmark ",
];

/// 5. Creative markers.
const CREATIVE_MARKERS: &[&str] = &[
    " story ",
    " poem ",
    " brainstorm ",
    " creative ",
    " imagine ",
    " fiction ",
    " narrative ",
];

/// 6. Simple indicators (negative signal).
const SIMPLE_INDICATORS: &[&str] = &[
    " what is ",
    " define ",
    " translate ",
    " hello ",
    " hi ",
    " thanks ",
    " yes ",
    " no ",
];

/// 7. Multi-step word markers.
///
/// The NUMERIC list markers live in [`count_numeric_list_markers`] and read
/// raw text.
const MULTI_STEP_MARKERS: &[&str] = &[
    " first ",
    " then ",
    " next ",
    " finally ",
    " step 1 ",
    " step 2 ",
];

/// 9. Agentic task markers.
const AGENTIC_MARKERS: &[&str] = &[
    " read file ",
    " write file ",
    " edit ",
    " deploy ",
    " execute ",
    " run command ",
    " install ",
];

/// 14. Output format complexity.
const OUTPUT_FORMAT_MARKERS: &[&str] = &[" json ", " csv ", " xml ", " markdown ", " structured "];

/// 15. Domain specificity.
const DOMAIN_MARKERS: &[&str] = &[
    " medical ",
    " legal ",
    " scientific ",
    " clinical ",
    " regulatory ",
    " compliance ",
    " diagnosis ",
];

/// Every word-boundary table, for the drift guard in tests.
#[cfg(test)]
const ALL_KEYWORD_TABLES: &[&[&str]] = &[
    REASONING_MARKERS,
    TECHNICAL_TERMS,
    CREATIVE_MARKERS,
    SIMPLE_INDICATORS,
    MULTI_STEP_MARKERS,
    AGENTIC_MARKERS,
    OUTPUT_FORMAT_MARKERS,
    DOMAIN_MARKERS,
];

/// Score a request across 15 dimensions and return a complexity tier.
///
/// Purely rule-based, zero external calls, one heap allocation beyond the
/// concatenated prompt (the normalized copy used for word-boundary matching).
///
/// MEASURED 2026-07-08 — release build, 100k warmed iterations, keyword-dense
/// ~430-char prompt, one dev workstation: **~7.7 us per classification**. The
/// "<1 microsecond" claim this doc used to carry was never benchmarked and was
/// never true: the same harness puts the pre-2026-07-08 scorer at ~5.1 us.
/// Re-measure before quoting a number here; do not restore an unmeasured claim.
pub fn classify(messages: &[solvela_protocol::ChatMessage], has_tools: bool) -> ScorerResult {
    let text = concatenate_user_content(messages);
    // Word-boundary view of the same text, built once. Used ONLY by the
    // keyword-density dimensions (3, 4, 5, 6, 7, 9, 14, 15). Dimensions 2
    // (code presence) and 10 (math/logic) read punctuation — backticks,
    // `"def "`, `=`, `+` — and must keep the RAW text.
    let normalized = normalize_for_keywords(&text);
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
    signals[2] = score_keyword_density(&normalized, REASONING_MARKERS);

    // 4. Technical terms
    signals[3] = score_keyword_density(&normalized, TECHNICAL_TERMS);

    // 5. Creative markers
    signals[4] = score_keyword_density(&normalized, CREATIVE_MARKERS);

    // 6. Simple indicators (negative signal — pushes score down)
    signals[5] = -score_keyword_density(&normalized, SIMPLE_INDICATORS);

    // 7. Multi-step patterns — word markers on the normalized text, numeric list
    //    markers on the RAW text (they are punctuation, like code and math).
    signals[6] = density_score(
        count_keywords(&normalized, MULTI_STEP_MARKERS) + count_numeric_list_markers(&text),
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
    signals[8] = score_keyword_density(&normalized, AGENTIC_MARKERS);

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
    signals[13] = score_keyword_density(&normalized, OUTPUT_FORMAT_MARKERS);

    // 15. Domain specificity
    signals[14] = score_keyword_density(&normalized, DOMAIN_MARKERS);

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
fn concatenate_user_content(messages: &[solvela_protocol::ChatMessage]) -> String {
    messages
        .iter()
        .filter(|m| m.role == solvela_protocol::Role::User)
        .map(|m| m.content.as_text())
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Normalize text for word-boundary keyword matching.
///
/// Lowercases, replaces every non-alphanumeric char with a space, collapses
/// runs of spaces, and pads with a leading and trailing space. Keywords are
/// stored already in this form (`" foo bar "`), so matching is a bare
/// `normalized.contains(kw)` that can only hit on true word/phrase boundaries.
///
/// This is what stops `"hi"` firing inside "within", `"no"` inside "know",
/// `"edit"` inside "credit", and `"class"` inside "classification"; it also
/// makes hyphenated phrasings ("step-by-step") match their spaced keywords
/// ("step by step") for free.
///
/// NOT for the code-presence or math/logic dimensions: both read punctuation
/// this deliberately destroys.
fn normalize_for_keywords(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push(' ');
    let mut last_was_space = true;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_was_space = false;
        } else if ch.is_alphanumeric() {
            out.extend(ch.to_lowercase());
            last_was_space = false;
        } else if !last_was_space {
            out.push(' ');
            last_was_space = true;
        }
    }
    if !last_was_space {
        out.push(' ');
    }
    out
}

/// Count numeric list markers in RAW text: a whitespace-delimited token that is
/// one-or-more ASCII digits followed by exactly one `.` or `)` — `"1."`, `"2)"`,
/// `"10."`.
///
/// This is punctuation-dependent, exactly like code presence and math symbols,
/// so it MUST read the raw text. On the normalized text `"[[2,1],[1,2]]"`
/// collapses to bare `" 2 1 1 2 "` and every stray digit becomes a "list
/// marker"; `"127.0.0.1"`, `"gpt-4.1"` and `"3.14"` do the same. Requiring the
/// terminator AND a token boundary on both sides rejects all of those: a
/// `split_whitespace` token is bounded by whitespace or the string ends, which
/// is precisely the "followed by whitespace or end-of-string" condition.
fn count_numeric_list_markers(text: &str) -> usize {
    text.split_whitespace()
        .filter(|token| {
            let digits = match token.strip_suffix('.') {
                Some(d) => d,
                None => match token.strip_suffix(')') {
                    Some(d) => d,
                    None => return false,
                },
            };
            !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
        })
        .count()
}

/// Map a raw match count onto the shared 0.0-1.0 density scale.
fn density_score(matches: usize) -> f64 {
    match matches {
        0 => 0.0,
        1 => 0.3,
        2 => 0.6,
        _ => 1.0,
    }
}

/// Count keywords present in `normalized`.
///
/// `normalized` MUST come from [`normalize_for_keywords`], and `keywords` MUST
/// be pre-padded (`" prove "`) — the padding IS the word-boundary check. Both
/// sides are already normalized, so this allocates nothing.
fn count_keywords(normalized: &str, keywords: &[&str]) -> usize {
    keywords.iter().filter(|k| normalized.contains(**k)).count()
}

/// Score keyword density: returns 0.0-1.0 based on how many keywords are found.
fn score_keyword_density(normalized: &str, keywords: &[&str]) -> f64 {
    density_score(count_keywords(normalized, keywords))
}

/// Score code presence: backticks, common keywords, indentation patterns.
fn score_code_presence(text: &str) -> f64 {
    let mut score = 0.0;
    if text.contains("```") || text.contains('`') {
        score += 0.4;
    }
    // `"let "` still matches the English "let me", and `"const "`/`"var "` are
    // matched against RAW text (trailing space), so they are not word-bounded.
    // MEASURED 2026-07-08: dropping `let`/`const`/`var` costs accuracy —
    // 56.16% -> 54.79% alone, 53.42% on top of dropping `-`/`/` from
    // `score_math_presence`. Operator decision: leave as-is until Tier 2.
    let code_keywords = [
        "function", "class", "def ", "fn ", "impl ", "struct ", "const ", "let ", "var ", "import",
        "return", "async", "await",
    ];
    let matches = code_keywords.iter().filter(|k| text.contains(**k)).count();
    score += (matches as f64 * 0.15).min(0.6);
    score.min(1.0)
}

/// Score math/logic presence: equations, formal notation.
///
/// MEASURED 2026-07-08, do not "clean up" without re-measuring: `-`, `/` and `*`
/// are not mathematics — they fire on "state-of-the-art", "and/or", "sub-100-
/// millisecond", dates, file paths, and markdown bullets. Removing them
/// nonetheless makes the scorer AGREE LESS with the golden set
/// (`tests/golden_set.rs`), because in long analytical prose this dimension is
/// currently acting as a proxy for "punctuation-rich technical writing" and is
/// propping those prompts over the 0.2 Complex threshold:
///
///   drop `/` alone      56.16% -> 56.16%
///   drop `-` alone      56.16% -> 56.16%
///   drop `*` alone      56.16% -> 56.16%
///   drop `-` and `/`    56.16% -> 54.79%  (the multiplayer/UDP row falls
///                                          Complex -> Medium, and Complex is
///                                          its `intended` value)
///   drop `-`, `/`, `*`  56.16% -> 54.79%
///
/// The real defect is under-scoring of long analytical prompts (weights /
/// `Tier::from_score` thresholds), which is deliberately Tier-2 work. Fix that
/// first, then delete these symbols. Removing the spurious numeric-list-marker
/// signal (see [`count_numeric_list_markers`]) cost accuracy for the same
/// reason and on overlapping rows — two independent pieces of evidence that the
/// 0.2 Complex cutoff is too high, not that the signals were right.
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
    use solvela_protocol::{ChatMessage, Role};

    fn user_msg(content: &str) -> ChatMessage {
        ChatMessage {
            role: Role::User,
            content: content.into(),
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
    fn normalize_collapses_punctuation_and_pads_boundaries() {
        assert_eq!(
            normalize_for_keywords("Step-by-step: DO it!"),
            " step by step do it "
        );
        assert_eq!(normalize_for_keywords(""), " ");
    }

    /// Keyword-density dimensions must match whole words/phrases, never bare
    /// substrings. Before this, the NEGATIVE simple-indicator "hi" fired inside
    /// "within" and "no" inside "know", systematically dragging ordinary
    /// prompts toward Simple; "edit" fired in "credit".
    #[test]
    fn keyword_density_matches_on_word_boundaries_only() {
        let haystack = normalize_for_keywords("Within, you know your credit score.");
        assert_eq!(score_keyword_density(&haystack, &[" hi ", " no "]), 0.0);
        assert_eq!(score_keyword_density(&haystack, &[" edit "]), 0.0);

        // ...but the real words still match.
        let real = normalize_for_keywords("Hi! No thanks.");
        assert_eq!(score_keyword_density(&real, &[" hi ", " no "]), 0.6);
    }

    /// FIX 4: hyphenated phrasing matches the spaced multi-word keyword.
    #[test]
    fn hyphenated_phrases_match_spaced_keywords() {
        let haystack = normalize_for_keywords("Walk me through this step-by-step: how?");
        assert_eq!(score_keyword_density(&haystack, &[" step by step "]), 0.3);
    }

    /// The keyword tables are stored pre-normalized and pre-padded so matching
    /// allocates nothing. If they ever drift from what `normalize_for_keywords`
    /// produces, the affected dimension silently stops matching. Pin both.
    #[test]
    fn padded_keywords_match_the_normalizer() {
        for table in ALL_KEYWORD_TABLES {
            for kw in *table {
                assert!(
                    kw.starts_with(' ') && kw.ends_with(' '),
                    "keyword {kw:?} is not padded"
                );
                assert_eq!(
                    *kw,
                    normalize_for_keywords(kw.trim()),
                    "keyword {kw:?} is not what the normalizer produces"
                );
            }
        }
    }

    /// Numeric list markers are punctuation, so they are detected on the RAW
    /// text. A digit is only a list marker when it is a whole whitespace-
    /// delimited token ending in exactly one `.` or `)`.
    #[test]
    fn numeric_list_markers_need_a_terminator_and_token_boundaries() {
        assert_eq!(
            count_numeric_list_markers("1. Read the CSV file. 2. Parse the rows."),
            2
        );
        assert_eq!(count_numeric_list_markers("1) alpha 2) beta 10. gamma"), 3);
        // Trailing marker at end-of-string still counts.
        assert_eq!(count_numeric_list_markers("do this 1."), 1);

        // ...and none of these are list markers.
        for text in [
            "Find the eigenvalues of the matrix [[2,1],[1,2]].",
            "Getting ECONNREFUSED 127.0.0.1:5432 from my Node app.",
            "Use gpt-4.1 for this.",
            "pi is roughly 3.14",
            "Solve for x: 2x + 5 = 13",
            "What is 17 * 24?",
            "1.Read the file",
        ] {
            assert_eq!(
                count_numeric_list_markers(text),
                0,
                "false positive: {text}"
            );
        }
    }

    /// Dimensions 2 (code presence) and 10 (math/logic) read punctuation the
    /// normalizer destroys, so `classify()` must hand them the RAW text.
    ///
    /// This asserts through `classify()` on the returned `signals` array, not on
    /// the helpers in isolation: the failure mode being guarded is a WIRING
    /// mistake (`signals[1] = score_code_presence(&normalized)`), and a helper-
    /// level assertion cannot see it. Verified by mutating both call sites to
    /// `&normalized` and watching this test fail.
    #[test]
    fn classify_feeds_code_and_math_dimensions_the_raw_text() {
        let prompt = "```rust\nfn main() { let x = 2 + 2 * 3; }\n```";
        let raw = prompt.to_lowercase();
        let normalized = normalize_for_keywords(&raw);

        // The guard is only meaningful if raw and normalized disagree here.
        assert_ne!(score_code_presence(&raw), score_code_presence(&normalized));
        assert_ne!(score_math_presence(&raw), score_math_presence(&normalized));

        let result = classify(&[user_msg(prompt)], false);
        assert_eq!(
            result.signals[1],
            score_code_presence(&raw),
            "dimension 2 must be scored on the raw text"
        );
        assert_eq!(
            result.signals[9],
            score_math_presence(&raw),
            "dimension 10 must be scored on the raw text"
        );
    }

    /// Helper-level companion to the wiring test above: the normalizer really
    /// does destroy the punctuation these two dimensions depend on.
    #[test]
    fn code_and_math_dimensions_read_raw_punctuation() {
        let code = "```rust\nfn main() {}\n```";
        assert!(score_code_presence(code) > score_code_presence(&normalize_for_keywords(code)));

        let math = "solve x = 2 + 2 * 3";
        assert!(score_math_presence(math) > 0.0);
        assert_eq!(score_math_presence(&normalize_for_keywords(math)), 0.0);
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
