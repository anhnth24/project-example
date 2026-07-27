// Thin wrappers around the three mutations `DocumentRowActions.tsx` offers,
// kept out of that file so the component only orchestrates UI state and
// never touches `apiClient`/`fetch` directly.
//
// Download is the odd one out: it is a two-step flow —
//   1. `POST .../download-capability` (typed, JSON) via `apiClient.request`.
//   2. `GET /downloads/{capability}` — deliberately NOT covered by
//      `apiClient.request` (see that file's module doc: the response is
//      `text/markdown` or `application/octet-stream`, never JSON), so this
//      is a raw `fetch` against the same `/api/v1` prefix the rest of the
//      app's non-`apiClient.request` calls use (see `api/sse.ts`'s
//      `SseRequestDescriptor.url` examples, e.g. `/api/v1/jobs/{id}/events`
//      — this file follows that same convention rather than reaching into
//      `client.ts`'s private `baseUrl`/`API_PREFIX`, which aren't exported).
//
// The capability is single-use server-side: this function performs the
// issue+redeem pair exactly once per call and never retries the redeem step
// itself (a 401 here is surfaced as an error, not silently retried — see
// `client.ts`'s own module doc for why blind retry is unsafe for a
// single-use token). Call-site de-duplication against React re-invoking an
// effect twice is `useSingleFlightAction.ts`'s job, not this file's.
import type { ApiClient } from '../../api/client';
import { isAbortError, NetworkError, normalizeErrorResponse } from '../../api/errors';
import { markdownFilenameFor, triggerBrowserDownload } from './saveBlob';

export type DownloadPurpose = 'markdown' | 'original';

const API_PREFIX = '/api/v1';

/**
 * Same base-url resolution `upload/jobEventsSource.ts` uses, and for the same
 * reason: `client.ts` keeps its `baseUrl`/`API_PREFIX` private, and the raw
 * redeem below must still reach the API origin rather than the SPA's own when
 * `VITE_MARKHAND_API_BASE_URL` is configured. Empty (same-origin) in tests.
 */
function apiBaseUrl(): string {
  const configured = import.meta.env.VITE_MARKHAND_API_BASE_URL as string | undefined;
  return configured?.replace(/\/$/, '') ?? '';
}

export interface DownloadDocumentParams {
  client: ApiClient;
  documentId: string;
  versionId: string;
  purpose: DownloadPurpose;
  /** `Document.title` — the original uploaded filename, used to derive a sensible saved filename for either purpose. */
  title: string;
  signal: AbortSignal;
}

/** Issues a single-use download capability and immediately redeems it, then hands the bytes to the browser to save. Never called twice for the same click — see `useSingleFlightAction.ts`. */
export async function downloadDocumentVersion(params: DownloadDocumentParams): Promise<void> {
  const { client, documentId, versionId, purpose, title, signal } = params;

  const issued = await client.request(
    'post',
    '/documents/{documentId}/versions/{versionId}/download-capability',
    { params: { path: { documentId, versionId } }, body: { purpose }, signal },
  );

  const token = await client.tokenProvider.getAccessToken();
  if (signal.aborted) throw new DOMException('Aborted', 'AbortError');

  let response: Response;
  try {
    const url = `${apiBaseUrl()}${API_PREFIX}/downloads/${encodeURIComponent(issued.capability)}`;
    response = await fetch(url, {
      method: 'GET',
      headers: { Authorization: `Bearer ${token}` },
      signal,
    });
  } catch (cause) {
    if (isAbortError(cause)) throw cause;
    throw new NetworkError('Network request failed while downloading.', { cause });
  }

  if (!response.ok) {
    throw await normalizeErrorResponse(response);
  }

  const blob = await response.blob();
  const filename = purpose === 'markdown' ? markdownFilenameFor(title) : title;
  triggerBrowserDownload(blob, filename);
}

export interface ReindexDocumentParams {
  client: ApiClient;
  documentId: string;
  signal: AbortSignal;
}

export interface ReindexOutcome {
  jobId: string;
  /** `false` when this call replayed an already-enqueued job instead of starting a new one. */
  created: boolean;
}

/** `POST /documents/{documentId}/reindex`. Idempotent server-side: a replay returns the same `jobId` with `created: false`. */
export async function requestReindex(params: ReindexDocumentParams): Promise<ReindexOutcome> {
  const { client, documentId, signal } = params;
  const result = await client.request('post', '/documents/{documentId}/reindex', {
    params: { path: { documentId } },
    signal,
  });
  return { jobId: result.jobId, created: result.created };
}

export interface DeleteDocumentParams {
  client: ApiClient;
  documentId: string;
  signal: AbortSignal;
}

/** `DELETE /documents/{documentId}`. A tombstone request (204) — deletion itself completes asynchronously server-side. */
export async function requestDelete(params: DeleteDocumentParams): Promise<void> {
  const { client, documentId, signal } = params;
  await client.request('delete', '/documents/{documentId}', {
    params: { path: { documentId } },
    signal,
  });
}
