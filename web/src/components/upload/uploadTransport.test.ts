import { describe, expect, it, vi } from 'vitest';
import type { TokenProvider } from '../../api/client';
import { startMultipartUpload, uploadUrl } from './uploadTransport';

/**
 * A hand-rolled `XMLHttpRequest` test double. `mocks/**` is deliberately
 * fetch-level only (see `mocks/fetchMock.ts`), and `fetch` cannot report
 * upload progress in the first place, so nothing in `mocks/**` can stand in
 * for the transport this file tests — this fake is the only way to drive
 * `xhr.upload.onprogress`, a mid-flight `abort()`, and non-2xx/network
 * outcomes deterministically.
 */
class FakeXhr {
  method = '';
  url = '';
  readonly requestHeaders: Record<string, string> = {};
  status = 0;
  responseText = '';
  private responseHeaders: Record<string, string> = {};
  sentBody: FormData | null = null;
  aborted = false;
  upload: { onprogress: ((event: ProgressEvent) => void) | null } = { onprogress: null };
  onload: (() => void) | null = null;
  onerror: (() => void) | null = null;
  onabort: (() => void) | null = null;

  open(method: string, url: string): void {
    this.method = method;
    this.url = url;
  }

  setRequestHeader(name: string, value: string): void {
    this.requestHeaders[name] = value;
  }

  send(body: FormData): void {
    this.sentBody = body;
  }

  abort(): void {
    this.aborted = true;
    this.onabort?.();
  }

  getResponseHeader(name: string): string | null {
    return this.responseHeaders[name] ?? null;
  }

  // --- test helpers, not part of the real XHR surface ---

  respond(status: number, body: unknown, headers: Record<string, string> = {}): void {
    this.status = status;
    this.responseText = typeof body === 'string' ? body : JSON.stringify(body);
    this.responseHeaders = headers;
    this.onload?.();
  }

  progress(loaded: number, total: number, lengthComputable = true): void {
    this.upload.onprogress?.({ loaded, total, lengthComputable } as ProgressEvent);
  }

  networkError(): void {
    this.onerror?.();
  }
}

function fakeTokenProvider(overrides: Partial<TokenProvider> = {}): TokenProvider {
  return {
    getAccessToken: vi.fn().mockResolvedValue('token-1'),
    refreshNow: vi.fn().mockResolvedValue('token-2'),
    onSessionLost: vi.fn(() => () => {}),
    ...overrides,
  };
}

async function flush(times = 3): Promise<void> {
  for (let i = 0; i < times; i += 1) await Promise.resolve();
}

describe('startMultipartUpload', () => {
  it('POSTs to /api/v1/uploads with the file and collectionId, bearer from the token provider', async () => {
    let created: FakeXhr | undefined;
    const tokenProvider = fakeTokenProvider();
    const file = new File(['hello'], 'a.pdf', { type: 'application/pdf' });

    startMultipartUpload({
      file,
      collectionId: 'col-1',
      tokenProvider,
      onProgress: () => {},
      xhrFactory: () => {
        created = new FakeXhr();
        return created as unknown as XMLHttpRequest;
      },
    });
    await flush();

    expect(created).toBeDefined();
    expect(created!.method).toBe('POST');
    expect(created!.url).toBe(uploadUrl());
    expect(created!.requestHeaders.Authorization).toBe('Bearer token-1');
    expect(created!.sentBody!.get('collectionId')).toBe('col-1');
    expect((created!.sentBody!.get('file') as File).name).toBe('a.pdf');
  });

  it('reports real progress as bytes go out, including an indeterminate state when length is not computable', async () => {
    let xhr: FakeXhr | undefined;
    const onProgress = vi.fn();
    startMultipartUpload({
      file: new File(['x'], 'a.pdf'),
      collectionId: 'col-1',
      tokenProvider: fakeTokenProvider(),
      onProgress,
      xhrFactory: () => {
        xhr = new FakeXhr();
        return xhr as unknown as XMLHttpRequest;
      },
    });
    await flush();

    xhr!.progress(50, 200);
    expect(onProgress).toHaveBeenLastCalledWith({ loaded: 50, total: 200 });

    xhr!.progress(120, 200);
    expect(onProgress).toHaveBeenLastCalledWith({ loaded: 120, total: 200 });

    xhr!.progress(120, 0, false);
    expect(onProgress).toHaveBeenLastCalledWith({ loaded: 120, total: undefined });
  });

  it('resolves success with the parsed 201 body', async () => {
    let xhr: FakeXhr | undefined;
    const started = startMultipartUpload({
      file: new File(['x'], 'a.pdf'),
      collectionId: 'col-1',
      tokenProvider: fakeTokenProvider(),
      onProgress: () => {},
      xhrFactory: () => {
        xhr = new FakeXhr();
        return xhr as unknown as XMLHttpRequest;
      },
    });
    await flush();
    xhr!.respond(201, {
      disposition: 'accepted',
      objectId: 'obj-1',
      documentId: 'doc-1',
      versionId: 'ver-1',
      jobId: 'job-1',
      collectionId: 'col-1',
      sha256: 'abc',
      sizeBytes: 1,
      canonicalFormat: 'markdown',
      requestId: 'req-1',
    });

    await expect(started.promise).resolves.toEqual({
      kind: 'success',
      body: expect.objectContaining({ documentId: 'doc-1', jobId: 'job-1' }),
    });
  });

  it.each([
    [409, 'conflict'],
    [413, 'too-large'],
    [403, 'forbidden'],
  ] as const)('maps HTTP %s to outcome kind %s', async (status, kind) => {
    let xhr: FakeXhr | undefined;
    const started = startMultipartUpload({
      file: new File(['x'], 'a.pdf'),
      collectionId: 'col-1',
      tokenProvider: fakeTokenProvider(),
      onProgress: () => {},
      xhrFactory: () => {
        xhr = new FakeXhr();
        return xhr as unknown as XMLHttpRequest;
      },
    });
    await flush();
    xhr!.respond(status, { code: 'x', message: `msg-${status}`, requestId: 'r1' });

    const outcome = await started.promise;
    expect(outcome.kind).toBe(kind);
  });

  it('surfaces 429 quota metadata from the Retry-After header and error details', async () => {
    let xhr: FakeXhr | undefined;
    const started = startMultipartUpload({
      file: new File(['x'], 'a.pdf'),
      collectionId: 'col-1',
      tokenProvider: fakeTokenProvider(),
      onProgress: () => {},
      xhrFactory: () => {
        xhr = new FakeXhr();
        return xhr as unknown as XMLHttpRequest;
      },
    });
    await flush();
    xhr!.respond(
      429,
      {
        code: 'rate_limited',
        message: 'Too many requests.',
        requestId: 'r1',
        details: { scope: 'org' },
      },
      { 'Retry-After': '30' },
    );

    const outcome = await started.promise;
    expect(outcome.kind).toBe('quota');
    if (outcome.kind === 'quota') {
      expect(outcome.rateLimit.retryAfterSeconds).toBe(30);
      expect(outcome.rateLimit.details).toEqual({ scope: 'org' });
    }
  });

  it('resolves network-error on an XHR error event', async () => {
    let xhr: FakeXhr | undefined;
    const started = startMultipartUpload({
      file: new File(['x'], 'a.pdf'),
      collectionId: 'col-1',
      tokenProvider: fakeTokenProvider(),
      onProgress: () => {},
      xhrFactory: () => {
        xhr = new FakeXhr();
        return xhr as unknown as XMLHttpRequest;
      },
    });
    await flush();
    xhr!.networkError();

    await expect(started.promise).resolves.toEqual({ kind: 'network-error' });
  });

  it('cancel() actually aborts the in-flight XHR and resolves aborted', async () => {
    let xhr: FakeXhr | undefined;
    const started = startMultipartUpload({
      file: new File(['x'], 'a.pdf'),
      collectionId: 'col-1',
      tokenProvider: fakeTokenProvider(),
      onProgress: () => {},
      xhrFactory: () => {
        xhr = new FakeXhr();
        return xhr as unknown as XMLHttpRequest;
      },
    });
    await flush();

    started.abort();

    expect(xhr!.aborted).toBe(true);
    await expect(started.promise).resolves.toEqual({ kind: 'aborted' });

    // A late response arriving after abort must not override the outcome.
    xhr!.respond(201, { disposition: 'accepted', documentId: 'doc-1' });
    await expect(started.promise).resolves.toEqual({ kind: 'aborted' });
  });

  it('cancel() before the token fetch resolves still results in aborted, without ever sending the request', async () => {
    let xhr: FakeXhr | undefined;
    let resolveToken!: (token: string) => void;
    const tokenProvider = fakeTokenProvider({
      getAccessToken: vi.fn(
        () =>
          new Promise<string>((resolve) => {
            resolveToken = resolve;
          }),
      ),
    });
    const started = startMultipartUpload({
      file: new File(['x'], 'a.pdf'),
      collectionId: 'col-1',
      tokenProvider,
      onProgress: () => {},
      xhrFactory: () => {
        xhr = new FakeXhr();
        return xhr as unknown as XMLHttpRequest;
      },
    });

    started.abort();
    resolveToken('token-1');
    await flush();

    expect(xhr!.sentBody).toBeNull(); // never actually sent
    await expect(started.promise).resolves.toEqual({ kind: 'aborted' });
  });

  it('retries exactly once via refreshNow() on a 401, reopening the same XHR with the new token', async () => {
    let xhr: FakeXhr | undefined;
    const tokenProvider = fakeTokenProvider();
    const started = startMultipartUpload({
      file: new File(['x'], 'a.pdf'),
      collectionId: 'col-1',
      tokenProvider,
      onProgress: () => {},
      xhrFactory: () => {
        xhr = new FakeXhr();
        return xhr as unknown as XMLHttpRequest;
      },
    });
    await flush();
    expect(xhr!.requestHeaders.Authorization).toBe('Bearer token-1');
    xhr!.respond(401, { code: 'unauthorized', message: 'expired', requestId: 'r1' });
    await flush();

    expect(tokenProvider.refreshNow).toHaveBeenCalledTimes(1);
    expect(xhr!.requestHeaders.Authorization).toBe('Bearer token-2');

    xhr!.respond(201, { disposition: 'accepted', documentId: 'doc-1' });
    await expect(started.promise).resolves.toEqual({
      kind: 'success',
      body: expect.objectContaining({ documentId: 'doc-1' }),
    });
  });

  it('a second consecutive 401 (after the one retry) resolves session-lost, not an infinite retry loop', async () => {
    let xhr: FakeXhr | undefined;
    const tokenProvider = fakeTokenProvider();
    const started = startMultipartUpload({
      file: new File(['x'], 'a.pdf'),
      collectionId: 'col-1',
      tokenProvider,
      onProgress: () => {},
      xhrFactory: () => {
        xhr = new FakeXhr();
        return xhr as unknown as XMLHttpRequest;
      },
    });
    await flush();
    xhr!.respond(401, { code: 'unauthorized', message: 'expired', requestId: 'r1' });
    await flush();
    xhr!.respond(401, { code: 'unauthorized', message: 'expired again', requestId: 'r2' });

    await expect(started.promise).resolves.toEqual({ kind: 'session-lost' });
    expect(tokenProvider.refreshNow).toHaveBeenCalledTimes(1);
  });

  it('resolves session-lost when the token provider itself rejects (no session to upload with)', async () => {
    const started = startMultipartUpload({
      file: new File(['x'], 'a.pdf'),
      collectionId: 'col-1',
      tokenProvider: fakeTokenProvider({
        getAccessToken: vi.fn().mockRejectedValue(new Error('no session')),
      }),
      onProgress: () => {},
      xhrFactory: () => new FakeXhr() as unknown as XMLHttpRequest,
    });

    await expect(started.promise).resolves.toEqual({ kind: 'session-lost' });
  });
});
