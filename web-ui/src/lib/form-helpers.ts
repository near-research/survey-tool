/**
 * Pure helper functions for form logic — no React or framework dependencies.
 * Extracted for testability and reuse.
 */

import type { FormQuestion } from './types';

export type AnswerValue = string | string[];
export type AnswerMap = Record<string, AnswerValue>;

/**
 * Determine if a question should be visible based on answers.
 */
export function isVisible(question: FormQuestion, answers: AnswerMap): boolean {
  if (!question.show_if) return true;

  const { question_id, value, values } = question.show_if;
  const answer = answers[question_id];

  if (answer === undefined) return false;

  if (values) {
    if (Array.isArray(answer)) {
      return answer.some(a => values.includes(a));
    }
    return values.includes(answer as string);
  }

  return answer === value;
}

/**
 * Reset answers for hidden questions and handle transitive chains.
 * E.g., q12 → q12b → q13: changing q12 clears q12b and q13.
 */
export function resetHiddenAnswers(answers: AnswerMap, questions: FormQuestion[]): AnswerMap {
  let changed = true;
  let result = { ...answers };
  let iterations = 0;
  const MAX_ITERATIONS = questions.length + 2;

  while (changed && iterations < MAX_ITERATIONS) {
    iterations++;
    changed = false;
    for (const q of questions) {
      if (!isVisible(q, result)) {
        const currentValue = result[q.id];
        const isEmpty = currentValue === '' || currentValue === undefined || (Array.isArray(currentValue) && currentValue.length === 0);

        if (!isEmpty) {
          result = { ...result, [q.id]: Array.isArray(currentValue) ? [] : '' };
          changed = true;
        }
      }
    }
  }

  return result;
}

/**
 * Compute the new selection for a multi-select question with exclusive options.
 * - Toggling an already-selected option removes it.
 * - Selecting an exclusive option deselects everything else.
 * - Selecting a regular option removes any exclusive options.
 */
export function applyExclusiveMultiSelect(
  current: string[],
  option: string,
  exclusiveOptions: string[]
): string[] {
  if (current.includes(option)) {
    return current.filter(v => v !== option);
  }
  const exclusiveSet = new Set(exclusiveOptions);
  if (exclusiveSet.has(option)) {
    return [option];
  }
  return [...current.filter(v => !exclusiveSet.has(v)), option];
}

// ==================== Error sanitization ====================

/** Patterns that are safe to show to end users (user-actionable messages). */
const USER_SAFE_PATTERNS = [
  'Please fill in',
  'Please provide contact',
  'Please sign in',
  'Form not loaded',
  'already submitted',
  'Not authorized',
  'Transaction cancelled',
  'not configured',
  'Each account can only submit once',
  'Request timed out',
  'HTTP ',
];

/**
 * Sanitize internal error messages to user-friendly text.
 * Passes through user-actionable messages; replaces technical internals with a generic fallback.
 */
export function sanitizeUserError(msg: string): string {
  for (const pattern of USER_SAFE_PATTERNS) {
    if (msg.includes(pattern)) {
      // Cap length to limit exposure if technical context is appended after the safe pattern
      return msg.length > 200 ? msg.slice(0, 200) + '\u2026' : msg;
    }
  }
  return 'Something went wrong. Please try again or contact the form administrator.';
}

// ==================== Response helpers ====================

export interface ResponseRecord {
  submitter_id: string;
  answers: Record<string, unknown>;
  submitted_at: string;
}

export type SortField = 'submitter_id' | 'submitted_at';
export type SortDirection = 'asc' | 'desc';

export function formatAnswer(value: unknown): string {
  if (value === undefined || value === null || value === '') {
    return '\u2014';
  }

  if (Array.isArray(value)) {
    const filtered = value.filter(v => v !== '');
    return filtered.join(' \u00b7 ');
  }

  return String(value);
}

export function getSortedResponses(
  responses: ResponseRecord[],
  field: SortField,
  direction: SortDirection
): ResponseRecord[] {
  const sorted = [...responses];
  sorted.sort((a, b) => {
    let aVal: string | number;
    let bVal: string | number;

    if (field === 'submitter_id') {
      aVal = a.submitter_id;
      bVal = b.submitter_id;
    } else {
      const aTime = new Date(a.submitted_at).getTime();
      const bTime = new Date(b.submitted_at).getTime();
      aVal = isNaN(aTime) ? 0 : aTime;
      bVal = isNaN(bTime) ? 0 : bTime;
    }

    if (aVal < bVal) return direction === 'asc' ? -1 : 1;
    if (aVal > bVal) return direction === 'asc' ? 1 : -1;
    return 0;
  });

  return sorted;
}

// ==================== NEAR helpers ====================

/**
 * Convert NEAR amount string to yoctoNEAR (bigint).
 * Uses string arithmetic to avoid Float64 precision loss.
 */
export function nearToYocto(nearStr: string): bigint {
  const trimmed = nearStr.trim();
  if (!/^\d+(\.\d{1,24})?$/.test(trimmed)) {
    throw new Error(`Invalid deposit format: "${nearStr}"`);
  }
  const parts = trimmed.split('.');
  const whole = parts[0] || '0';
  const frac = (parts[1] || '').padEnd(24, '0').slice(0, 24);
  const result = BigInt(whole) * 10n ** 24n + BigInt(frac);
  if (result <= 0n) throw new Error(`Invalid deposit: "${nearStr}" (must be positive)`);
  return result;
}
