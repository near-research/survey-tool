/**
 * Tests for receipt parsing logic (extractSuccessValue).
 *
 * The function extracts the WASI output from NEAR transaction receipt chains.
 * It finds the last non-empty SuccessValue, falls back to transaction.outcome
 * or top-level status, and throws on failures or missing results.
 */

import { describe, it, expect } from 'vitest';
import { extractSuccessValue } from './near';
import type { TransactionResult } from './near';

// Helper: base64-encode a JSON object (mimics NEAR SuccessValue encoding)
function b64(obj: unknown): string {
  return btoa(JSON.stringify(obj));
}

describe('extractSuccessValue', () => {
  // ==================== Success paths ====================

  it('extracts last non-empty SuccessValue from receipts_outcome', () => {
    const tx: TransactionResult = {
      receipts_outcome: [
        { outcome: { status: { SuccessValue: '' } } },          // empty (cross-contract hop)
        { outcome: { status: { SuccessValue: b64({ success: true }) } } }, // WASI output
      ],
    };
    expect(extractSuccessValue(tx)).toBe(b64({ success: true }));
  });

  it('picks the last non-empty receipt when multiple have values', () => {
    const tx: TransactionResult = {
      receipts_outcome: [
        { outcome: { status: { SuccessValue: b64({ step: 1 }) } } },
        { outcome: { status: { SuccessValue: '' } } },
        { outcome: { status: { SuccessValue: b64({ step: 3 }) } } },
      ],
    };
    expect(extractSuccessValue(tx)).toBe(b64({ step: 3 }));
  });

  it('handles single receipt with non-empty value', () => {
    const tx: TransactionResult = {
      receipts_outcome: [
        { outcome: { status: { SuccessValue: b64({ ok: true }) } } },
      ],
    };
    expect(extractSuccessValue(tx)).toBe(b64({ ok: true }));
  });

  it('falls back to transaction.outcome when receipts are all empty', () => {
    const tx: TransactionResult = {
      receipts_outcome: [
        { outcome: { status: { SuccessValue: '' } } },
      ],
      transaction: {
        outcome: { status: { SuccessValue: b64({ fallback: true }) } },
      },
    };
    expect(extractSuccessValue(tx)).toBe(b64({ fallback: true }));
  });

  it('falls back to top-level status when no receipts or transaction outcome', () => {
    const tx: TransactionResult = {
      status: { SuccessValue: b64({ topLevel: true }) },
    };
    expect(extractSuccessValue(tx)).toBe(b64({ topLevel: true }));
  });

  it('falls back to top-level status when receipts_outcome is missing', () => {
    const tx: TransactionResult = {
      transaction: { outcome: { status: {} } },
      status: { SuccessValue: 'abc123' },
    };
    expect(extractSuccessValue(tx)).toBe('abc123');
  });

  it('returns empty string as SuccessValue (caller handles empty response)', () => {
    const tx: TransactionResult = {
      receipts_outcome: [],
      status: { SuccessValue: '' },
    };
    // Empty string is a valid SuccessValue (indicates WASI panic/OOM)
    expect(extractSuccessValue(tx)).toBe('');
  });

  // ==================== Failure paths ====================

  it('throws on receipt failure', () => {
    const tx: TransactionResult = {
      receipts_outcome: [
        { outcome: { status: { Failure: { ActionError: { kind: 'FunctionCallError' } } } } },
      ],
    };
    expect(() => extractSuccessValue(tx)).toThrow('OutLayer execution failed');
  });

  it('throws on failure even when other receipts succeed', () => {
    const tx: TransactionResult = {
      receipts_outcome: [
        { outcome: { status: { SuccessValue: b64({ ok: true }) } } },
        { outcome: { status: { Failure: { ActionError: { kind: 'Exceeded' } } } } },
      ],
    };
    expect(() => extractSuccessValue(tx)).toThrow('OutLayer execution failed');
  });

  it('throws when no success value found anywhere', () => {
    const tx: TransactionResult = {
      receipts_outcome: [],
    };
    expect(() => extractSuccessValue(tx)).toThrow('No result from OutLayer execution');
  });

  it('throws for completely empty object', () => {
    const tx: TransactionResult = {};
    expect(() => extractSuccessValue(tx)).toThrow('No result from OutLayer execution');
  });

  // ==================== Edge cases ====================

  it('handles receipts with missing outcome gracefully', () => {
    const tx: TransactionResult = {
      receipts_outcome: [
        {} as any,  // malformed receipt
        { outcome: { status: { SuccessValue: b64({ ok: true }) } } },
      ],
    };
    expect(extractSuccessValue(tx)).toBe(b64({ ok: true }));
  });

  it('handles receipts with undefined status', () => {
    const tx: TransactionResult = {
      receipts_outcome: [
        { outcome: { status: {} } },
      ],
      status: { SuccessValue: b64({ fallback: true }) },
    };
    expect(extractSuccessValue(tx)).toBe(b64({ fallback: true }));
  });

  it('prioritizes receipts_outcome over transaction.outcome', () => {
    const tx: TransactionResult = {
      receipts_outcome: [
        { outcome: { status: { SuccessValue: b64({ receipt: true }) } } },
      ],
      transaction: {
        outcome: { status: { SuccessValue: b64({ transaction: true }) } },
      },
    };
    expect(extractSuccessValue(tx)).toBe(b64({ receipt: true }));
  });

  it('prioritizes transaction.outcome over top-level status', () => {
    const tx: TransactionResult = {
      receipts_outcome: [
        { outcome: { status: { SuccessValue: '' } } },
      ],
      transaction: {
        outcome: { status: { SuccessValue: b64({ txOutcome: true }) } },
      },
      status: { SuccessValue: b64({ topLevel: true }) },
    };
    expect(extractSuccessValue(tx)).toBe(b64({ txOutcome: true }));
  });
});
