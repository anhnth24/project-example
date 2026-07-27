import { describe, expect, it, beforeEach } from 'vitest';
import {
  mockUuid,
  mockTimestamp,
  encodeCursor,
  decodeCursor,
  nextRequestId,
  resetRequestIdCounter,
} from './ids';

describe('mockUuid', () => {
  it('is deterministic and syntactically a uuid', () => {
    expect(mockUuid(1)).toBe(mockUuid(1));
    expect(mockUuid(1)).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/);
  });

  it('differs for different seeds', () => {
    expect(mockUuid(1)).not.toBe(mockUuid(2));
  });

  it('rejects negative or non-integer seeds', () => {
    expect(() => mockUuid(-1)).toThrow();
    expect(() => mockUuid(1.5)).toThrow();
  });
});

describe('mockTimestamp', () => {
  it('is deterministic and parses as a valid date-time', () => {
    const ts = mockTimestamp(5);
    expect(ts).toBe(mockTimestamp(5));
    expect(Number.isNaN(Date.parse(ts))).toBe(false);
  });

  it('increases with the step', () => {
    expect(Date.parse(mockTimestamp(10))).toBeGreaterThan(Date.parse(mockTimestamp(5)));
  });
});

describe('pagination cursors', () => {
  it('round-trips through encode/decode', () => {
    for (const offset of [0, 1, 50, 12345]) {
      expect(decodeCursor(encodeCursor(offset))).toBe(offset);
    }
  });

  it('treats an absent cursor as offset 0', () => {
    expect(decodeCursor(undefined)).toBe(0);
    expect(decodeCursor(null)).toBe(0);
  });

  it('rejects a cursor it did not mint', () => {
    expect(() => decodeCursor('not-a-real-cursor')).toThrow();
  });

  it('is opaque (not a plain-text offset)', () => {
    expect(encodeCursor(7)).not.toContain('7');
  });
});

describe('nextRequestId', () => {
  beforeEach(() => resetRequestIdCounter());

  it('produces a fresh uuid each call and resets cleanly', () => {
    const a = nextRequestId();
    const b = nextRequestId();
    expect(a).not.toBe(b);
    resetRequestIdCounter();
    expect(nextRequestId()).toBe(a);
  });
});
