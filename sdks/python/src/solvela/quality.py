"""Degraded response detection with 4 heuristics."""

from __future__ import annotations

from enum import Enum


class DegradedReason(str, Enum):
    """Reason a response is considered degraded."""

    EMPTY_CONTENT = "empty_content"
    KNOWN_ERROR_PHRASE = "known_error_phrase"
    REPETITIVE_LOOP = "repetitive_loop"
    TRUNCATED_MID_WORD = "truncated_mid_word"


# Phrases that strongly suggest a safety-refusal non-answer from a gated
# model. The list is intentionally narrow — broader matches like "i cannot"
# would flag legitimate prose ("I cannot stress this enough...") and trigger
# paid retries on perfectly good responses. Treat additions cautiously.
_ERROR_PHRASES = [
    "i cannot",
    "as an ai",
    "i'm sorry, but i",
]

# Mid-word truncation heuristic threshold. The earlier rule (len > 100 and
# last char alphanumeric) misfired on any normal response ending in a digit
# ("...launched in 2024") or a proper-noun capital ("...invented by GPT"),
# triggering a paid retry on a perfectly good completion. The current rule
# requires the last character to be a lowercase letter — the dominant
# signature of a max_tokens cut — and pushes the length floor up so
# accidental hits on short answers are implausible.
_TRUNCATION_MIN_LEN = 250


def check_degraded(content: str) -> DegradedReason | None:
    """Check if response content is degraded. Returns reason or None if OK."""
    # 1. Empty/whitespace
    if not content or not content.strip():
        return DegradedReason.EMPTY_CONTENT

    # 2. Known error phrases (case-insensitive)
    lower = content.lower()
    for phrase in _ERROR_PHRASES:
        if phrase in lower:
            return DegradedReason.KNOWN_ERROR_PHRASE

    # 3. Repetitive 3-word phrases (any trigram appears 5+ times)
    words = content.split()
    if len(words) >= 15:
        trigram_counts: dict[str, int] = {}
        for i in range(len(words) - 2):
            trigram = f"{words[i]} {words[i + 1]} {words[i + 2]}"
            trigram_counts[trigram] = trigram_counts.get(trigram, 0) + 1
            if trigram_counts[trigram] >= 5:
                return DegradedReason.REPETITIVE_LOOP

    # 4. Truncated mid-word: long response ending with a lowercase letter
    # (strong signal of a max_tokens cut, e.g. "the result is appro").
    # Digits, capitals, and punctuation no longer trigger.
    if len(content) > _TRUNCATION_MIN_LEN and content[-1].islower():
        return DegradedReason.TRUNCATED_MID_WORD

    return None
