// Fetch-based SSE transport for Markhand's two resumable streams:
// `GET /api/v1/jobs/{jobId}/events` (crates/server/src/routes/events.rs) and
// `POST /api/v1/ask/stream` (crates/server/src/routes/ask.rs). Native
// `EventSource` cannot send a bearer header, so this parses the
// `text/event-stream` body of a `fetch()` response by hand.
//
// This module is deliberately endpoint-agnostic: it owns the wire protocol
// (framing, cursor, refresh-on-401, reconnect, revocation, backpressure,
// cancellation) and takes a `SseRequestFactory` from the caller for
// everything endpoint-specific (URL, body, and — for `/ask/stream` — folding
// a `streamSessionId` learned from the first `ask.started` event into later
// reconnects). See the constructor doc for the exact seam.
//
// Server behaviour this file encodes, with citations:
//
// - Envelope shape `{version, sequence, event, requestId, data}`
//   (camelCase on the wire): `crates/server/src/api/sse.rs:7-13`.
// - The SSE `id:` line carries `envelope.sequence.to_string()`
//   (routes/events.rs:354-360, services/qa/ask_stream.rs:797-803) — exact,
//   unlike the JSON `sequence` field which is a lossy JS `number` after
//   `JSON.parse`. See lastEventId.ts for why cursors are tracked from `id:`.
// - A "control" close frame (`stream.closed` sent by `send_control_closed`,
//   routes/events.rs:332-352 and services/qa/ask_stream.rs:759-778) has NO
//   `id:` line at all — "must not advance Last-Event-ID" per the comment at
//   routes/events.rs:344. A *durable* `stream.closed` (ask stream only, via
//   `close_with_terminal`, db/ask_streams.rs:444-505) DOES carry a real `id`
//   and belongs in the normal sequence. This file tells them apart purely by
//   whether the frame carried an `id:` line, matching that invariant
//   literally instead of guessing from the event name.
// - Close reasons and what they mean, from `StreamAuthError::close_reason()`
//   (services/stream_auth.rs:37-49) plus the ask-only/job-only reasons
//   emitted directly (`cancelled`, `session_expired`, `snapshot_complete`,
//   `live_tail_timeout`, `stream_error`, `send_timeout`, `completed`): see
//   `classifySseCloseCode` below for the exact policy taken for each.
// - `/ask/stream` resume-time authorization failure returns HTTP 401 with
//   `ApiError.code` set to the same reason string (`RouteError::StreamClosed`,
//   routes/ask.rs:456-461), while a plain expired/invalid/missing bearer
//   token — at connect OR reconnect — always returns 401 with
//   `code: "unauthorized"` (auth/middleware.rs:108-138), never
//   `"token_expired"`. Both are treated as refreshable; the other
//   `StreamClosed` codes (`session_revoked`, `principal_denied`,
//   `citation_revoked`) are not.

import type { components } from './generated/contract';
import type { TokenProvider } from './session';
import {
  advanceCursor,
  createCursor,
  requestCursorParams,
  type LastEventIdCursor,
} from './lastEventId';

export type { TokenProvider };

/** The wire envelope for a durable event — re-exported so callers don't hand-roll it. */
export type SseEnvelopeLike = components['schemas']['SseEnvelope'];

/**
 * The synthetic "give up" frame (`send_control_closed`). Structurally almost
 * identical to `SseEnvelopeLike` — same field names, camelCase — plus
 * `control: true`, and critically it never arrives with an SSE `id:` (see
 * module doc), which is how `SseConnection` tells the two apart.
 */
export interface SseControlEnvelope {
  readonly version: number;
  readonly event: string;
  readonly requestId: string;
  readonly data: unknown;
  readonly control: true;
}

/** One parsed unit handed to the consumer. Exactly one `kind` per SSE dispatch (plus a synthetic trailing `closed`). */
export type SseMessage =
  | { kind: 'event'; id: string; envelope: SseEnvelopeLike }
  | { kind: 'control'; envelope: SseControlEnvelope }
  | { kind: 'heartbeat' }
  | { kind: 'gap'; expected: LastEventIdCursor; received: LastEventIdCursor }
  | { kind: 'protocol-violation'; message: string; receivedId: string }
  | { kind: 'parse-error'; raw: string; event?: string }
  | { kind: 'closed'; reason: SseCloseReason };

export type SseCloseReason =
  { type: 'server'; code: string } | { type: 'network-error' } | { type: 'session-lost' };

// ---------------------------------------------------------------------------
// Low-level frame parser: bytes/text in, complete SSE dispatches out.
// Handles multi-line `data:`, `event:`, `id:`, comment/heartbeat lines,
// CRLF/LF/CR, and a chunk boundary splitting a frame (or a single line)
// mid-way, by buffering an incomplete trailing line across `push()` calls.
// ---------------------------------------------------------------------------

/** One completed SSE dispatch. `comment` is true for a heartbeat/comment-only block (no `event:`/`data:`/`id:` seen). */
export interface SseLineFrame {
  readonly event?: string;
  readonly data: string;
  readonly id?: string;
  readonly comment: boolean;
}

export class SseFrameParser {
  private carry = '';
  private eventType: string | undefined;
  private dataLines: string[] = [];
  private id: string | undefined;
  private sawComment = false;
  private sawField = false;

  /** Feed a decoded text chunk (already UTF-8 decoded — see `decode({ stream: true })` below); returns any frames completed by it. */
  push(text: string): SseLineFrame[] {
    const combined = this.carry + text;
    const lines = combined.split(/\r\n|\r|\n/);
    // The trailing element is text after the last line break — an
    // incomplete line if `text` didn't end on a break — and must carry over.
    this.carry = lines.pop() ?? '';
    const frames: SseLineFrame[] = [];
    for (const line of lines) {
      const frame = this.consumeLine(line);
      if (frame) {
        frames.push(frame);
      }
    }
    return frames;
  }

  /** Call once when the underlying stream ends, to flush a final frame that arrived without a trailing blank line. */
  flush(): SseLineFrame[] {
    const frames: SseLineFrame[] = [];
    if (this.carry !== '') {
      const tail = this.carry;
      this.carry = '';
      const frame = this.consumeLine(tail);
      if (frame) {
        frames.push(frame);
      }
    }
    const final = this.dispatch();
    if (final) {
      frames.push(final);
    }
    return frames;
  }

  private consumeLine(line: string): SseLineFrame | undefined {
    if (line === '') {
      return this.dispatch();
    }
    if (line.startsWith(':')) {
      this.sawComment = true;
      return undefined;
    }
    const colon = line.indexOf(':');
    const field = colon === -1 ? line : line.slice(0, colon);
    let value = colon === -1 ? '' : line.slice(colon + 1);
    if (value.startsWith(' ')) {
      value = value.slice(1);
    }
    switch (field) {
      case 'event':
        this.eventType = value;
        this.sawField = true;
        break;
      case 'data':
        this.dataLines.push(value);
        this.sawField = true;
        break;
      case 'id':
        this.id = value;
        this.sawField = true;
        break;
      default:
        // Unknown/unused field (e.g. `retry:`) — ignore, don't error.
        break;
    }
    return undefined;
  }

  private dispatch(): SseLineFrame | undefined {
    const hadField = this.sawField;
    const hadComment = this.sawComment;
    if (!hadField && !hadComment) {
      return undefined; // stray blank line — nothing accumulated since the last dispatch
    }
    const frame: SseLineFrame = {
      event: this.eventType,
      data: this.dataLines.join('\n'),
      id: this.id,
      comment: !hadField && hadComment,
    };
    this.eventType = undefined;
    this.dataLines = [];
    this.id = undefined;
    this.sawComment = false;
    this.sawField = false;
    return frame;
  }
}

// ---------------------------------------------------------------------------
// Close-reason policy
// ---------------------------------------------------------------------------

export type SseCloseAction = 'refresh' | 'terminal' | 'retry' | 'complete';

/** `token_expired` (services/stream_auth.rs:40): the token expired mid-stream — refresh once, then resume from Last-Event-ID. */
const REFRESH_CODES: ReadonlySet<string> = new Set(['token_expired']);

/** Durable end-of-stream reasons: `completed` is ask's normal finish (ask_stream.rs:452); `snapshot_complete` is job's (routes/events.rs:161-165, job already terminal, backlog drained). */
const COMPLETE_CODES: ReadonlySet<string> = new Set(['completed', 'snapshot_complete']);

/** Transient producer/server hiccups worth a backoff-and-reconnect, not a hard failure: `stream_error` (DB/timeout), `send_timeout` (reserve() timed out server-side), `live_tail_timeout` (idle-polled out while the job/session was still open). */
const RETRY_CODES: ReadonlySet<string> = new Set([
  'stream_error',
  'send_timeout',
  'live_tail_timeout',
]);

/**
 * Classify a `stream.closed` reason (or a 401 `ApiError.code`) into what the
 * client should do next. Everything not explicitly a refresh/complete/retry
 * code — `session_revoked`, `principal_denied`, `auth_revoked`,
 * `citation_revoked`, `cancelled`, `session_expired`, and any reason this
 * client doesn't recognize — is `terminal`. Treating unknown reasons as
 * terminal rather than retryable is a deliberate fail-closed choice: the
 * "no reconnect storm" and "zero content after revoke" requirements both
 * fail if a future/unrecognized server reason is guessed to be transient.
 */
export function classifySseCloseCode(code: string): SseCloseAction {
  if (REFRESH_CODES.has(code)) return 'refresh';
  if (COMPLETE_CODES.has(code)) return 'complete';
  if (RETRY_CODES.has(code)) return 'retry';
  return 'terminal';
}

function closeReasonOf(envelope: { event: string; data: unknown }): string | undefined {
  if (envelope.event !== 'stream.closed') {
    return undefined;
  }
  const data = envelope.data;
  if (
    typeof data === 'object' &&
    data !== null &&
    typeof (data as Record<string, unknown>).reason === 'string'
  ) {
    return (data as { reason: string }).reason;
  }
  return undefined;
}

function isEnvelopeShaped(
  value: unknown,
): value is { version: number; event: string; requestId: string; data: unknown } {
  if (typeof value !== 'object' || value === null) {
    return false;
  }
  const record = value as Record<string, unknown>;
  return (
    typeof record.event === 'string' && typeof record.requestId === 'string' && 'data' in record
  );
}

async function safeReadJson(response: Response): Promise<Record<string, unknown> | undefined> {
  try {
    const value: unknown = await response.json();
    return typeof value === 'object' && value !== null
      ? (value as Record<string, unknown>)
      : undefined;
  } catch {
    return undefined;
  }
}

/** `Retry-After` on 429 (components.responses.RateLimited, openapi.yaml) is documented as a number of seconds. */
function parseRetryAfterMs(headerValue: string | null): number | undefined {
  if (!headerValue) {
    return undefined;
  }
  const seconds = Number(headerValue);
  return Number.isFinite(seconds) && seconds >= 0 ? seconds * 1000 : undefined;
}

function defaultBackoff(attempt: number): number {
  return Math.min(500 * 2 ** (attempt - 1), 10_000);
}

class AbortWaitError extends Error {}

function delay(ms: number, signal: AbortSignal): Promise<void> {
  if (signal.aborted) {
    return Promise.reject(new AbortWaitError('aborted'));
  }
  if (ms <= 0) {
    return Promise.resolve();
  }
  return new Promise((resolve, reject) => {
    const onAbort = () => {
      clearTimeout(timer);
      reject(new AbortWaitError('aborted'));
    };
    const timer = setTimeout(() => {
      signal.removeEventListener('abort', onAbort);
      resolve();
    }, ms);
    signal.addEventListener('abort', onAbort, { once: true });
  });
}

// ---------------------------------------------------------------------------
// Connection
// ---------------------------------------------------------------------------

export interface SseRequestDescriptor {
  readonly url: string;
  readonly method?: string;
  readonly headers?: Record<string, string>;
  readonly body?: string;
}

/**
 * Builds the request for a (re)connect at `cursor`. Called once per attempt,
 * including every reconnect, so a caller resuming `/ask/stream` can close
 * over the `streamSessionId` it learned from the first `ask.started`
 * event's `envelope.data.streamSessionId` and add it as a query parameter
 * from the second call onward — `SseConnection` itself has no notion of
 * ask/job specifics, only of `lastEventId`/`Last-Event-ID`, which it adds
 * automatically (see `requestCursorParams`).
 */
export type SseRequestFactory = (cursor: LastEventIdCursor) => SseRequestDescriptor;

export interface SseConnectionOptions {
  readonly tokenProvider: TokenProvider;
  /** Defaults to global `fetch`; override in tests. */
  readonly fetchImpl?: typeof fetch;
  /** Resume from this cursor instead of a fresh connect at `0`. */
  readonly initialLastEventId?: string;
  /** External cancellation, merged with the connection's own `abort()`. */
  readonly signal?: AbortSignal;
  /** Backoff schedule for transient retries, keyed by 1-based attempt number. Overridable for deterministic tests. */
  readonly backoff?: (attempt: number) => number;
  /** Give up after this many consecutive transient failures. */
  readonly maxTransientAttempts?: number;
}

type ConsumeOutcome =
  | { action: 'aborted' }
  | { action: 'reconnect' }
  | { action: 'refresh' }
  | { action: 'terminal'; code: string }
  | { action: 'complete'; code: string };

/**
 * A resumable, bearer-authenticated SSE connection over `fetch`.
 *
 * Usage:
 *
 * ```ts
 * const connection = new SseConnection(
 *   (cursor) => ({ url: `/api/v1/jobs/${jobId}/events`, method: 'GET' }),
 *   { tokenProvider },
 * );
 * for await (const message of connection) {
 *   if (message.kind === 'event') { ... }
 * }
 * ```
 *
 * Properties this class is built to satisfy (see the class-level doc for
 * server citations backing each policy):
 *
 * - No acknowledged event lost or repeated across reconnect: the cursor only
 *   ever advances past an `id`-bearing frame once yielded, and every
 *   (re)connect asks `SseRequestFactory` for a fresh descriptor which this
 *   class stamps with that exact cursor as `lastEventId`/`Last-Event-ID`.
 * - Sequence order + gap surfaced: `advanceCursor` (lastEventId.ts) rejects
 *   any non-increasing id as a `protocol-violation` (terminal — never
 *   silently accepted as if it were fine), and reports non-contiguous but
 *   increasing ids as `gap` without deciding for the caller whether that's
 *   alarming (see the long comment on `advanceCursor` — it isn't, for job
 *   streams).
 * - Exactly one refresh + resume per 401/`token_expired`, not a storm:
 *   `refreshedWithoutProgress` allows exactly one `refreshNow()` between
 *   moments of forward progress (any yielded event/heartbeat/control resets
 *   it); a second consecutive auth failure with zero progress in between
 *   goes straight to `closed`.
 * - Zero content after session loss/revoke: a `terminal` classification
 *   (session_revoked/principal_denied/citation_revoked/auth_revoked/
 *   cancelled/session_expired/unknown) ends the generator immediately with
 *   no further reconnect; separately, `tokenProvider.onSessionLost` aborts
 *   the in-flight fetch/reader synchronously, without waiting for the
 *   server to say so.
 * - Bounded backpressure: this is an async generator — `reader.read()` is
 *   only called again once the consumer calls `.next()` (i.e. resumes the
 *   `for await` loop), so an unread stream never grows an internal queue.
 * - Cancellation: `abort()` triggers `AbortController.abort()`, which both
 *   aborts the in-flight `fetch()` (via `signal`) and cancels+releases the
 *   stream reader (`consumeBody`'s `abort` listener calls `reader.cancel()`
 *   before `reader.releaseLock()` in `finally`).
 */
export class SseConnection implements AsyncIterable<SseMessage> {
  private readonly controller = new AbortController();
  private readonly fetchImpl: typeof fetch;
  private readonly backoff: (attempt: number) => number;
  private readonly maxTransientAttempts: number;
  private readonly unsubscribeSessionLost: () => void;
  private cursor: LastEventIdCursor;
  private refreshedWithoutProgress = false;
  private transientAttempt = 0;
  private sessionLostExternally = false;
  private iterator: AsyncGenerator<SseMessage, void, void> | undefined;

  constructor(
    private readonly requestFactory: SseRequestFactory,
    private readonly options: SseConnectionOptions,
  ) {
    this.cursor = createCursor(options.initialLastEventId);
    this.fetchImpl = options.fetchImpl ?? fetch;
    this.backoff = options.backoff ?? defaultBackoff;
    this.maxTransientAttempts = options.maxTransientAttempts ?? 6;
    if (options.signal) {
      if (options.signal.aborted) {
        this.controller.abort();
      } else {
        options.signal.addEventListener('abort', () => this.controller.abort(), { once: true });
      }
    }
    this.unsubscribeSessionLost = options.tokenProvider.onSessionLost(() => {
      this.sessionLostExternally = true;
      this.controller.abort();
    });
  }

  /** Stop the fetch and release the reader. Idempotent. No further messages are yielded except a final `session-lost` close if that's why we stopped. */
  abort(): void {
    this.controller.abort();
  }

  get aborted(): boolean {
    return this.controller.signal.aborted;
  }

  [Symbol.asyncIterator](): AsyncIterator<SseMessage> {
    this.iterator ??= this.driveWithFinalClose();
    return this.iterator;
  }

  private async *driveWithFinalClose(): AsyncGenerator<SseMessage, void, void> {
    try {
      yield* this.run();
    } finally {
      this.unsubscribeSessionLost();
    }
    if (this.sessionLostExternally) {
      yield { kind: 'closed', reason: { type: 'session-lost' } };
    }
  }

  private async *run(): AsyncGenerator<SseMessage, void, void> {
    while (!this.controller.signal.aborted) {
      const descriptor = this.requestFactory(this.cursor);

      let token: string;
      try {
        token = await this.options.tokenProvider.getAccessToken();
      } catch {
        return;
      }
      if (this.controller.signal.aborted) {
        return;
      }

      const headers = new Headers(descriptor.headers);
      headers.set('Accept', 'text/event-stream');
      headers.set('Authorization', `Bearer ${token}`);
      let url = descriptor.url;
      const resume = requestCursorParams(this.cursor);
      if (resume) {
        url += (url.includes('?') ? '&' : '?') + `lastEventId=${resume.query}`;
        headers.set('Last-Event-ID', resume.header);
      }

      let response: Response;
      try {
        response = await this.fetchImpl(url, {
          method: descriptor.method ?? 'GET',
          headers,
          body: descriptor.body,
          signal: this.controller.signal,
        });
      } catch {
        if (this.controller.signal.aborted) {
          return;
        }
        this.transientAttempt += 1;
        if (!(yield* this.stopOrRetry(this.transientAttempt))) return;
        continue;
      }

      if (response.status === 401) {
        const body = await safeReadJson(response);
        const code = typeof body?.code === 'string' ? body.code : 'unauthorized';
        const refreshable = code === 'unauthorized' || classifySseCloseCode(code) === 'refresh';
        if (!refreshable) {
          yield { kind: 'closed', reason: { type: 'server', code } };
          return;
        }
        if (!(yield* this.refreshOrStop(code))) return;
        continue;
      }

      if (!response.ok) {
        if (response.status === 429 || response.status >= 500) {
          const retryAfterMs = parseRetryAfterMs(response.headers.get('retry-after'));
          this.transientAttempt += 1;
          if (!(yield* this.stopOrRetry(this.transientAttempt, retryAfterMs))) return;
          continue;
        }
        const body = await safeReadJson(response);
        const code = typeof body?.code === 'string' ? body.code : String(response.status);
        yield { kind: 'closed', reason: { type: 'server', code } };
        return;
      }

      if (!response.body) {
        this.transientAttempt += 1;
        if (!(yield* this.stopOrRetry(this.transientAttempt))) return;
        continue;
      }

      const outcome = yield* this.consumeBody(response.body);
      switch (outcome.action) {
        case 'aborted':
          return;
        case 'reconnect':
          this.transientAttempt += 1;
          if (!(yield* this.stopOrRetry(this.transientAttempt))) return;
          continue;
        case 'refresh':
          if (!(yield* this.refreshOrStop('token_expired'))) return;
          continue;
        case 'terminal':
          yield { kind: 'closed', reason: { type: 'server', code: outcome.code } };
          return;
        case 'complete':
          yield { kind: 'closed', reason: { type: 'server', code: outcome.code } };
          return;
      }
    }
  }

  /** Single-flight refresh-then-resume, guarded against a storm: refuses a second refresh attempt without intervening forward progress. */
  private async *refreshOrStop(code: string): AsyncGenerator<SseMessage, boolean, void> {
    if (this.refreshedWithoutProgress) {
      yield { kind: 'closed', reason: { type: 'server', code } };
      return false;
    }
    this.refreshedWithoutProgress = true;
    try {
      await this.options.tokenProvider.refreshNow();
    } catch {
      return false;
    }
    return true;
  }

  private async retryOrStop(attempt: number, retryAfterMs?: number): Promise<boolean> {
    if (attempt > this.maxTransientAttempts) {
      return false;
    }
    try {
      await delay(retryAfterMs ?? this.backoff(attempt), this.controller.signal);
      return true;
    } catch {
      return false;
    }
  }

  private async *stopOrRetry(
    attempt: number,
    retryAfterMs?: number,
  ): AsyncGenerator<SseMessage, boolean, void> {
    if (await this.retryOrStop(attempt, retryAfterMs)) {
      return true;
    }
    if (!this.controller.signal.aborted) {
      yield { kind: 'closed', reason: { type: 'network-error' } };
    }
    return false;
  }

  private async *consumeBody(
    body: ReadableStream<Uint8Array>,
  ): AsyncGenerator<SseMessage, ConsumeOutcome, void> {
    const reader = body.getReader();
    const decoder = new TextDecoder(); // one persistent decoder: `{ stream: true }` below carries a
    // split multi-byte UTF-8 sequence across chunk boundaries correctly —
    // important for Vietnamese text, which is full of multi-byte codepoints.
    const parser = new SseFrameParser();
    const cancelOnAbort = () => {
      reader.cancel().catch(() => {});
    };
    this.controller.signal.addEventListener('abort', cancelOnAbort, { once: true });
    try {
      while (true) {
        let step: ReadableStreamReadResult<Uint8Array>;
        try {
          step = await reader.read();
        } catch {
          return this.controller.signal.aborted ? { action: 'aborted' } : { action: 'reconnect' };
        }
        if (this.controller.signal.aborted) {
          return { action: 'aborted' };
        }
        if (step.done) {
          const outcome = yield* this.processFrames(parser.flush());
          return outcome ?? { action: 'reconnect' };
        }
        const text = decoder.decode(step.value, { stream: true });
        const outcome = yield* this.processFrames(parser.push(text));
        if (outcome) {
          return outcome;
        }
      }
    } finally {
      this.controller.signal.removeEventListener('abort', cancelOnAbort);
      reader.releaseLock();
    }
  }

  private *processFrames(
    frames: SseLineFrame[],
  ): Generator<SseMessage, ConsumeOutcome | undefined, void> {
    for (const frame of frames) {
      this.transientAttempt = 0; // bytes arrived: the connection is alive and authenticated
      for (const message of this.toMessages(frame)) {
        if (
          message.kind === 'event' ||
          message.kind === 'heartbeat' ||
          message.kind === 'control'
        ) {
          this.refreshedWithoutProgress = false;
        }
        yield message;
        const outcome = this.closeOutcomeFor(message);
        if (outcome) {
          return outcome;
        }
      }
    }
    return undefined;
  }

  private toMessages(frame: SseLineFrame): SseMessage[] {
    if (frame.comment) {
      return [{ kind: 'heartbeat' }];
    }
    let parsed: unknown;
    try {
      parsed = JSON.parse(frame.data);
    } catch {
      return [{ kind: 'parse-error', raw: frame.data, event: frame.event }];
    }
    if (!isEnvelopeShaped(parsed)) {
      return [{ kind: 'parse-error', raw: frame.data, event: frame.event }];
    }

    if (frame.id !== undefined) {
      // Carries `id:` -> durable, sequence-bearing event (module doc).
      const result = advanceCursor(this.cursor, frame.id);
      if (!result.ok) {
        return [
          {
            kind: 'protocol-violation',
            message: `id "${frame.id}" invalid or not greater than last acked "${this.cursor}" (${result.error})`,
            receivedId: frame.id,
          },
        ];
      }
      const messages: SseMessage[] = [];
      if (!result.contiguous) {
        messages.push({ kind: 'gap', expected: result.expected, received: frame.id });
      }
      this.cursor = result.cursor;
      messages.push({ kind: 'event', id: frame.id, envelope: parsed as SseEnvelopeLike });
      return messages;
    }

    // No `id:` -> synthetic control frame; must not advance the cursor.
    return [
      {
        kind: 'control',
        envelope: { ...(parsed as Record<string, unknown>), control: true } as SseControlEnvelope,
      },
    ];
  }

  private closeOutcomeFor(message: SseMessage): ConsumeOutcome | undefined {
    if (message.kind === 'protocol-violation') {
      // A duplicate/out-of-order id is never legitimate (lastEventId.ts) —
      // fail closed rather than keep accepting a possibly-replayed stream.
      return { action: 'terminal', code: 'protocol_violation' };
    }
    if (message.kind !== 'event' && message.kind !== 'control') {
      return undefined;
    }
    const reason = closeReasonOf(message.envelope);
    if (reason === undefined) {
      return undefined;
    }
    switch (classifySseCloseCode(reason)) {
      case 'refresh':
        return { action: 'refresh' };
      case 'complete':
        return { action: 'complete', code: reason };
      case 'retry':
        return { action: 'reconnect' };
      case 'terminal':
        return { action: 'terminal', code: reason };
    }
  }
}
