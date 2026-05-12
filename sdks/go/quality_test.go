package solvela

import (
	"strings"
	"testing"
)

func TestCheckDegradedEmptyContent(t *testing.T) {
	reason := CheckDegraded("")
	if reason != DegradedEmptyContent {
		t.Errorf("got %q, want %q", reason, DegradedEmptyContent)
	}
}

func TestCheckDegradedWhitespaceOnly(t *testing.T) {
	reason := CheckDegraded("   \t\n  ")
	if reason != DegradedEmptyContent {
		t.Errorf("got %q, want %q", reason, DegradedEmptyContent)
	}
}

func TestCheckDegradedKnownErrorPhrase(t *testing.T) {
	reason := CheckDegraded("I cannot help with that request.")
	if reason != DegradedKnownErrorPhrase {
		t.Errorf("got %q, want %q", reason, DegradedKnownErrorPhrase)
	}
}

func TestCheckDegradedKnownErrorPhraseCaseInsensitive(t *testing.T) {
	reason := CheckDegraded("AS AN AI language model, I can help.")
	if reason != DegradedKnownErrorPhrase {
		t.Errorf("got %q, want %q", reason, DegradedKnownErrorPhrase)
	}
}

func TestCheckDegradedRepetitiveLoop(t *testing.T) {
	// Build content with a trigram repeated 5+ times
	repeated := strings.Repeat("the quick brown fox jumps ", 6)
	reason := CheckDegraded(repeated)
	if reason != DegradedRepetitiveLoop {
		t.Errorf("got %q, want %q", reason, DegradedRepetitiveLoop)
	}
}

func TestCheckDegradedTruncatedMidWord(t *testing.T) {
	// >100 chars ending with a long unfinished token: "comput" (6 chars,
	// no trailing punctuation, not a closer).
	content := "The system processes input data and continues to handle each request through the long pipeline as it comput"
	if len(content) <= 100 {
		t.Fatalf("test content too short: %d chars", len(content))
	}
	reason := CheckDegraded(content)
	if reason != DegradedTruncatedMidWord {
		t.Errorf("got %q, want %q", reason, DegradedTruncatedMidWord)
	}
}

// TestCheckDegradedClosingChars walks the closer set: prose ending in a
// closer should never flag as truncated, even when it would have under the
// old "ends-with-alphanumeric" heuristic.
func TestCheckDegradedClosingChars(t *testing.T) {
	base := "The quick brown fox jumps over the lazy dog and then runs through the forest while the birds sing softly all evening long"
	if len(base) <= 100 {
		t.Fatalf("test base too short: %d chars", len(base))
	}
	cases := []struct {
		name string
		end  string
	}{
		{"period", "."},
		{"exclamation", "!"},
		{"question", "?"},
		{"double_quote", `."`},
		{"close_paren", ")"},
		{"close_bracket", "]"},
		{"close_brace", "}"},
		{"backtick", "`"},
		{"single_quote", "'"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			content := base + tc.end
			if len(content) <= 100 {
				t.Fatalf("test content too short: %d chars", len(content))
			}
			reason := CheckDegraded(content)
			if reason != "" {
				t.Errorf("ending %q: got %q, want not degraded", tc.end, reason)
			}
		})
	}
}

// TestCheckDegradedShortFinalToken — a short final token like "OK" is a
// common legitimate ending and must NOT flag as truncated, even when the
// total content exceeds 100 chars.
func TestCheckDegradedShortFinalToken(t *testing.T) {
	// Total >100 chars but the last whitespace-delimited token is "OK"
	// (2 chars) — under the 4-char threshold for mid-word truncation.
	content := "The system processes the input and produces a result, and the current status of every queued job is OK"
	if len(content) <= 100 {
		t.Fatalf("test content too short: %d chars", len(content))
	}
	reason := CheckDegraded(content)
	if reason != "" {
		t.Errorf("got %q, want empty (short final token is not truncation)", reason)
	}
}

// TestCheckDegradedTrailingWhitespace — well-formed completions that end in
// trailing whitespace (often a newline from the model) must not be
// flagged as truncated. The trailing-whitespace trim happens before the
// closer check.
func TestCheckDegradedTrailingWhitespace(t *testing.T) {
	content := "The quick brown fox jumps over the lazy dog and then runs through the forest while the birds sing softly.\n"
	if len(content) <= 100 {
		t.Fatalf("test content too short: %d chars", len(content))
	}
	reason := CheckDegraded(content)
	if reason != "" {
		t.Errorf("got %q, want empty (trailing whitespace is not truncation)", reason)
	}
}

// TestCheckDegradedTrailingWhitespaceLongTokenStillTruncated — trailing
// whitespace alone is benign, but a stream ending mid-long-token followed
// by whitespace is still truncated. The right-trim must reveal the
// underlying mid-word boundary.
func TestCheckDegradedTrailingWhitespaceLongTokenStillTruncated(t *testing.T) {
	content := "The system processes input data and continues to handle each request through the pipeline as it comput   "
	reason := CheckDegraded(content)
	if reason != DegradedTruncatedMidWord {
		t.Errorf("got %q, want %q (trim should expose mid-word boundary)", reason, DegradedTruncatedMidWord)
	}
}

func TestCheckDegradedNormalContent(t *testing.T) {
	reason := CheckDegraded("This is a perfectly normal response.")
	if reason != "" {
		t.Errorf("got %q, want empty (not degraded)", reason)
	}
}

func TestCheckDegradedContentEndingWithPeriod(t *testing.T) {
	// >100 chars ending with period should NOT be truncated
	// Use diverse words to avoid triggering repetitive loop detection
	content := "The quick brown fox jumps over the lazy dog and then runs through the forest while the birds sing their beautiful songs in the morning light."
	if len(content) <= 100 {
		t.Fatalf("test content too short: %d chars", len(content))
	}
	reason := CheckDegraded(content)
	if reason != "" {
		t.Errorf("got %q, want empty (period ending is not truncated)", reason)
	}
}

func TestCheckDegradedShortContentEndingAlphanumeric(t *testing.T) {
	// <=100 chars ending with alphanumeric should NOT be truncated
	reason := CheckDegraded("Short text ending with word")
	if reason != "" {
		t.Errorf("got %q, want empty (short content not checked for truncation)", reason)
	}
}
