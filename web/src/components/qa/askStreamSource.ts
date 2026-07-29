// Thin seam between `ChatTurnBubble` (via `useAskStream`) and `api/sse.ts`'s
// `SseConnection`, building the `POST /ask/stream` request — same shape as
// `components/upload/jobEventsSource.ts`'s `createJobEventsSource` for
// `GET /jobs/{jobId}/events`, minus that file's "mocks never mock SSE" caveat:
// `mocks/handlers/qa.ts` registers `askStream` as a real fetch-mocked
// operation (see that file's module doc for why that's safe here even though
// `jobEvents` still isn't), so this reaches the mock the same way it reaches
// a real deployment — no test-only substitution needed for the happy path.
import type { components } from '../../api/generated/contract';
import type { TokenProvider } from '../../api/client';
import { SseConnection, type SseMessage } from '../../api/sse';
import type { ScopeSafeSseSource } from '../../hooks/useScopeSafeSse';

type AskRequest = components['schemas']['AskRequest'];

const API_PREFIX = '/api/v1';

function apiBaseUrl(): string {
  const configured = import.meta.env.VITE_MARKHAND_API_BASE_URL as string | undefined;
  return configured?.replace(/\/$/, '') ?? '';
}

export function buildAskStreamUrl(): string {
  return `${apiBaseUrl()}${API_PREFIX}/ask/stream`;
}

/**
 * A resumable, bearer-authenticated `/ask/stream` connection, ready to hand
 * to `useScopeSafeSse`. `request` is sent as the POST body on the first
 * connect; per `SseRequestFactory`'s own doc (`api/sse.ts`), the *same*
 * descriptor is asked for again on every reconnect, so once the first
 * `ask.started` event reports a `streamSessionId` (via `onStreamSessionId`,
 * called by the caller's message handler), later (re)connects can resume
 * against the pinned retrieval snapshot instead of asking the server to
 * re-run retrieval/provider — `getStreamSessionId` closes over whatever the
 * caller has learned by the time each connect attempt happens.
 */
export function createAskStreamSource(
  request: AskRequest,
  tokenProvider: TokenProvider,
  getStreamSessionId: () => string | undefined,
): ScopeSafeSseSource<SseMessage> {
  const body = JSON.stringify(request);
  return new SseConnection(
    () => {
      const streamSessionId = getStreamSessionId();
      const url = streamSessionId
        ? `${buildAskStreamUrl()}?streamSessionId=${encodeURIComponent(streamSessionId)}`
        : buildAskStreamUrl();
      return {
        url,
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body,
      };
    },
    { tokenProvider },
  );
}
