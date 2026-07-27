// Real multipart upload with real progress. `fetch()` has no
// `upload.onprogress` equivalent (the `ReadableStream` request-body-streaming
// path that would give one is not implemented in any current browser for
// same-origin form uploads), so this is the one place in the SPA that talks
// to the network via `XMLHttpRequest` instead of `api/client.ts`'s fetch
// wrapper — exactly what the P2-08 brief calls out ("`api/client.ts`...
// deliberately does not cover multipart... you will need your own `fetch`
// for the upload itself; real progress needs `XMLHttpRequest`").
//
// Mirrors `api/client.ts`'s conventions where it can:
//   - same `/api/v1` prefix + `VITE_MARKHAND_API_BASE_URL` base;
//   - bearer token from the shared `TokenProvider` seam, one refresh-and-retry
//     on a single 401 (matching `authedFetch`'s "retry exactly once" policy),
//     never reading a token from storage directly;
//   - abortable.
// Does not reuse `normalizeErrorResponse`/`HttpApiError` from `api/errors.ts`
// because those operate on a `fetch` `Response`, which `XMLHttpRequest` never
// produces — the response-interpretation logic below is this file's own,
// deliberately small, parallel version for the one XHR response shape it has
// to handle (the `createUpload` operation).
import type { TokenProvider } from '../../api/client';
import type { ApiErrorBody, UploadOutcome, UploadProgress, UploadSuccessBody } from './types';

const API_PREFIX = '/api/v1';

function apiBaseUrl(): string {
  const configured = import.meta.env.VITE_MARKHAND_API_BASE_URL as string | undefined;
  return configured?.replace(/\/$/, '') ?? '';
}

/** `POST /uploads` absolute path — same base-url/prefix convention as `api/client.ts`. */
export function uploadUrl(): string {
  return `${apiBaseUrl()}${API_PREFIX}/uploads`;
}

function isApiErrorBody(value: unknown): value is ApiErrorBody {
  return (
    typeof value === 'object' &&
    value !== null &&
    typeof (value as Record<string, unknown>).code === 'string' &&
    typeof (value as Record<string, unknown>).message === 'string' &&
    typeof (value as Record<string, unknown>).requestId === 'string'
  );
}

function parseJsonBody(text: string): unknown {
  if (text.length === 0) return null;
  try {
    return JSON.parse(text);
  } catch {
    return null;
  }
}

function parseErrorBody(xhr: XMLHttpRequest): ApiErrorBody | null {
  const parsed = parseJsonBody(xhr.responseText);
  return isApiErrorBody(parsed) ? parsed : null;
}

function parseRetryAfter(xhr: XMLHttpRequest): number | undefined {
  const raw = xhr.getResponseHeader('Retry-After');
  if (raw === null) return undefined;
  const seconds = Number.parseInt(raw, 10);
  return Number.isFinite(seconds) && seconds >= 0 ? seconds : undefined;
}

/** Interprets a completed (non-401, non-network-error, non-abort) XHR response into an `UploadOutcome`. */
function interpretResponse(xhr: XMLHttpRequest): UploadOutcome {
  if (xhr.status === 201) {
    const parsed = parseJsonBody(xhr.responseText);
    return { kind: 'success', body: parsed as UploadSuccessBody };
  }
  const error = parseErrorBody(xhr);
  switch (xhr.status) {
    case 409:
      return { kind: 'conflict', error };
    case 413:
      return { kind: 'too-large', error };
    case 429:
      return {
        kind: 'quota',
        error,
        rateLimit: { retryAfterSeconds: parseRetryAfter(xhr), details: error?.details },
      };
    case 403:
      return { kind: 'forbidden', error };
    default:
      return { kind: 'http-error', status: xhr.status, error };
  }
}

export interface StartUploadOptions {
  file: File;
  collectionId: string;
  tokenProvider: TokenProvider;
  onProgress: (progress: UploadProgress) => void;
  /** Overridable for tests; defaults to the real `XMLHttpRequest`. */
  xhrFactory?: () => XMLHttpRequest;
}

export interface StartedUpload {
  readonly promise: Promise<UploadOutcome>;
  /** Aborts the in-flight request. Safe to call after it has already settled (no-op) or before the token fetch that precedes `send()` has resolved. */
  abort(): void;
}

/** Kicks off the multipart upload immediately; call `.abort()` to cancel. Resolves exactly once, never rejects (every failure mode is a variant of `UploadOutcome`). */
export function startMultipartUpload(options: StartUploadOptions): StartedUpload {
  const xhr = (options.xhrFactory ?? (() => new XMLHttpRequest()))();
  let settled = false;
  let abortRequested = false;
  let resolveOutcome!: (outcome: UploadOutcome) => void;
  const promise = new Promise<UploadOutcome>((resolve) => {
    resolveOutcome = resolve;
  });

  function finish(outcome: UploadOutcome): void {
    if (settled) return;
    settled = true;
    resolveOutcome(outcome);
  }

  function send(attempt: 1 | 2): void {
    const tokenPromise =
      attempt === 1 ? options.tokenProvider.getAccessToken() : options.tokenProvider.refreshNow();
    void tokenPromise.then(
      (token) => {
        if (settled) return;
        if (abortRequested) {
          finish({ kind: 'aborted' });
          return;
        }
        const form = new FormData();
        form.append('collectionId', options.collectionId);
        form.append('file', options.file, options.file.name);

        xhr.open('POST', uploadUrl());
        xhr.setRequestHeader('Accept', 'application/json');
        xhr.setRequestHeader('Authorization', `Bearer ${token}`);
        xhr.upload.onprogress = (event: ProgressEvent) => {
          options.onProgress({
            loaded: event.loaded,
            total: event.lengthComputable ? event.total : undefined,
          });
        };
        xhr.onerror = () => finish({ kind: 'network-error' });
        xhr.onabort = () => finish({ kind: 'aborted' });
        xhr.onload = () => {
          if (xhr.status === 401 && attempt === 1) {
            send(2);
            return;
          }
          if (xhr.status === 401) {
            finish({ kind: 'session-lost' });
            return;
          }
          finish(interpretResponse(xhr));
        };
        xhr.send(form);
      },
      () => finish({ kind: 'session-lost' }), // getAccessToken()/refreshNow() rejected — no valid session to upload with
    );
  }

  send(1);

  return {
    promise,
    abort() {
      if (settled) return;
      abortRequested = true;
      xhr.abort(); // no-op if `send()` hasn't reached `xhr.send()` yet; `abortRequested` covers that window (see `send()` above)
    },
  };
}
