import { describe, expect, it } from 'vitest';
import { describeUploadFailure } from './messages';
import type { UploadOutcome } from './types';

function apiError(message: string, details?: unknown) {
  return { code: 'x', message, requestId: 'r1', details };
}

describe('describeUploadFailure', () => {
  it('surfaces a size-specific message for 413, distinct from the generic one', () => {
    const message = describeUploadFailure({ kind: 'too-large', error: null });
    expect(message).toMatch(/dung lượng/i);
  });

  it('surfaces a conflict-specific message for 409', () => {
    const outcome: UploadOutcome = {
      kind: 'conflict',
      error: apiError('Idempotency replay mismatch.'),
    };
    expect(describeUploadFailure(outcome)).toBe('Idempotency replay mismatch.');
  });

  it('falls back to a Vietnamese conflict message when the server sent no body', () => {
    const message = describeUploadFailure({ kind: 'conflict', error: null });
    expect(message).toMatch(/xung đột/i);
  });

  it('surfaces a quota-specific message for 429, including the retry-after seconds', () => {
    const outcome: UploadOutcome = {
      kind: 'quota',
      error: apiError('Too many requests; quota exhausted.'),
      rateLimit: { retryAfterSeconds: 42, details: undefined },
    };
    const message = describeUploadFailure(outcome);
    expect(message).toContain('42 giây');
    expect(message).toContain('Too many requests; quota exhausted.');
  });

  it('quota message without a known retry-after still reads as a quota message', () => {
    const outcome: UploadOutcome = {
      kind: 'quota',
      error: null,
      rateLimit: { retryAfterSeconds: undefined, details: undefined },
    };
    expect(describeUploadFailure(outcome)).toMatch(/hạn mức/i);
  });

  it('the three messages (too-large, conflict, quota) are all different from each other and from the generic fallback', () => {
    const generic = describeUploadFailure({ kind: 'http-error', status: 500, error: null });
    const tooLarge = describeUploadFailure({ kind: 'too-large', error: null });
    const conflict = describeUploadFailure({ kind: 'conflict', error: null });
    const quota = describeUploadFailure({
      kind: 'quota',
      error: null,
      rateLimit: { retryAfterSeconds: undefined, details: undefined },
    });
    const messages = [generic, tooLarge, conflict, quota];
    expect(new Set(messages).size).toBe(messages.length);
  });

  it('distinguishes network errors from generic HTTP errors', () => {
    expect(describeUploadFailure({ kind: 'network-error' })).toMatch(/kết nối mạng/i);
  });

  it('has a distinct message for session loss', () => {
    expect(describeUploadFailure({ kind: 'session-lost' })).toMatch(/phiên đăng nhập/i);
  });
});
