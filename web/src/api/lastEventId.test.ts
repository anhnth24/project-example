import { describe, expect, it } from 'vitest';
import {
  advanceCursor,
  createCursor,
  nextExpected,
  parseLastEventIdToken,
  requestCursorParams,
  resolveLastEventId,
  ZERO_CURSOR,
} from './lastEventId';

describe('parseLastEventIdToken', () => {
  // Fixtures mirror crates/server/src/api/last_event_id.rs tests
  // (`accepts_zero_through_i64_max`, `rejects_malformed_negative_overflow_conflict_future`)
  // so this module is verifiably the same contract, not a lookalike.
  it('accepts zero through i64::MAX', () => {
    expect(parseLastEventIdToken('0')).toEqual({ ok: true, value: 0n });
    expect(parseLastEventIdToken('42')).toEqual({ ok: true, value: 42n });
    expect(parseLastEventIdToken('9223372036854775807')).toEqual({
      ok: true,
      value: 9223372036854775807n,
    });
  });

  it('rejects malformed, negative, overflow', () => {
    expect(parseLastEventIdToken('-1')).toEqual({ ok: false, error: 'negative' });
    expect(parseLastEventIdToken('1.5')).toEqual({ ok: false, error: 'malformed' });
    expect(parseLastEventIdToken('abc')).toEqual({ ok: false, error: 'malformed' });
    expect(parseLastEventIdToken('')).toEqual({ ok: false, error: 'malformed' });
    expect(parseLastEventIdToken('   ')).toEqual({ ok: false, error: 'malformed' });
    expect(parseLastEventIdToken('9223372036854775808')).toEqual({
      ok: false,
      error: 'out_of_range',
    });
  });

  it('tolerates leading/trailing whitespace and leading zeros, like Rust i64::parse', () => {
    expect(parseLastEventIdToken('  7  ')).toEqual({ ok: true, value: 7n });
    expect(parseLastEventIdToken('007')).toEqual({ ok: true, value: 7n });
  });
});

describe('resolveLastEventId', () => {
  it('both absent -> 0', () => {
    expect(resolveLastEventId(undefined, undefined)).toEqual({ ok: true, value: 0n });
  });

  it('single source wins', () => {
    expect(resolveLastEventId('9', undefined)).toEqual({ ok: true, value: 9n });
    expect(resolveLastEventId(undefined, '9')).toEqual({ ok: true, value: 9n });
  });

  it('equal query+header is accepted', () => {
    expect(resolveLastEventId('3', '3')).toEqual({ ok: true, value: 3n });
  });

  it('unequal query+header conflicts', () => {
    expect(resolveLastEventId('3', '4')).toEqual({ ok: false, error: 'conflicting' });
  });

  it('rejects a cursor from the future against a high-water mark', () => {
    expect(resolveLastEventId('9', undefined, 5n)).toEqual({ ok: false, error: 'out_of_range' });
  });

  it('propagates a malformed token from either source', () => {
    expect(resolveLastEventId('abc', undefined)).toEqual({ ok: false, error: 'malformed' });
    expect(resolveLastEventId(undefined, '-1')).toEqual({ ok: false, error: 'negative' });
  });
});

describe('createCursor', () => {
  it('defaults to the zero cursor', () => {
    expect(createCursor()).toBe(ZERO_CURSOR);
    expect(createCursor()).toBe('0');
  });

  it('normalizes a valid seed', () => {
    expect(createCursor('007')).toBe('7');
  });

  it('throws on an invalid seed rather than silently starting at 0', () => {
    expect(() => createCursor('not-a-number')).toThrow(/malformed/);
    expect(() => createCursor('-1')).toThrow(/negative/);
  });
});

describe('nextExpected', () => {
  it('increments by one', () => {
    expect(nextExpected('0')).toBe('1');
    expect(nextExpected('41')).toBe('42');
  });

  it('does not lose precision above Number.MAX_SAFE_INTEGER', () => {
    // 2**53 + 1, one past the point where `Number` can no longer represent
    // consecutive integers exactly. A Number-based implementation would
    // compute the wrong next value here.
    const big = '9007199254740993';
    expect(nextExpected(big)).toBe('9007199254740994');
  });
});

describe('advanceCursor', () => {
  it('reports a contiguous advance', () => {
    expect(advanceCursor('4', '5')).toEqual({
      ok: true,
      cursor: '5',
      contiguous: true,
      expected: '5',
    });
  });

  it('reports a non-contiguous but strictly increasing advance as a gap, not an error', () => {
    // Legitimate for job events: event_log.sequence_no is one counter shared
    // by the whole org (jobs.rs next_event_sequence), so another job's
    // events consume sequence numbers in between. This must not be treated
    // as a protocol failure.
    expect(advanceCursor('2', '7')).toEqual({
      ok: true,
      cursor: '7',
      contiguous: false,
      expected: '3',
    });
  });

  it('rejects a non-increasing id as not_monotonic (duplicate/out-of-order)', () => {
    expect(advanceCursor('5', '5')).toEqual({ ok: false, error: 'not_monotonic' });
    expect(advanceCursor('5', '3')).toEqual({ ok: false, error: 'not_monotonic' });
  });

  it('rejects a malformed id without throwing', () => {
    expect(advanceCursor('5', 'abc')).toEqual({ ok: false, error: 'malformed' });
    expect(advanceCursor('5', '-1')).toEqual({ ok: false, error: 'negative' });
  });
});

describe('requestCursorParams', () => {
  it('omits both for the zero cursor (fresh connect)', () => {
    expect(requestCursorParams('0')).toBeUndefined();
  });

  it('sends equal query and header values for a resume', () => {
    expect(requestCursorParams('42')).toEqual({ query: '42', header: '42' });
  });
});
