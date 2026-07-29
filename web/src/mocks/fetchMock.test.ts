import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { installMockFetch, uninstallMockFetch, resetMockState } from './fetchMock';
import { mockControl } from './control';
import { getStore } from './fixtures';

const API_PREFIX = '/api/v1';

async function login(): Promise<{ accessToken: string }> {
  const res = await fetch(`${API_PREFIX}/auth/login`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ email: 'demo@markhand.test', password: 'demo-password' }),
  });
  expect(res.status).toBe(200);
  return (await res.json()) as { accessToken: string };
}

function authed(accessToken: string, init: RequestInit = {}): RequestInit {
  return { ...init, headers: { ...(init.headers ?? {}), Authorization: `Bearer ${accessToken}` } };
}

describe('fetchMock', () => {
  beforeEach(() => {
    installMockFetch();
    resetMockState();
  });

  afterEach(() => {
    uninstallMockFetch();
  });

  it('answers health/live without auth', async () => {
    const res = await fetch(`${API_PREFIX}/health/live`);
    expect(res.status).toBe(200);
    const body = (await res.json()) as { status: string; requestId: string };
    expect(body.status).toBe('ok');
    expect(body.requestId).toMatch(/^[0-9a-f-]{36}$/);
  });

  it('serves the raw openapi document verbatim', async () => {
    const res = await fetch(`${API_PREFIX}/openapi.yaml`);
    expect(res.status).toBe(200);
    expect(res.headers.get('content-type')).toContain('application/yaml');
    const text = await res.text();
    expect(text).toContain('operationId: healthLive');
  });

  it('rejects an auth-required route with no Authorization header (401)', async () => {
    const res = await fetch(`${API_PREFIX}/collections`);
    expect(res.status).toBe(401);
    const body = (await res.json()) as { code: string; requestId: string };
    expect(body.code).toBe('unauthorized');
  });

  it('logs in, then uses the access token on an authenticated route', async () => {
    const { accessToken } = await login();
    const res = await fetch(`${API_PREFIX}/collections`, authed(accessToken));
    expect(res.status).toBe(200);
    const body = (await res.json()) as { items: unknown[]; page: { hasMore: boolean } };
    expect(body.items.length).toBeGreaterThan(0);
    expect(body.page).toEqual({ hasMore: false, nextCursor: null });
  });

  it('rejects bad credentials with 401', async () => {
    const res = await fetch(`${API_PREFIX}/auth/login`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ email: 'demo@markhand.test', password: 'wrong' }),
    });
    expect(res.status).toBe(401);
  });

  it('refreshes and rotates the refresh token (old one stops working)', async () => {
    const loginRes = await fetch(`${API_PREFIX}/auth/login`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ email: 'demo@markhand.test', password: 'demo-password' }),
    });
    const { refreshToken } = (await loginRes.json()) as { refreshToken: string };

    const refreshRes = await fetch(`${API_PREFIX}/auth/refresh`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ refreshToken }),
    });
    expect(refreshRes.status).toBe(200);

    const replay = await fetch(`${API_PREFIX}/auth/refresh`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ refreshToken }),
    });
    expect(replay.status).toBe(401);
  });

  it('logs out idempotently (204 even on replay)', async () => {
    const { refreshToken } = (await (
      await fetch(`${API_PREFIX}/auth/login`, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ email: 'demo@markhand.test', password: 'demo-password' }),
      })
    ).json()) as { refreshToken: string };

    const first = await fetch(`${API_PREFIX}/auth/logout`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ refreshToken }),
    });
    expect(first.status).toBe(204);

    const second = await fetch(`${API_PREFIX}/auth/logout`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ refreshToken }),
    });
    expect(second.status).toBe(204);
  });

  it('returns 404 for an unknown collection', async () => {
    const { accessToken } = await login();
    const res = await fetch(
      `${API_PREFIX}/collections/00000000-0000-0000-0000-000000000000`,
      authed(accessToken),
    );
    expect(res.status).toBe(404);
    const body = (await res.json()) as { code: string };
    expect(body.code).toBe('not_found');
  });

  it('paginates listDocuments with a cursor that round-trips through PageInfo', async () => {
    const { accessToken } = await login();
    const collectionId = getStore().collections[0].id;
    const firstPage = await fetch(
      `${API_PREFIX}/collections/${collectionId}/documents?limit=1`,
      authed(accessToken),
    );
    expect(firstPage.status).toBe(200);
    const firstBody = (await firstPage.json()) as {
      items: { id: string }[];
      page: { hasMore: boolean; nextCursor: string | null };
    };
    expect(firstBody.items).toHaveLength(1);
    expect(firstBody.page.hasMore).toBe(true);
    expect(firstBody.page.nextCursor).toBeTruthy();

    const secondPage = await fetch(
      `${API_PREFIX}/collections/${collectionId}/documents?limit=1&cursor=${encodeURIComponent(firstBody.page.nextCursor!)}`,
      authed(accessToken),
    );
    const secondBody = (await secondPage.json()) as {
      items: { id: string }[];
      page: { hasMore: boolean };
    };
    expect(secondBody.items).toHaveLength(1);
    expect(secondBody.items[0].id).not.toBe(firstBody.items[0].id);
  });

  it('creates a collection end-to-end and can fetch it back', async () => {
    const { accessToken } = await login();
    const createRes = await fetch(
      `${API_PREFIX}/collections`,
      authed(accessToken, {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ name: 'New Collection', slug: 'new-collection' }),
      }),
    );
    expect(createRes.status).toBe(201);
    const created = (await createRes.json()) as { id: string };

    const getRes = await fetch(`${API_PREFIX}/collections/${created.id}`, authed(accessToken));
    expect(getRes.status).toBe(200);
    const fetched = (await getRes.json()) as { name: string };
    expect(fetched.name).toBe('New Collection');
  });

  describe('failure injection', () => {
    it('forces a 429 with quota metadata and a Retry-After header', async () => {
      const { accessToken } = await login();
      mockControl.forceStatus('listCollections', 429, {
        times: 1,
        quota: { limit: 10, remaining: 0 },
      });
      const res = await fetch(`${API_PREFIX}/collections`, authed(accessToken));
      expect(res.status).toBe(429);
      expect(res.headers.get('Retry-After')).toBeTruthy();
      const body = (await res.json()) as {
        code: string;
        details: { limit: number; remaining: number; resetAt: string };
      };
      expect(body.code).toBe('rate_limited');
      expect(body.details).toMatchObject({ limit: 10, remaining: 0 });

      // Only the next `times` calls are forced; it self-clears after.
      const followUp = await fetch(`${API_PREFIX}/collections`, authed(accessToken));
      expect(followUp.status).toBe(200);
    });

    // Each status below is forced on an operation that actually *declares* that
    // status in the spec — the drift-guard (`assertStatusDeclared`) rejects a
    // forced status the operation's contract doesn't allow, so this table is
    // itself a check that these five failure modes are genuinely reachable.
    it('forces 401 on demand (authMe declares 401)', async () => {
      const { accessToken } = await login();
      mockControl.forceStatus('authMe', 401, { times: 1 });
      const res = await fetch(`${API_PREFIX}/auth/me`, authed(accessToken));
      expect(res.status).toBe(401);
    });

    it('forces 403 on demand (createCollection declares 403)', async () => {
      const { accessToken } = await login();
      mockControl.forceStatus('createCollection', 403, { times: 1 });
      const res = await fetch(
        `${API_PREFIX}/collections`,
        authed(accessToken, {
          method: 'POST',
          headers: { 'content-type': 'application/json' },
          body: JSON.stringify({ name: 'x', slug: 'x' }),
        }),
      );
      expect(res.status).toBe(403);
    });

    it('forces 404 on demand (getJob declares 404)', async () => {
      const { accessToken } = await login();
      mockControl.forceStatus('getJob', 404, { times: 1 });
      const res = await fetch(
        `${API_PREFIX}/jobs/${getStore().jobs.keys().next().value}`,
        authed(accessToken),
      );
      expect(res.status).toBe(404);
    });

    it('produces a real (not forced) 409 for a document mid-conversion (createUpload)', async () => {
      const { accessToken } = await login();
      const collectionId = getStore().collections[0].id;
      const inFlightDocument = (getStore().documents.get(collectionId) ?? []).find(
        (d) => d.state !== 'indexed',
      );
      expect(inFlightDocument).toBeDefined();
      const form = new FormData();
      form.set('file', new File(['content'], 'revision.txt'));
      form.set('collectionId', collectionId);
      form.set('documentId', inFlightDocument!.id);
      const res = await fetch(
        `${API_PREFIX}/uploads`,
        authed(accessToken, { method: 'POST', body: form }),
      );
      expect(res.status).toBe(409);
      const body = (await res.json()) as { code: string };
      expect(body.code).toBe('conflict');
    });

    it('forces 503 on demand (healthReady declares 503, no auth required)', async () => {
      mockControl.forceStatus('healthReady', 503, { times: 1 });
      const res = await fetch(`${API_PREFIX}/health/ready`);
      expect(res.status).toBe(503);
      const followUp = await fetch(`${API_PREFIX}/health/ready`);
      expect(followUp.status).toBe(200);
    });
  });

  it('throws a clear error for the still-unmocked SSE streaming endpoint instead of returning wrong data', async () => {
    // `askStream` moved out of `DELIBERATELY_UNMOCKED_OPERATIONS`'s effective
    // set in P2-10 — `mocks/handlers/qa.ts` registers a real handler for it
    // (a pre-serialized `text/event-stream` body, since every event it will
    // ever emit is already decided before the response is built — see that
    // file's module doc), so it's no longer this test's example. `jobEvents`
    // remains genuinely unmocked (a real job's live progress cannot be
    // pre-decided the same way), so it's the one still exercised here.
    const { accessToken } = await login();
    const jobId = getStore().jobs.keys().next().value;
    await expect(fetch(`${API_PREFIX}/jobs/${jobId}/events`, authed(accessToken))).rejects.toThrow(
      /deliberately not mocked/,
    );
  });
});
