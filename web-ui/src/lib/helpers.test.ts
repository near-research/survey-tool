/**
 * Tests for pure helper functions across the web-ui.
 *
 * Covers: isVisible, resetHiddenAnswers, applyExclusiveMultiSelect,
 * formatAnswer, getSortedResponses, nearToYocto (form-helpers.ts),
 * getFormApiUrl (hooks.ts).
 */

import { describe, it, expect, beforeEach } from 'vitest';
import type { FormQuestion } from './types';
import {
  isVisible,
  resetHiddenAnswers,
  applyExclusiveMultiSelect,
  formatAnswer,
  getSortedResponses,
  nearToYocto,
  sanitizeUserError,
} from './form-helpers';
import type { AnswerMap } from './form-helpers';
import { getFormApiUrl } from './hooks';

// ==================== Test utilities ====================

/** Minimal question factory — only fills fields relevant to the function under test. */
function q(overrides: Partial<FormQuestion> & { id: string }): FormQuestion {
  return {
    section: 1,
    section_title: 'Test',
    label: overrides.id,
    type: 'single_select',
    options: null,
    optional: false,
    show_if: null,
    ...overrides,
  };
}

// ==================== isVisible ====================

describe('isVisible', () => {
  it('returns true when no show_if', () => {
    expect(isVisible(q({ id: 'q1' }), {})).toBe(true);
  });

  it('returns false when dependency not yet answered', () => {
    const question = q({ id: 'q2', show_if: { question_id: 'q1', value: 'Yes' } });
    expect(isVisible(question, {})).toBe(false);
  });

  it('returns true when single value matches', () => {
    const question = q({ id: 'q2', show_if: { question_id: 'q1', value: 'Yes' } });
    expect(isVisible(question, { q1: 'Yes' })).toBe(true);
  });

  it('returns false when single value does not match', () => {
    const question = q({ id: 'q2', show_if: { question_id: 'q1', value: 'Yes' } });
    expect(isVisible(question, { q1: 'No' })).toBe(false);
  });

  it('matches multi-value conditional (values array) with string answer', () => {
    const question = q({ id: 'q2', show_if: { question_id: 'q1', values: ['A', 'B'] } });
    expect(isVisible(question, { q1: 'A' })).toBe(true);
    expect(isVisible(question, { q1: 'C' })).toBe(false);
  });

  it('matches multi-value conditional with array answer (multi_select)', () => {
    const question = q({ id: 'q2', show_if: { question_id: 'q1', values: ['A', 'B'] } });
    expect(isVisible(question, { q1: ['C', 'A'] })).toBe(true);
    expect(isVisible(question, { q1: ['C', 'D'] })).toBe(false);
  });

  it('returns false when array answer is empty', () => {
    const question = q({ id: 'q2', show_if: { question_id: 'q1', values: ['A'] } });
    expect(isVisible(question, { q1: [] })).toBe(false);
  });
});

// ==================== resetHiddenAnswers ====================

describe('resetHiddenAnswers', () => {
  it('clears answers for hidden questions', () => {
    const questions = [
      q({ id: 'q1' }),
      q({ id: 'q2', show_if: { question_id: 'q1', value: 'Yes' } }),
    ];
    const answers: AnswerMap = { q1: 'No', q2: 'leftover' };
    const result = resetHiddenAnswers(answers, questions);
    expect(result.q2).toBe('');
    expect(result.q1).toBe('No');
  });

  it('handles transitive chains (q1 → q2 → q3)', () => {
    const questions = [
      q({ id: 'q1' }),
      q({ id: 'q2', show_if: { question_id: 'q1', value: 'Yes' } }),
      q({ id: 'q3', show_if: { question_id: 'q2', value: 'A' } }),
    ];
    const answers: AnswerMap = { q1: 'No', q2: 'A', q3: 'data' };
    const result = resetHiddenAnswers(answers, questions);
    expect(result.q2).toBe('');
    expect(result.q3).toBe('');
  });

  it('clears array answers to empty array', () => {
    const questions = [
      q({ id: 'q1' }),
      q({ id: 'q2', type: 'multi_select', show_if: { question_id: 'q1', value: 'Yes' } }),
    ];
    const answers: AnswerMap = { q1: 'No', q2: ['X', 'Y'] };
    const result = resetHiddenAnswers(answers, questions);
    expect(result.q2).toEqual([]);
  });

  it('does not clear already-empty answers (no infinite loop)', () => {
    const questions = [
      q({ id: 'q1' }),
      q({ id: 'q2', show_if: { question_id: 'q1', value: 'Yes' } }),
    ];
    const answers: AnswerMap = { q1: 'No', q2: '' };
    const result = resetHiddenAnswers(answers, questions);
    expect(result.q2).toBe('');
  });

  it('leaves visible answers untouched', () => {
    const questions = [
      q({ id: 'q1' }),
      q({ id: 'q2', show_if: { question_id: 'q1', value: 'Yes' } }),
    ];
    const answers: AnswerMap = { q1: 'Yes', q2: 'keep me' };
    const result = resetHiddenAnswers(answers, questions);
    expect(result.q2).toBe('keep me');
  });
});

// ==================== applyExclusiveMultiSelect ====================

describe('applyExclusiveMultiSelect', () => {
  const exclusive = ['None of the above', 'Prefer not to say'];

  it('adds a regular option', () => {
    expect(applyExclusiveMultiSelect([], 'A', exclusive)).toEqual(['A']);
  });

  it('adds multiple regular options', () => {
    expect(applyExclusiveMultiSelect(['A'], 'B', exclusive)).toEqual(['A', 'B']);
  });

  it('toggles off a selected option', () => {
    expect(applyExclusiveMultiSelect(['A', 'B'], 'A', exclusive)).toEqual(['B']);
  });

  it('selecting exclusive deselects everything else', () => {
    expect(applyExclusiveMultiSelect(['A', 'B'], 'None of the above', exclusive))
      .toEqual(['None of the above']);
  });

  it('selecting regular removes exclusive options', () => {
    expect(applyExclusiveMultiSelect(['None of the above'], 'A', exclusive))
      .toEqual(['A']);
  });

  it('selecting exclusive from empty', () => {
    expect(applyExclusiveMultiSelect([], 'Prefer not to say', exclusive))
      .toEqual(['Prefer not to say']);
  });

  it('toggling off exclusive leaves empty', () => {
    expect(applyExclusiveMultiSelect(['None of the above'], 'None of the above', exclusive))
      .toEqual([]);
  });
});

// ==================== formatAnswer ====================

describe('formatAnswer', () => {
  it('returns em-dash for undefined', () => {
    expect(formatAnswer(undefined)).toBe('\u2014');
  });

  it('returns em-dash for null', () => {
    expect(formatAnswer(null)).toBe('\u2014');
  });

  it('returns em-dash for empty string', () => {
    expect(formatAnswer('')).toBe('\u2014');
  });

  it('joins array with middle dot', () => {
    expect(formatAnswer(['A', 'B', 'C'])).toBe('A \u00b7 B \u00b7 C');
  });

  it('returns single array element as-is', () => {
    expect(formatAnswer(['Only'])).toBe('Only');
  });

  it('returns empty array as empty string', () => {
    expect(formatAnswer([])).toBe('');
  });

  it('stringifies numbers', () => {
    expect(formatAnswer(42)).toBe('42');
  });

  it('stringifies booleans', () => {
    expect(formatAnswer(true)).toBe('true');
  });

  it('returns plain strings directly', () => {
    expect(formatAnswer('hello')).toBe('hello');
  });

  it('formats rank answers with different rank counts', () => {
    // rank_count=2: only first 2 slots matter
    expect(formatAnswer(['A', 'B'])).toBe('A \u00b7 B');
    // rank_count=5: 5 slots, some empty
    expect(formatAnswer(['A', '', 'C', '', 'E'])).toBe('A \u00b7 C \u00b7 E');
  });

  it('filters empty strings from rank arrays', () => {
    expect(formatAnswer(['First', '', 'Third'])).toBe('First \u00b7 Third');
  });
});

// ==================== getSortedResponses ====================

describe('getSortedResponses', () => {
  const responses = [
    { submitter_id: 'charlie.near', answers: {}, submitted_at: '2026-03-03T10:00:00Z' },
    { submitter_id: 'alice.near', answers: {}, submitted_at: '2026-03-01T08:00:00Z' },
    { submitter_id: 'bob.near', answers: {}, submitted_at: '2026-03-02T09:00:00Z' },
  ];

  it('sorts by submitter_id ascending', () => {
    const sorted = getSortedResponses(responses, 'submitter_id', 'asc');
    expect(sorted.map(r => r.submitter_id)).toEqual(['alice.near', 'bob.near', 'charlie.near']);
  });

  it('sorts by submitter_id descending', () => {
    const sorted = getSortedResponses(responses, 'submitter_id', 'desc');
    expect(sorted.map(r => r.submitter_id)).toEqual(['charlie.near', 'bob.near', 'alice.near']);
  });

  it('sorts by submitted_at ascending', () => {
    const sorted = getSortedResponses(responses, 'submitted_at', 'asc');
    expect(sorted.map(r => r.submitter_id)).toEqual(['alice.near', 'bob.near', 'charlie.near']);
  });

  it('sorts by submitted_at descending', () => {
    const sorted = getSortedResponses(responses, 'submitted_at', 'desc');
    expect(sorted.map(r => r.submitter_id)).toEqual(['charlie.near', 'bob.near', 'alice.near']);
  });

  it('handles malformed dates (NaN guard)', () => {
    const withBad = [
      { submitter_id: 'a.near', answers: {}, submitted_at: 'not-a-date' },
      { submitter_id: 'b.near', answers: {}, submitted_at: '2026-01-01T00:00:00Z' },
    ];
    const sorted = getSortedResponses(withBad, 'submitted_at', 'asc');
    expect(sorted[0].submitter_id).toBe('a.near');
  });

  it('does not mutate original array', () => {
    const original = [...responses];
    getSortedResponses(responses, 'submitter_id', 'asc');
    expect(responses).toEqual(original);
  });
});

// ==================== getFormApiUrl ====================

describe('getFormApiUrl', () => {
  beforeEach(() => {
    delete process.env.NEXT_PUBLIC_DATABASE_API_URL;
    delete process.env.NEXT_PUBLIC_FORM_ID;
  });

  it('returns null when no formId and no env var', () => {
    expect(getFormApiUrl()).toBeNull();
  });

  it('uses explicit formId parameter', () => {
    expect(getFormApiUrl('abc-123')).toBe('http://localhost:4001/v1/forms/abc-123');
  });

  it('uses NEXT_PUBLIC_FORM_ID when no parameter', () => {
    process.env.NEXT_PUBLIC_FORM_ID = 'env-form-id';
    expect(getFormApiUrl()).toBe('http://localhost:4001/v1/forms/env-form-id');
  });

  it('uses custom database API URL', () => {
    process.env.NEXT_PUBLIC_DATABASE_API_URL = 'https://api.example.com';
    expect(getFormApiUrl('my-form')).toBe('https://api.example.com/v1/forms/my-form');
  });

  it('encodes special characters in formId', () => {
    const url = getFormApiUrl('form with spaces');
    expect(url).toBe('http://localhost:4001/v1/forms/form%20with%20spaces');
  });

  it('parameter takes precedence over env var', () => {
    process.env.NEXT_PUBLIC_FORM_ID = 'env-id';
    expect(getFormApiUrl('param-id')).toBe('http://localhost:4001/v1/forms/param-id');
  });
});

// ==================== nearToYocto ====================

describe('nearToYocto', () => {
  it('converts 1 NEAR to 10^24 yoctoNEAR', () => {
    expect(nearToYocto('1')).toBe(10n ** 24n);
  });

  it('converts 0.025 NEAR correctly (no Float64 loss)', () => {
    expect(nearToYocto('0.025')).toBe(25n * 10n ** 21n);
  });

  it('converts whole number with no decimal', () => {
    expect(nearToYocto('5')).toBe(5n * 10n ** 24n);
  });

  it('handles maximum decimal precision (24 digits)', () => {
    expect(nearToYocto('0.000000000000000000000001')).toBe(1n);
  });

  it('trims whitespace', () => {
    expect(nearToYocto('  1  ')).toBe(10n ** 24n);
  });

  it('rejects negative values', () => {
    expect(() => nearToYocto('-1')).toThrow('Invalid deposit');
  });

  it('rejects empty string', () => {
    expect(() => nearToYocto('')).toThrow('Invalid deposit');
  });

  it('rejects non-numeric input', () => {
    expect(() => nearToYocto('abc')).toThrow('Invalid deposit');
  });

  it('rejects zero', () => {
    expect(() => nearToYocto('0')).toThrow('must be positive');
  });

  it('rejects 0.0', () => {
    expect(() => nearToYocto('0.0')).toThrow('must be positive');
  });
});

// ==================== Contact answer validation ====================
// Tests for the inline contact validation logic in forms/[id].tsx:389-402.
// The validation check is: v.startsWith('Yes') && v.includes(':') && detail.trim() === ''
// We test the predicate directly to document edge cases.

describe('contact answer validation predicate', () => {
  /**
   * Mirrors the inline validation in forms/[id].tsx:
   *   typeof v === 'string' && v.startsWith('Yes') && v.includes(':')
   *     && v.split(':').slice(1).join(':').trim() === ''
   * Returns true when the contact answer is INVALID (Yes selected, but detail empty).
   */
  function isInvalidContact(v: unknown): boolean {
    return typeof v === 'string'
      && v.startsWith('Yes')
      && v.includes(':')
      && v.split(':').slice(1).join(':').trim() === '';
  }

  it('flags "Yes:" as invalid (empty detail)', () => {
    expect(isInvalidContact('Yes:')).toBe(true);
  });

  it('flags "Yes, email:" as invalid (empty detail)', () => {
    expect(isInvalidContact('Yes, email:')).toBe(true);
  });

  it('flags "Yes:" with whitespace-only detail as invalid', () => {
    expect(isInvalidContact('Yes:   ')).toBe(true);
  });

  it('accepts "Yes:alice@example.com" as valid', () => {
    expect(isInvalidContact('Yes:alice@example.com')).toBe(false);
  });

  it('accepts "Yes, email:alice@example.com" as valid', () => {
    expect(isInvalidContact('Yes, email:alice@example.com')).toBe(false);
  });

  it('handles multiple colons (detail contains colon)', () => {
    expect(isInvalidContact('Yes:http://example.com')).toBe(false);
  });

  it('accepts "No" (non-Yes option, no detail needed)', () => {
    expect(isInvalidContact('No')).toBe(false);
  });

  it('accepts empty string (unanswered — caught by required field check)', () => {
    expect(isInvalidContact('')).toBe(false);
  });

  it('accepts non-string values', () => {
    expect(isInvalidContact(undefined)).toBe(false);
    expect(isInvalidContact(null)).toBe(false);
    expect(isInvalidContact(['Yes:'])).toBe(false);
  });

  // Documents the known gap: "Yes" without colon passes validation.
  // The ContactQuestion component always produces "Yes:" (with colon),
  // so this case shouldn't occur in practice, but it's documented here.
  it('"Yes" without colon passes validation (known gap — ContactQuestion always adds colon)', () => {
    expect(isInvalidContact('Yes')).toBe(false);
  });
});

// ==================== sanitizeUserError ====================

describe('sanitizeUserError', () => {
  it('passes through "already submitted" messages', () => {
    const msg = 'You have already submitted this form. Each account can only submit once.';
    expect(sanitizeUserError(msg)).toBe(msg);
  });

  it('passes through "Not authorized" messages', () => {
    const msg = 'Not authorized to read responses';
    expect(sanitizeUserError(msg)).toBe(msg);
  });

  it('passes through "Please sign in" messages', () => {
    const msg = 'Please sign in with your NEAR wallet';
    expect(sanitizeUserError(msg)).toBe(msg);
  });

  it('sanitizes internal crypto errors', () => {
    expect(sanitizeUserError('Failed to derive form key: secp256k1 error')).toBe(
      'Something went wrong. Please try again or contact the form administrator.'
    );
  });

  it('sanitizes database errors', () => {
    expect(sanitizeUserError('connection refused to localhost:4001')).toBe(
      'Something went wrong. Please try again or contact the form administrator.'
    );
  });

  it('sanitizes unknown errors', () => {
    expect(sanitizeUserError('TypeError: Cannot read properties of null')).toBe(
      'Something went wrong. Please try again or contact the form administrator.'
    );
  });

  it('passes through "not configured" messages', () => {
    expect(sanitizeUserError('NEXT_PUBLIC_MASTER_PUBLIC_KEY not configured')).toContain('not configured');
  });
});
