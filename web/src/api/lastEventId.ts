// Client-side Last-Event-ID cursor: parsing, formatting and gap detection for
// resumable SSE reconnect. Mirrors the parsing rules in
// `crates/server/src/api/last_event_id.rs` (`parse_last_event_id_token`,
// `resolve_last_event_id`) so a client-generated cursor can never be rejected
// by the server for a reason the client itself couldn't have predicted.
//
// Sequence numbers are transported as a decimal string in the SSE `id:` line
// (see `sse_event()` in `crates/server/src/routes/events.rs:354-360` and
// `crates/server/src/services/qa/ask_stream.rs:797-803`, both doing
// `.id(envelope.sequence.to_string())`), NOT as the JSON `sequence` field
// inside `data:` (that field round-trips through `serde_json`/`JSON.parse` as
// a JS `number`, which loses precision above 2**53). We therefore track and
// compare cursors as arbitrary-precision decimal strings via `BigInt`, never
// via `Number`, so a session that runs long enough to exceed 2**53 events
// still resumes correctly.

/** Server-side `i64` upper bound accepted by `parse_last_event_id_token` (last_event_id.rs:38-42). */
const I64_MAX = 9223372036854775807n;

export type LastEventIdErrorKind = 'malformed' | 'negative' | 'out_of_range';

export type ParseLastEventIdResult =
  { ok: true; value: bigint } | { ok: false; error: LastEventIdErrorKind };

/**
 * Parse one cursor token into `0..=i64::MAX`, mirroring
 * `parse_last_event_id_token` (last_event_id.rs:26-43) byte for byte: trim,
 * reject empty/non-digit/negative, reject values that overflow `i64`.
 */
export function parseLastEventIdToken(raw: string): ParseLastEventIdResult {
  const trimmed = raw.trim();
  if (trimmed.length === 0) {
    return { ok: false, error: 'malformed' };
  }
  if (trimmed.startsWith('-')) {
    return { ok: false, error: 'negative' };
  }
  if (!/^[0-9]+$/.test(trimmed)) {
    return { ok: false, error: 'malformed' };
  }
  const value = BigInt(trimmed);
  if (value > I64_MAX) {
    return { ok: false, error: 'out_of_range' };
  }
  return { ok: true, value };
}

export type ResolveLastEventIdError = LastEventIdErrorKind | 'conflicting';

export type ResolveLastEventIdResult =
  { ok: true; value: bigint } | { ok: false; error: ResolveLastEventIdError };

/**
 * Merge an optional `lastEventId` query value and `Last-Event-ID` header
 * value the same way the server does in `resolve_last_event_id`
 * (last_event_id.rs:53-78): both absent -> 0; one present -> that value;
 * both present and equal -> that value; both present and unequal ->
 * `conflicting`. `highWater`, when given, rejects a cursor from the future.
 *
 * Not used internally by `SseConnection` (which always sends matching
 * query/header values), but kept here — and tested against the same fixtures
 * as the server's own unit tests — so this module is a faithful, verifiable
 * model of the resume contract rather than a generic SSE guess.
 */
export function resolveLastEventId(
  query: string | undefined,
  header: string | undefined,
  highWater?: bigint,
): ResolveLastEventIdResult {
  const parsedQuery = query === undefined ? undefined : parseLastEventIdToken(query);
  if (parsedQuery && !parsedQuery.ok) {
    return parsedQuery;
  }
  const parsedHeader = header === undefined ? undefined : parseLastEventIdToken(header);
  if (parsedHeader && !parsedHeader.ok) {
    return parsedHeader;
  }

  let value: bigint;
  if (parsedQuery === undefined && parsedHeader === undefined) {
    value = 0n;
  } else if (parsedHeader === undefined) {
    value = (parsedQuery as { ok: true; value: bigint }).value;
  } else if (parsedQuery === undefined) {
    value = parsedHeader.value;
  } else if (parsedQuery.value === parsedHeader.value) {
    value = parsedQuery.value;
  } else {
    return { ok: false, error: 'conflicting' };
  }

  if (highWater !== undefined && value > highWater) {
    return { ok: false, error: 'out_of_range' };
  }
  return { ok: true, value };
}

/** Canonical decimal-string cursor. `'0'` means "no events acknowledged yet". */
export type LastEventIdCursor = string;

export const ZERO_CURSOR: LastEventIdCursor = '0';

/** Normalize a seed value (e.g. restored from storage) into a canonical cursor. Throws on an invalid seed. */
export function createCursor(seed?: string): LastEventIdCursor {
  if (seed === undefined) {
    return ZERO_CURSOR;
  }
  const parsed = parseLastEventIdToken(seed);
  if (!parsed.ok) {
    throw new Error(`invalid Last-Event-ID seed "${seed}": ${parsed.error}`);
  }
  return parsed.value.toString();
}

/** The sequence a *contiguous* stream would deliver next after `cursor`. */
export function nextExpected(cursor: LastEventIdCursor): LastEventIdCursor {
  return (BigInt(cursor) + 1n).toString();
}

export type AdvanceCursorResult =
  | { ok: true; cursor: LastEventIdCursor; contiguous: boolean; expected: LastEventIdCursor }
  | { ok: false; error: LastEventIdErrorKind | 'not_monotonic' };

/**
 * Advance `current` by a newly received `id:` value.
 *
 * Only two things are ever guaranteed by the server, and they differ by
 * endpoint — read the query, don't assume:
 *
 * - Strictly increasing (`received > current`) is guaranteed for BOTH
 *   endpoints: job events select `WHERE sequence_no > $after` (jobs.rs:212)
 *   and ask events do the same (ask_streams.rs list_events_after). A
 *   `received <= current` id is never legitimate — it means a duplicate or
 *   out-of-order delivery, so this returns `not_monotonic` rather than
 *   silently accepting it.
 * - Contiguity (`received === current + 1`) is NOT guaranteed for job
 *   events: `event_log.sequence_no` is a single counter shared by the whole
 *   org (`next_event_sequence`, jobs.rs:993-1001 — `MAX(sequence_no) FROM
 *   event_log WHERE org_id = $1`, no `job_id` filter), so any other job's or
 *   session's events legitimately consume sequence numbers in between the
 *   ones a given job's SSE client sees. A generic client that assumes
 *   per-stream contiguity would misfire constantly on job streams. Ask
 *   stream sequences, by contrast, ARE per-session
 *   (`ask_stream_sessions.next_sequence`, db/ask_streams.rs:82,401) so a
 *   non-contiguous jump there really does mean a missing/purged event.
 *
 * This function reports `contiguous` either way and lets the caller decide
 * what a gap means for their endpoint; it never hides the jump.
 */
export function advanceCursor(current: LastEventIdCursor, receivedId: string): AdvanceCursorResult {
  const parsed = parseLastEventIdToken(receivedId);
  if (!parsed.ok) {
    return { ok: false, error: parsed.error };
  }
  if (parsed.value <= BigInt(current)) {
    return { ok: false, error: 'not_monotonic' };
  }
  const expected = nextExpected(current);
  const cursor = parsed.value.toString();
  return { ok: true, cursor, contiguous: cursor === expected, expected };
}

export interface CursorRequestParams {
  readonly query: string;
  readonly header: string;
}

/**
 * Build the `lastEventId` query value + `Last-Event-ID` header value for a
 * (re)connect at `cursor`. Returns `undefined` for the zero cursor so a
 * fresh connection omits both — required for `/ask/stream` without a
 * `streamSessionId`, which rejects any non-zero cursor
 * (`routes/ask.rs:177-182`, `LastEventIdError::OutOfRange`) — and matches
 * plain `resolve_last_event_id(None, None, _) -> 0` for job events.
 *
 * Query and header are always sent equal on purpose: `resolve_last_event_id`
 * only accepts an equal pair or a single source, never differing values
 * (last_event_id.rs:66-71), and the live reconnect tests always send both
 * (sse_stream_readiness.rs:561-566, :1110-1115).
 */
export function requestCursorParams(cursor: LastEventIdCursor): CursorRequestParams | undefined {
  if (cursor === ZERO_CURSOR) {
    return undefined;
  }
  return { query: cursor, header: cursor };
}
