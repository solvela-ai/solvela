"""Tests for quality check — degraded response detection."""

from __future__ import annotations

from solvela.quality import DegradedReason, check_degraded


class TestEmptyContent:
    def test_empty_content_is_degraded(self) -> None:
        assert check_degraded("") == DegradedReason.EMPTY_CONTENT

    def test_whitespace_only_is_degraded(self) -> None:
        assert check_degraded("   \n\t  ") == DegradedReason.EMPTY_CONTENT


class TestKnownErrorPhrases:
    def test_known_error_phrase(self) -> None:
        assert (
            check_degraded("I cannot help with that request.") == DegradedReason.KNOWN_ERROR_PHRASE
        )

    def test_case_insensitive_error_phrase(self) -> None:
        assert (
            check_degraded("As An AI language model, I think this is fine.")
            == DegradedReason.KNOWN_ERROR_PHRASE
        )

    def test_im_sorry_phrase(self) -> None:
        assert (
            check_degraded("I'm sorry, but I can't do that.") == DegradedReason.KNOWN_ERROR_PHRASE
        )


class TestRepetitiveLoop:
    def test_repetitive_loop(self) -> None:
        # A 3-word phrase repeated 20 times (60 words total, well above 15)
        content = " ".join(["the quick brown"] * 20)
        assert check_degraded(content) == DegradedReason.REPETITIVE_LOOP


class TestTruncatedMidWord:
    def test_truncated_mid_word(self) -> None:
        # >250 chars ending with a lowercase letter — the canonical
        # max_tokens-cut signature.
        content = "This is a long response that contains many words and goes on " + "a" * 200 + "b"
        assert len(content) > 250
        assert check_degraded(content) == DegradedReason.TRUNCATED_MID_WORD

    def test_content_ending_with_period_not_truncated(self) -> None:
        # Long response ending in punctuation — not truncated. Padding must
        # avoid trigram repetition or the loop heuristic fires first.
        content = (
            "The quick brown fox jumps over the lazy dog and continues "
            "down a winding path through forest and field. Birds call from "
            "high branches while leaves rustle softly in the afternoon "
            "breeze. Eventually the traveler reaches a quiet clearing where "
            "an old wooden bench sits beneath a single tall oak whose "
            "branches stretch far overhead in welcome silence."
        )
        assert len(content) > 250
        assert check_degraded(content) is None

    def test_content_ending_with_digit_is_not_truncated(self) -> None:
        # Regression: "...launched in 2024" used to fire false-positive when
        # the rule was `len > 100 and content[-1].isalnum()`.
        content = (
            "The first commercial release shipped two decades ago and the "
            "platform has evolved through several major architectural shifts "
            "since then. Compatibility breaks were handled in waves rather "
            "than as a single migration, with the most recent generation "
            "released in 2024"
        )
        assert len(content) > 250
        assert check_degraded(content) is None

    def test_content_ending_with_capital_letter_is_not_truncated(self) -> None:
        # Regression: "...invented by GPT" should not fire either.
        content = (
            "Modern transformer architectures power most of the current "
            "generation of conversational systems and have replaced earlier "
            "recurrent approaches in nearly every benchmark, even when the "
            "training corpora are different. The acronym for this family of "
            "models is GPT"
        )
        assert len(content) > 250
        assert content[-1].isupper()
        assert check_degraded(content) is None

    def test_short_response_ending_in_lowercase_not_truncated(self) -> None:
        # A response under the 250-char floor cannot be considered truncated;
        # the heuristic only fires on long responses where max_tokens is the
        # most plausible cause of a mid-word ending.
        content = "Short answer ending mid-word in appro"
        assert len(content) < 250
        assert content[-1].islower()
        assert check_degraded(content) is None


class TestNormalContent:
    def test_normal_content_not_degraded(self) -> None:
        assert check_degraded("The capital of France is Paris.") is None
