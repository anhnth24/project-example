// Session/token state for the SPA. Access token lives in memory only (no
// storage, no cookie) and every consumer — the typed client in `client.ts`,
// and the SSE agent's `sse.ts` — goes through the `TokenProvider` seam below
// so there is exactly one refresh in flight at any time.
import type { components } from './generated/contract';

/** The pair `POST /auth/login` and `POST /auth/refresh` both return. */
export type SessionTokens = components['schemas']['TokenResponse'];

/**
 * Seam shared with the SSE agent (`sse.ts` + `lastEventId.ts`). This shape is
 * fixed — the other agent imports only the type and must not need to read
 * this file's implementation. Do not change it without flagging it back.
 */
export interface TokenProvider {
  /** Current access token, refreshing first if it is expired or missing. Concurrent callers share one in-flight refresh. */
  getAccessToken(): Promise<string>;
  /** Force one refresh after a 401 on a request that used a token believed valid. Also single-flight. */
  refreshNow(): Promise<string>;
  /** Subscribe to terminal session loss (refresh rejected). Returns an unsubscribe. */
  onSessionLost(listener: () => void): () => void;
}

/** Thrown when a refresh attempt fails: the session is over until the next `setTokens`. */
export class SessionLostError extends Error {
  readonly name = 'SessionLostError';
  readonly cause?: unknown;

  constructor(
    message = 'Session refresh failed; the session is no longer valid.',
    cause?: unknown,
  ) {
    super(message);
    this.cause = cause;
  }
}

/** Thrown when a token is requested but nobody has ever logged in (no refresh token to use). */
export class NoSessionError extends Error {
  readonly name = 'NoSessionError';

  constructor(message = 'No active session.') {
    super(message);
  }
}

/** Performs the actual network round-trip; injected so this module has no fetch/contract coupling. */
export type RefreshFn = (refreshToken: string) => Promise<SessionTokens>;

export interface SessionManager extends TokenProvider {
  /** Installs a fresh token pair (e.g. after login). Clears any prior terminal session-lost state. */
  setTokens(tokens: SessionTokens): void;
  /** Drops in-memory tokens without notifying `onSessionLost` listeners (e.g. after an explicit logout call completes). */
  clear(): void;
  /** Whether any token (access or refresh) is currently held. */
  hasSession(): boolean;
  /** The refresh token to send in `POST /auth/logout`'s body, or null if there is no session. */
  getRefreshTokenForLogout(): string | null;
  /**
   * Subscribe to every token rotation, including the ones the manager performs
   * on its own during a refresh. Returns an unsubscribe.
   *
   * ADR 0010 rotates the refresh token on every use and revokes the whole family
   * when an already-rotated token is replayed. Anything that persists the refresh
   * token therefore cannot wait for a lifecycle event to write it back: a copy
   * that misses one rotation is not merely stale, it is a token whose next use
   * looks like a replay attack and signs the user out everywhere. Listeners fire
   * synchronously as part of applying the new pair.
   */
  onTokensRotated(listener: (tokens: SessionTokens) => void): () => void;
}

export interface SessionManagerOptions {
  /**
   * Milliseconds subtracted from the server's `expiresIn` before we consider
   * the access token due for renewal, so `getAccessToken` refreshes slightly
   * ahead of actual server-side expiry instead of racing it. Default 10s.
   */
  expirySkewMs?: number;
}

const DEFAULT_EXPIRY_SKEW_MS = 10_000;

export function createSessionManager(
  refreshFn: RefreshFn,
  options: SessionManagerOptions = {},
): SessionManager {
  const skewMs = options.expirySkewMs ?? DEFAULT_EXPIRY_SKEW_MS;

  let accessToken: string | null = null;
  let refreshToken: string | null = null;
  let expiresAt = 0;
  let terminallyLost = false;
  let inFlight: Promise<string> | null = null;
  const listeners = new Set<() => void>();
  const rotationListeners = new Set<(tokens: SessionTokens) => void>();

  function isExpired(): boolean {
    return accessToken === null || Date.now() >= expiresAt;
  }

  function notifySessionLost(): void {
    for (const listener of [...listeners]) listener();
  }

  function applyTokens(tokens: SessionTokens): string {
    accessToken = tokens.accessToken;
    refreshToken = tokens.refreshToken;
    expiresAt = Date.now() + tokens.expiresIn * 1000 - skewMs;
    // Synchronous, so a persisted copy can never lag a rotation. A listener that
    // throws must not take the session down with it: the tokens are already
    // applied and the caller is mid-refresh.
    for (const listener of [...rotationListeners]) {
      try {
        listener(tokens);
      } catch {
        // ignored on purpose; persistence is best-effort, the session is not
      }
    }
    return tokens.accessToken;
  }

  function forgetTokens(): void {
    accessToken = null;
    refreshToken = null;
    expiresAt = 0;
  }

  // Single-flight: the first caller creates `inFlight`; every concurrent
  // caller (whether it arrived via `getAccessToken` on an expired token or
  // `refreshNow` reacting to a 401) observes it already set — synchronously,
  // before any `await` — and shares the same promise. A failed attempt marks
  // the session terminally lost so later calls reject immediately instead of
  // retrying in a loop, until the next `setTokens`.
  function triggerRefresh(): Promise<string> {
    if (terminallyLost) {
      return Promise.reject(new SessionLostError());
    }
    if (!refreshToken) {
      return Promise.reject(new NoSessionError());
    }
    if (!inFlight) {
      inFlight = refreshFn(refreshToken)
        .then(applyTokens)
        .catch((cause: unknown) => {
          terminallyLost = true;
          forgetTokens();
          notifySessionLost();
          throw cause;
        })
        .finally(() => {
          inFlight = null;
        });
    }
    return inFlight;
  }

  return {
    async getAccessToken() {
      if (terminallyLost) throw new SessionLostError();
      if (accessToken === null && refreshToken === null) throw new NoSessionError();
      if (!isExpired() && accessToken !== null) return accessToken;
      return triggerRefresh();
    },

    async refreshNow() {
      return triggerRefresh();
    },

    onSessionLost(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },

    onTokensRotated(listener) {
      rotationListeners.add(listener);
      return () => rotationListeners.delete(listener);
    },

    setTokens(tokens) {
      terminallyLost = false;
      applyTokens(tokens);
    },

    clear() {
      terminallyLost = false;
      forgetTokens();
    },

    hasSession() {
      return accessToken !== null || refreshToken !== null;
    },

    getRefreshTokenForLogout() {
      return refreshToken;
    },
  };
}
