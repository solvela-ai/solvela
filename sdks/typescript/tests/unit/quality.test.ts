import { describe, it, expect } from 'vitest';
import { checkDegraded, DegradedReason } from '../../src/quality.js';

describe('checkDegraded', () => {
  it('returns null for good content', () => {
    expect(checkDegraded('This is a perfectly good response.')).toBeNull();
  });

  it('detects empty content', () => {
    expect(checkDegraded('')).toBe(DegradedReason.EmptyContent);
    expect(checkDegraded('   ')).toBe(DegradedReason.EmptyContent);
  });

  it('detects known error phrases', () => {
    expect(checkDegraded('I apologize, but I cannot assist with that request.')).toBe(
      DegradedReason.KnownErrorPhrase,
    );
    expect(checkDegraded("I'm sorry, I can't help with that.")).toBe(
      DegradedReason.KnownErrorPhrase,
    );
    expect(checkDegraded('As an AI language model, I cannot do that.')).toBe(
      DegradedReason.KnownErrorPhrase,
    );
  });

  it('detects repetitive loops', () => {
    const repeated = 'the cat sat on the mat. '.repeat(20);
    expect(checkDegraded(repeated)).toBe(DegradedReason.RepetitiveLoop);
  });

  it('does not flag short repetition', () => {
    const short = 'hello hello hello';
    expect(checkDegraded(short)).toBeNull();
  });

  it('detects truncated mid-word', () => {
    expect(checkDegraded('The answer to the question is approximatel')).toBe(
      DegradedReason.TruncatedMidWord,
    );
  });

  it('does not flag normal endings', () => {
    expect(checkDegraded('The answer is 42.')).toBeNull();
    expect(checkDegraded('Here you go!')).toBeNull();
  });

  it('does not flag short content as truncated', () => {
    expect(checkDegraded('Hi')).toBeNull();
  });
});
