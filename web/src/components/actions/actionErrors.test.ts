import { describe, expect, it } from 'vitest';
import { HttpApiError, NetworkError } from '../../api/errors';
import { describeActionError } from './actionErrors';

function httpError(
  status: number,
  overrides: Partial<ConstructorParameters<typeof HttpApiError>[0]> = {},
) {
  return new HttpApiError({
    status,
    code: overrides.code ?? 'some_code',
    message: overrides.message ?? 'Server message',
    requestId: overrides.requestId ?? 'req-1',
    rateLimit: overrides.rateLimit,
  });
}

describe('describeActionError', () => {
  it('gives a distinct message for 403, independent of the action', () => {
    const message = describeActionError(httpError(403), 'download');
    expect(message).toMatch(/quyền/i);
    expect(describeActionError(httpError(403), 'reindex')).toBe(message);
    expect(describeActionError(httpError(403), 'delete')).toBe(message);
  });

  it('gives a distinct message for 429 that includes the retry-after seconds when known', () => {
    const withRetry = describeActionError(
      httpError(429, { rateLimit: { retryAfterSeconds: 12 } }),
      'reindex',
    );
    expect(withRetry).toContain('12 giây');

    const withoutRetry = describeActionError(httpError(429), 'reindex');
    expect(withoutRetry).not.toBe(withRetry);
    expect(withoutRetry).toMatch(/quá nhiều yêu cầu/i);
  });

  it('rounds a long retry-after into minutes', () => {
    const message = describeActionError(
      httpError(429, { rateLimit: { retryAfterSeconds: 125 } }),
      'delete',
    );
    expect(message).toMatch(/3 phút/);
  });

  it('gives download a single-use-capability-specific 404 message, distinct from other actions', () => {
    const downloadMessage = describeActionError(httpError(404), 'download');
    const deleteMessage = describeActionError(httpError(404), 'delete');
    expect(downloadMessage).toMatch(/hết hạn|đã được dùng/);
    expect(deleteMessage).not.toBe(downloadMessage);
  });

  it('falls back to the network-error copy for NetworkError', () => {
    expect(describeActionError(new NetworkError('boom'), 'reindex')).toMatch(/kết nối/i);
  });

  it('falls back to a generic message for anything else', () => {
    expect(describeActionError(new Error('weird'), 'delete')).toMatch(/không thể hoàn tất/i);
  });
});
