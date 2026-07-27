// Thin seam between `UploadItemRow` and `api/sse.ts`'s `SseConnection`, for
// one reason: `mocks/**` is deliberately fetch-only and never mocks SSE (see
// `mocks/fetchMock.ts`'s `DELIBERATELY_UNMOCKED_OPERATIONS` handling for
// `jobEvents`), so tests need a seam to substitute a fake abortable stream
// (the same pattern `hooks/useScopeSafeSse.test.tsx` uses) without
// `UploadItemRow` constructing `SseConnection` inline.
import type { TokenProvider } from '../../api/client';
import { SseConnection, type SseMessage } from '../../api/sse';
import type { ScopeSafeSseSource } from '../../hooks/useScopeSafeSse';

const API_PREFIX = '/api/v1';

function apiBaseUrl(): string {
  const configured = import.meta.env.VITE_MARKHAND_API_BASE_URL as string | undefined;
  return configured?.replace(/\/$/, '') ?? '';
}

/** `GET /jobs/{jobId}/events` absolute path — same base-url/prefix convention as `api/client.ts`. */
export function buildJobEventsUrl(jobId: string): string {
  return `${apiBaseUrl()}${API_PREFIX}/jobs/${encodeURIComponent(jobId)}/events`;
}

/** A resumable, bearer-authenticated SSE stream of `jobId`'s events, ready to hand to `useScopeSafeSse`. */
export function createJobEventsSource(
  jobId: string,
  tokenProvider: TokenProvider,
): ScopeSafeSseSource<SseMessage> {
  return new SseConnection(() => ({ url: buildJobEventsUrl(jobId), method: 'GET' }), {
    tokenProvider,
  });
}
