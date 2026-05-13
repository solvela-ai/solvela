export enum DegradedReason {
  EmptyContent = 'empty_content',
  KnownErrorPhrase = 'known_error_phrase',
  RepetitiveLoop = 'repetitive_loop',
  TruncatedMidWord = 'truncated_mid_word',
}

const KNOWN_ERROR_PHRASES = [
  'i cannot assist with',
  "i can't assist with",
  "i can't help with",
  'i cannot help with',
  'as an ai language model',
  'as a large language model',
  "i'm not able to",
  'i am not able to',
  'i apologize, but i cannot',
  "i'm sorry, i can't",
];

const MIN_LENGTH_FOR_TRUNCATION = 20;
const MIN_LENGTH_FOR_REPETITION = 100;
const REPETITION_THRESHOLD = 0.15;

export function checkDegraded(content: string): DegradedReason | null {
  // 1. Empty content
  if (content.trim().length === 0) {
    return DegradedReason.EmptyContent;
  }

  // 2. Known error phrases
  const lower = content.toLowerCase();
  for (const phrase of KNOWN_ERROR_PHRASES) {
    if (lower.includes(phrase)) {
      return DegradedReason.KnownErrorPhrase;
    }
  }

  // 3. Repetitive loop detection
  if (content.length >= MIN_LENGTH_FOR_REPETITION) {
    const words = content.split(/\s+/);
    if (words.length >= 10) {
      // Check for repeating n-grams (window size 3-5)
      for (const n of [3, 4, 5]) {
        const ngrams = new Map<string, number>();
        for (let i = 0; i <= words.length - n; i++) {
          const gram = words.slice(i, i + n).join(' ');
          ngrams.set(gram, (ngrams.get(gram) ?? 0) + 1);
        }
        const totalNgrams = words.length - n + 1;
        const maxCount = Math.max(...ngrams.values());
        if (maxCount > 3 && maxCount / totalNgrams > REPETITION_THRESHOLD) {
          return DegradedReason.RepetitiveLoop;
        }
      }
    }
  }

  // 4. Truncated mid-word
  if (content.length >= MIN_LENGTH_FOR_TRUNCATION) {
    const trimmed = content.trimEnd();
    const lastChar = trimmed[trimmed.length - 1];
    if (lastChar && /[a-zA-Z]/.test(lastChar)) {
      // Check if it ends without punctuation or common ending
      const endsWithPunctuation = /[.!?;:)\]"'`]$/.test(trimmed);
      if (!endsWithPunctuation) {
        // Check if the last "word" looks truncated (not a common short word)
        const lastWord = trimmed.split(/\s+/).pop() ?? '';
        if (lastWord.length > 3) {
          return DegradedReason.TruncatedMidWord;
        }
      }
    }
  }

  return null;
}
