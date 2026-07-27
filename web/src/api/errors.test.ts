import { describe, expect, it } from 'vitest';
import {
  extractRateLimitInfo,
  isAbortError,
  isApiErrorBody,
  normalizeErrorResponse,
  parseRetryAfterHeader,
  HttpApiError,
  NetworkError,
} from './errors';

function jsonResponse(
  status: number,
  body: unknown,
  headers: Record<string, string> = {},
): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json', ...headers },
  });
}

describe('isApiErrorBody', () => {
  it('accepts the documented shape', () => {
    expect(isApiErrorBody({ code: 'x', message: 'm', requestId: 'r' })).toBe(true);
    expect(isApiErrorBody({ code: 'x', message: 'm', requestId: 'r', details: { a: 1 } })).toBe(
      true,
    );
  });

  it('rejects anything missing a required field or the wrong type', () => {
    expect(isApiErrorBody(null)).toBe(false);
    expect(isApiErrorBody(undefined)).toBe(false);
    expect(isApiErrorBody('oops')).toBe(false);
    expect(isApiErrorBody({ code: 'x', message: 'm' })).toBe(false);
    expect(isApiErrorBody({ code: 1, message: 'm', requestId: 'r' })).toBe(false);
  });
});

describe('parseRetryAfterHeader', () => {
  it('reads an integer Retry-After header', () => {
    const response = new Response(null, { headers: { 'Retry-After': '7' } });
    expect(parseRetryAfterHeader(response)).toBe(7);
  });

  it('returns undefined when absent or unparsable', () => {
    expect(parseRetryAfterHeader(new Response(null))).toBeUndefined();
    expect(parseRetryAfterHeader(new Response(null, { headers: { 'Retry-After': 'soon' } }))).toBe(
      undefined,
    );
  });
});

describe('extractRateLimitInfo', () => {
  it('combines the Retry-After header with scope/quota from details on a 429', () => {
    const response = jsonResponse(
      429,
      {
        code: 'rate_limited',
        message: 'Too many requests',
        requestId: 'req-1',
        details: { retryAfterSeconds: 3, scope: 'auth', quota: 'rate_limit' },
      },
      { 'Retry-After': '3' },
    );
    const body = {
      code: 'rate_limited',
      message: 'Too many requests',
      requestId: 'req-1',
      details: { retryAfterSeconds: 3, scope: 'auth', quota: 'rate_limit' },
    };
    expect(extractRateLimitInfo(response, body)).toEqual({
      retryAfterSeconds: 3,
      scope: 'auth',
      quota: 'rate_limit',
    });
  });

  it('returns undefined for non-429 responses', () => {
    const response = new Response(null, { status: 500, headers: { 'Retry-After': '3' } });
    expect(extractRateLimitInfo(response, null)).toBeUndefined();
  });
});

describe('normalizeErrorResponse', () => {
  it('builds a HttpApiError from the canonical error envelope', async () => {
    const response = jsonResponse(404, {
      code: 'not_found',
      message: 'Document not found',
      requestId: 'req-2',
    });
    const error = await normalizeErrorResponse(response);
    expect(error).toBeInstanceOf(HttpApiError);
    expect(error.status).toBe(404);
    expect(error.code).toBe('not_found');
    expect(error.message).toBe('Document not found');
    expect(error.requestId).toBe('req-2');
    expect(error.rateLimit).toBeUndefined();
  });

  it('surfaces quota metadata on a 429 rather than swallowing it', async () => {
    const response = jsonResponse(
      429,
      {
        code: 'rate_limited',
        message: 'Too many requests',
        requestId: 'req-3',
        details: { retryAfterSeconds: 5, scope: 'ip', quota: 'rate_limit' },
      },
      { 'Retry-After': '5' },
    );
    const error = await normalizeErrorResponse(response);
    expect(error.status).toBe(429);
    expect(error.rateLimit).toEqual({ retryAfterSeconds: 5, scope: 'ip', quota: 'rate_limit' });
  });

  it('falls back to a usable error for a non-JSON or empty body', async () => {
    const empty = new Response(null, { status: 500, statusText: 'Internal Server Error' });
    const errorFromEmpty = await normalizeErrorResponse(empty);
    expect(errorFromEmpty.status).toBe(500);
    expect(errorFromEmpty.code).toBe('unknown_error');

    const html = new Response('<html>oops</html>', { status: 502, statusText: 'Bad Gateway' });
    const errorFromHtml = await normalizeErrorResponse(html);
    expect(errorFromHtml.status).toBe(502);
    expect(errorFromHtml.code).toBe('unknown_error');
  });
});

describe('isAbortError', () => {
  it('recognizes the DOMException fetch/AbortController produce', () => {
    expect(isAbortError(new DOMException('aborted', 'AbortError'))).toBe(true);
    expect(isAbortError(new Error('aborted'))).toBe(false);
    expect(isAbortError(new NetworkError('nope'))).toBe(false);
  });
});
