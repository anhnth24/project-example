import { afterEach, describe, expect, it, vi } from 'vitest';
import { createApiClient } from './client';
import { HttpApiError, NetworkError, isAbortError } from './errors';
import type { SessionTokens } from './session';

function jsonResponse(
  status: number,
  body: unknown,
  headers: Record<string, string> = {},
): Response {
  return new Response(body === undefined ? null : JSON.stringify(body), {
    status,
    headers: { 'Content-Type': 'application/json', ...headers },
  });
}

function noContentResponse(status = 204): Response {
  return new Response(null, { status });
}

function bearerOf(init: RequestInit | undefined): string | undefined {
  const headers = init?.headers as Record<string, string> | undefined;
  return headers?.Authorization;
}

function validTokens(overrides: Partial<SessionTokens> = {}): SessionTokens {
  return {
    accessToken: 'old-token',
    refreshToken: 'refresh-1',
    tokenType: 'Bearer',
    expiresIn: 3600,
    orgId: 'org-1',
    userId: 'user-1',
    ...overrides,
  };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('request()', () => {
  it('prefixes business routes with /api/v1 and injects a bearer access token', async () => {
    const fetchMock = vi.fn(async (input: string | URL, init?: RequestInit) => {
      expect(String(input)).toBe('/api/v1/collections');
      expect(bearerOf(init)).toBe('Bearer old-token');
      return jsonResponse(200, { items: [], page: { hasMore: false } });
    });
    vi.stubGlobal('fetch', fetchMock);

    const client = createApiClient({ baseUrl: '' });
    client.sessionManager.setTokens(validTokens());

    const result = await client.request('get', '/collections');
    expect(result).toEqual({ items: [], page: { hasMore: false } });
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('retries exactly once after a 401 with a freshly refreshed token, and does not loop on a persistent 401', async () => {
    let refreshCalls = 0;
    let businessCallsWithNewToken = 0;
    const fetchMock = vi.fn(async (input: string | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === '/api/v1/auth/refresh') {
        refreshCalls++;
        return jsonResponse(
          200,
          validTokens({ accessToken: 'new-token', refreshToken: 'refresh-2' }),
        );
      }
      if (url === '/api/v1/collections') {
        if (bearerOf(init) === 'Bearer new-token') businessCallsWithNewToken++;
        return jsonResponse(401, { code: 'unauthorized', message: 'expired', requestId: 'r' });
      }
      throw new Error(`unexpected url ${url}`);
    });
    vi.stubGlobal('fetch', fetchMock);

    const client = createApiClient({ baseUrl: '' });
    client.sessionManager.setTokens(validTokens());

    await expect(client.request('get', '/collections')).rejects.toMatchObject({ status: 401 });
    expect(refreshCalls).toBe(1);
    expect(businessCallsWithNewToken).toBe(1);
    // 1 initial (old token) + 1 refresh + 1 retry (new token) = 3, never a second refresh.
    expect(fetchMock).toHaveBeenCalledTimes(3);
  });

  it('single-flights the refresh across N concurrent requests that each hit a 401, and all N succeed with the new token', async () => {
    let refreshCalls = 0;
    const fetchMock = vi.fn(async (input: string | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === '/api/v1/auth/refresh') {
        refreshCalls++;
        return jsonResponse(
          200,
          validTokens({ accessToken: 'new-token', refreshToken: 'refresh-2' }),
        );
      }
      if (url === '/api/v1/collections') {
        if (bearerOf(init) === 'Bearer new-token') {
          return jsonResponse(200, { items: [], page: { hasMore: false } });
        }
        return jsonResponse(401, { code: 'unauthorized', message: 'expired', requestId: 'r' });
      }
      throw new Error(`unexpected url ${url}`);
    });
    vi.stubGlobal('fetch', fetchMock);

    const client = createApiClient({ baseUrl: '' });
    client.sessionManager.setTokens(validTokens());

    const results = await Promise.all([
      client.request('get', '/collections'),
      client.request('get', '/collections'),
      client.request('get', '/collections'),
      client.request('get', '/collections'),
    ]);

    for (const result of results) {
      expect(result).toEqual({ items: [], page: { hasMore: false } });
    }
    expect(refreshCalls).toBe(1);
  });

  it('rejects every concurrent request and fires onSessionLost exactly once when the refresh itself fails', async () => {
    let refreshCalls = 0;
    const fetchMock = vi.fn(async (input: string | URL) => {
      const url = String(input);
      if (url === '/api/v1/auth/refresh') {
        refreshCalls++;
        return jsonResponse(401, {
          code: 'invalid_refresh_token',
          message: 'expired',
          requestId: 'r',
        });
      }
      if (url === '/api/v1/collections') {
        return jsonResponse(401, { code: 'unauthorized', message: 'expired', requestId: 'r' });
      }
      throw new Error(`unexpected url ${url}`);
    });
    vi.stubGlobal('fetch', fetchMock);

    const client = createApiClient({ baseUrl: '' });
    client.sessionManager.setTokens(validTokens());
    const lost = vi.fn();
    client.tokenProvider.onSessionLost(lost);

    const results = await Promise.allSettled([
      client.request('get', '/collections'),
      client.request('get', '/collections'),
      client.request('get', '/collections'),
    ]);

    for (const result of results) {
      expect(result.status).toBe('rejected');
    }
    expect(refreshCalls).toBe(1);
    expect(lost).toHaveBeenCalledTimes(1);
  });

  it('surfaces 429 quota metadata instead of swallowing it', async () => {
    const fetchMock = vi.fn(async () =>
      jsonResponse(
        429,
        {
          code: 'rate_limited',
          message: 'Too many requests',
          requestId: 'r',
          details: { retryAfterSeconds: 4, scope: 'user', quota: 'rate_limit' },
        },
        { 'Retry-After': '4' },
      ),
    );
    vi.stubGlobal('fetch', fetchMock);

    const client = createApiClient({ baseUrl: '' });
    client.sessionManager.setTokens(validTokens());

    const error = await client.request('get', '/collections').catch((e: unknown) => e);
    expect(error).toBeInstanceOf(HttpApiError);
    const httpError = error as HttpApiError;
    expect(httpError.status).toBe(429);
    expect(httpError.rateLimit).toEqual({
      retryAfterSeconds: 4,
      scope: 'user',
      quota: 'rate_limit',
    });
  });

  it('propagates AbortError without wrapping it, and does not attempt a refresh retry', async () => {
    const fetchMock = vi.fn(async (_input: string | URL, init?: RequestInit) => {
      if (init?.signal?.aborted) {
        throw new DOMException('The operation was aborted.', 'AbortError');
      }
      return jsonResponse(200, {});
    });
    vi.stubGlobal('fetch', fetchMock);

    const client = createApiClient({ baseUrl: '' });
    client.sessionManager.setTokens(validTokens());

    const controller = new AbortController();
    controller.abort();

    const error = await client
      .request('get', '/collections', { signal: controller.signal })
      .catch((e: unknown) => e);
    expect(isAbortError(error)).toBe(true);
    expect(error).not.toBeInstanceOf(NetworkError);
    // Only the one aborted attempt — no refresh call was ever triggered.
    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it('wraps a raw fetch failure as NetworkError rather than an unhandled rejection', async () => {
    const fetchMock = vi.fn(async () => {
      throw new TypeError('Failed to fetch');
    });
    vi.stubGlobal('fetch', fetchMock);

    const client = createApiClient({ baseUrl: '' });
    client.sessionManager.setTokens(validTokens());

    const error = await client.request('get', '/collections').catch((e: unknown) => e);
    expect(error).toBeInstanceOf(NetworkError);
    expect((error as NetworkError).cause).toBeInstanceOf(TypeError);
  });

  it('substitutes path params and serializes query params', async () => {
    const fetchMock = vi.fn(async (input: string | URL) => {
      expect(String(input)).toBe('/api/v1/collections/col-1/documents?limit=10');
      return jsonResponse(200, { items: [], page: { hasMore: false } });
    });
    vi.stubGlobal('fetch', fetchMock);

    const client = createApiClient({ baseUrl: '' });
    client.sessionManager.setTokens(validTokens());

    await client.request('get', '/collections/{collectionId}/documents', {
      params: { path: { collectionId: 'col-1' }, query: { limit: 10 } },
    });
  });
});

describe('login()', () => {
  it('installs the returned tokens so a subsequent request is authenticated with them', async () => {
    const fetchMock = vi.fn(async (input: string | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === '/api/v1/auth/login') {
        return jsonResponse(200, validTokens({ accessToken: 'from-login' }));
      }
      if (url === '/api/v1/auth/me') {
        if (bearerOf(init) === 'Bearer from-login') {
          return jsonResponse(200, {
            userId: 'u',
            orgId: 'o',
            email: 'a@b.com',
            displayName: 'A',
            permissions: [],
            allowedCollectionIds: [],
            sessionId: 's',
          });
        }
        return jsonResponse(401, { code: 'unauthorized', message: 'no', requestId: 'r' });
      }
      throw new Error(`unexpected url ${url}`);
    });
    vi.stubGlobal('fetch', fetchMock);

    const client = createApiClient({ baseUrl: '' });
    const tokens = await client.login({ email: 'a@b.com', password: 'secret' });
    expect(tokens.accessToken).toBe('from-login');

    const me = await client.me();
    expect(me.email).toBe('a@b.com');
  });
});

describe('logout()', () => {
  it('skips the network call and resolves when there is no session', async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal('fetch', fetchMock);

    const client = createApiClient({ baseUrl: '' });
    await expect(client.logout()).resolves.toBeUndefined();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it('always clears the local session, even when the server call fails', async () => {
    const fetchMock = vi.fn(async (input: string | URL) => {
      if (String(input) === '/api/v1/auth/logout') {
        throw new TypeError('Failed to fetch');
      }
      throw new Error('unexpected call');
    });
    vi.stubGlobal('fetch', fetchMock);

    const client = createApiClient({ baseUrl: '' });
    client.sessionManager.setTokens(validTokens());

    await expect(client.logout()).rejects.toBeInstanceOf(NetworkError);
    expect(client.sessionManager.hasSession()).toBe(false);
  });

  it('clears the session on a successful 204 and does not call it again for a second logout', async () => {
    const fetchMock = vi.fn(async () => noContentResponse());
    vi.stubGlobal('fetch', fetchMock);

    const client = createApiClient({ baseUrl: '' });
    client.sessionManager.setTokens(validTokens());

    await client.logout();
    expect(client.sessionManager.hasSession()).toBe(false);
    expect(fetchMock).toHaveBeenCalledTimes(1);

    await client.logout();
    expect(fetchMock).toHaveBeenCalledTimes(1); // no refresh token left, no second network call
  });
});
