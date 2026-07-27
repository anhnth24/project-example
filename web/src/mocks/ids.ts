/**
 * Deterministic id/timestamp/cursor generation for mock fixtures.
 *
 * Deterministic on purpose: tests that assert against mock responses should
 * get the same uuid/timestamp/cursor every run, not something that changes
 * with wall-clock time or a random seed. All three helpers are pure functions
 * of an integer counter/offset the caller supplies (usually a small fixture
 * index), so the same call always returns the same value.
 */

const UUID_NAMESPACE = '00000000-0000-4000-8000-'; // fixed prefix; only the last 12 hex digits vary

/** A stable, syntactically-valid (RFC 4122 v4-shaped) uuid derived from a small integer. */
export function mockUuid(seed: number): string {
  if (!Number.isInteger(seed) || seed < 0) {
    throw new Error(`mockUuid: seed must be a non-negative integer, got ${seed}`);
  }
  return `${UUID_NAMESPACE}${seed.toString(16).padStart(12, '0')}`;
}

const BASE_TIMESTAMP_MS = Date.UTC(2026, 0, 1, 0, 0, 0); // 2026-01-01T00:00:00.000Z

/** A stable ISO-8601 timestamp, `stepMinutes` minutes after a fixed epoch. */
export function mockTimestamp(stepMinutes: number): string {
  return new Date(BASE_TIMESTAMP_MS + stepMinutes * 60_000).toISOString();
}

/**
 * Opaque pagination cursor helpers. The cursor is a base64url encoding of the
 * next-offset integer — opaque to callers (per the spec's "opaque pagination
 * cursor" wording), but it genuinely round-trips through `PageInfo.nextCursor`
 * the way the real API's cursor is expected to.
 */
export function encodeCursor(offset: number): string {
  if (!Number.isInteger(offset) || offset < 0) {
    throw new Error(`encodeCursor: offset must be a non-negative integer, got ${offset}`);
  }
  return btoa(`offset:${offset}`).replace(/=+$/, '');
}

export function decodeCursor(cursor: string | null | undefined): number {
  if (!cursor) return 0;
  let decoded: string;
  try {
    decoded = atob(cursor);
  } catch {
    throw new Error(`decodeCursor: "${cursor}" is not valid base64`);
  }
  const match = /^offset:(\d+)$/.exec(decoded);
  if (!match)
    throw new Error(`decodeCursor: "${cursor}" did not decode to a recognizable offset cursor`);
  return Number(match[1]);
}

let requestIdCounter = 0;
/** A fresh per-response requestId. Reset between tests via `resetRequestIdCounter`. */
export function nextRequestId(): string {
  requestIdCounter += 1;
  return mockUuid(0xfeed_0000 + requestIdCounter);
}

export function resetRequestIdCounter(): void {
  requestIdCounter = 0;
}
