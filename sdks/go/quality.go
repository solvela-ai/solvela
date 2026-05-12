package solvela

import "strings"

// DegradedReason describes why a response was considered degraded.
type DegradedReason string

const (
	DegradedEmptyContent     DegradedReason = "empty_content"
	DegradedKnownErrorPhrase DegradedReason = "known_error_phrase"
	DegradedRepetitiveLoop   DegradedReason = "repetitive_loop"
	DegradedTruncatedMidWord DegradedReason = "truncated_mid_word"
)

var knownErrorPhrases = []string{
	"i cannot",
	"as an ai",
	"i'm sorry, but i",
}

// CheckDegraded checks if response content is degraded.
// Returns empty [DegradedReason] if OK, or a specific DegradedReason value.
func CheckDegraded(content string) DegradedReason {
	// 1. Empty/whitespace
	if strings.TrimSpace(content) == "" {
		return DegradedEmptyContent
	}

	// 2. Known error phrases (case-insensitive)
	lower := strings.ToLower(content)
	for _, phrase := range knownErrorPhrases {
		if strings.Contains(lower, phrase) {
			return DegradedKnownErrorPhrase
		}
	}

	// 3. Repetitive 3-word phrases (any trigram appears 5+ times)
	words := strings.Fields(content)
	if len(words) >= 15 {
		counts := make(map[string]int)
		for i := 0; i <= len(words)-3; i++ {
			trigram := words[i] + " " + words[i+1] + " " + words[i+2]
			counts[trigram]++
			if counts[trigram] >= 5 {
				return DegradedRepetitiveLoop
			}
		}
	}

	// 4. Truncated mid-word.
	//
	// A response is treated as truncated mid-word only when all three of these
	// hold:
	//   - the (right-trimmed) content is longer than 100 chars,
	//   - the final non-space character is not a closing-style character
	//     (sentence punctuation, closing quote, closing bracket, backtick,
	//     or newline) that would naturally end well-formed prose, AND
	//   - the final whitespace-delimited token is at least 4 chars long.
	//
	// Trailing whitespace alone is not a truncation signal — well-formed
	// streamed completions often end with a newline. Short final tokens
	// like "OK", "Hi", or "1" are not truncation signals either; they are
	// common legitimate endings and would false-positive almost every
	// short reply otherwise. The previous heuristic (>100 chars && last
	// char is alphanumeric) flagged virtually any prose ending in a
	// letter, including correctly punctuated sentences whose closing
	// quote was followed by no punctuation.
	trimmed := strings.TrimRight(content, " \t\r\n")
	if len(trimmed) > 100 {
		last := rune(trimmed[len(trimmed)-1])
		const closers = ".!?\"'`)]}"
		if !strings.ContainsRune(closers, last) {
			lastSpace := strings.LastIndexAny(trimmed, " \t\n")
			lastToken := trimmed[lastSpace+1:]
			if len(lastToken) >= 4 {
				return DegradedTruncatedMidWord
			}
		}
	}

	return ""
}
