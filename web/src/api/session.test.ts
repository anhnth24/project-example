import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
  createSessionManager,
  NoSessionError,
  SessionLostError,
  type RefreshFn,
  type SessionTokens,
} from './session';

function tokens(overrides: Partial<SessionTokens> = {}): SessionTokens {
  return {
    accessToken: 'access-1',
    refreshToken: 'refresh-1',
    tokenType: 'Bearer',
    expiresIn: 3600,
    orgId: 'org-1',
    userId: 'user-1',
    ...overrides,
  };
}

function expiredTokens(overrides: Partial<SessionTokens> = {}): SessionTokens {
  // Negative expiresIn puts expiresAt in the past regardless of skew.
  return tokens({ expiresIn: -1, ...overrides });
}

/** A promise plus externally-callable resolve/reject, so tests can control exactly when a "network" call settles. */
function createDeferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

describe('createSessionManager', () => {
  describe('no session', () => {
    it('rejects getAccessToken with NoSessionError and never calls refreshFn', async () => {
      const refreshFn = vi.fn<RefreshFn>();
      const manager = createSessionManager(refreshFn);
      await expect(manager.getAccessToken()).rejects.toBeInstanceOf(NoSessionError);
      expect(refreshFn).not.toHaveBeenCalled();
    });
  });

  describe('valid cached token', () => {
    it('returns the cached access token without calling refreshFn', async () => {
      const refreshFn = vi.fn<RefreshFn>();
      const manager = createSessionManager(refreshFn);
      manager.setTokens(tokens());
      await expect(manager.getAccessToken()).resolves.toBe('access-1');
      expect(refreshFn).not.toHaveBeenCalled();
    });
  });

  describe('rotation notifications', () => {
    // ADR 0010 rotates the refresh token on every use and revokes the family
    // when an already-rotated one is replayed, so anything persisting the token
    // has to learn about the rotation the manager performs on its own — not
    // just the ones the caller triggers.
    it('notifies on the rotation performed inside a refresh, not only on setTokens', async () => {
      const refreshFn = vi
        .fn<RefreshFn>()
        .mockResolvedValue(tokens({ accessToken: 'access-2', refreshToken: 'refresh-2' }));
      const manager = createSessionManager(refreshFn);
      const seen: string[] = [];
      manager.onTokensRotated((rotated) => seen.push(rotated.refreshToken));

      manager.setTokens(expiredTokens({ refreshToken: 'refresh-1' }));
      expect(seen).toEqual(['refresh-1']);

      await manager.getAccessToken();
      expect(refreshFn).toHaveBeenCalledTimes(1);
      // The rotation the caller never asked for is the one that matters.
      expect(seen).toEqual(['refresh-1', 'refresh-2']);
    });

    it('fires once per rotation even when concurrent callers share one refresh', async () => {
      const refreshFn = vi.fn<RefreshFn>().mockResolvedValue(tokens({ refreshToken: 'refresh-2' }));
      const manager = createSessionManager(refreshFn);
      const seen: string[] = [];
      manager.setTokens(expiredTokens({ refreshToken: 'refresh-1' }));
      manager.onTokensRotated((rotated) => seen.push(rotated.refreshToken));

      await Promise.all([
        manager.getAccessToken(),
        manager.getAccessToken(),
        manager.getAccessToken(),
      ]);
      expect(seen).toEqual(['refresh-2']);
    });

    it('unsubscribes, and a throwing listener does not break the refresh', async () => {
      const refreshFn = vi.fn<RefreshFn>().mockResolvedValue(tokens({ accessToken: 'access-2' }));
      const manager = createSessionManager(refreshFn);
      manager.onTokensRotated(() => {
        throw new Error('persistence is unavailable');
      });
      const seen: string[] = [];
      const unsubscribe = manager.onTokensRotated((rotated) => seen.push(rotated.accessToken));

      manager.setTokens(expiredTokens());
      unsubscribe();
      await expect(manager.getAccessToken()).resolves.toBe('access-2');
      expect(seen).toEqual(['access-1']);
    });
  });

  describe('single-flight refresh', () => {
    it('shares exactly one in-flight refresh across N concurrent getAccessToken calls on an expired token', async () => {
      const deferred = createDeferred<SessionTokens>();
      const refreshFn = vi.fn(() => deferred.promise);
      const manager = createSessionManager(refreshFn);
      manager.setTokens(expiredTokens());

      const p1 = manager.getAccessToken();
      const p2 = manager.getAccessToken();
      const p3 = manager.getAccessToken();

      // Give any (incorrect) per-call refresh a chance to fire before we resolve.
      await Promise.resolve();
      await Promise.resolve();
      expect(refreshFn).toHaveBeenCalledTimes(1);

      deferred.resolve(tokens({ accessToken: 'access-2', refreshToken: 'refresh-2' }));

      await expect(Promise.all([p1, p2, p3])).resolves.toEqual([
        'access-2',
        'access-2',
        'access-2',
      ]);
      expect(refreshFn).toHaveBeenCalledTimes(1);
    });

    it('shares exactly one in-flight refresh across N concurrent refreshNow calls', async () => {
      const deferred = createDeferred<SessionTokens>();
      const refreshFn = vi.fn(() => deferred.promise);
      const manager = createSessionManager(refreshFn);
      manager.setTokens(tokens()); // not expired — refreshNow forces it anyway

      const calls = [manager.refreshNow(), manager.refreshNow(), manager.refreshNow()];
      await Promise.resolve();
      await Promise.resolve();
      expect(refreshFn).toHaveBeenCalledTimes(1);

      deferred.resolve(tokens({ accessToken: 'access-3' }));
      await expect(Promise.all(calls)).resolves.toEqual(['access-3', 'access-3', 'access-3']);
      expect(refreshFn).toHaveBeenCalledTimes(1);
    });

    it('shares one in-flight refresh between a mix of getAccessToken (expired) and refreshNow callers', async () => {
      const deferred = createDeferred<SessionTokens>();
      const refreshFn = vi.fn(() => deferred.promise);
      const manager = createSessionManager(refreshFn);
      manager.setTokens(expiredTokens());

      const calls = [manager.getAccessToken(), manager.refreshNow(), manager.getAccessToken()];
      await Promise.resolve();
      expect(refreshFn).toHaveBeenCalledTimes(1);

      deferred.resolve(tokens({ accessToken: 'access-4' }));
      await expect(Promise.all(calls)).resolves.toEqual(['access-4', 'access-4', 'access-4']);
      expect(refreshFn).toHaveBeenCalledTimes(1);
    });

    it('only refreshes once per real refresh call, not once per concurrent invocation (fails if single-flight is removed)', async () => {
      // Regression guard: refreshFn returns a different token per call. If
      // single-flight were broken, concurrent callers would disagree on the
      // token, since each would have triggered its own refresh.
      let counter = 0;
      const refreshFn = vi.fn(async () => tokens({ accessToken: `access-${++counter}` }));
      const manager = createSessionManager(refreshFn);
      manager.setTokens(expiredTokens());

      const results = await Promise.all([
        manager.getAccessToken(),
        manager.getAccessToken(),
        manager.getAccessToken(),
      ]);
      expect(new Set(results).size).toBe(1);
      expect(refreshFn).toHaveBeenCalledTimes(1);
    });
  });

  describe('failed refresh', () => {
    it('rejects every concurrent waiter and fires onSessionLost exactly once', async () => {
      const failure = new Error('refresh token rejected by server');
      const refreshFn = vi.fn(async () => {
        throw failure;
      });
      const manager = createSessionManager(refreshFn);
      manager.setTokens(expiredTokens());

      const lost = vi.fn();
      manager.onSessionLost(lost);

      const results = await Promise.allSettled([
        manager.getAccessToken(),
        manager.refreshNow(),
        manager.refreshNow(),
      ]);

      for (const result of results) {
        expect(result.status).toBe('rejected');
      }
      expect(refreshFn).toHaveBeenCalledTimes(1);
      expect(lost).toHaveBeenCalledTimes(1);
    });

    it('does not retry the refresh in a loop after a terminal failure', async () => {
      const refreshFn = vi.fn(async () => {
        throw new Error('invalid refresh token');
      });
      const manager = createSessionManager(refreshFn);
      manager.setTokens(expiredTokens());

      await expect(manager.getAccessToken()).rejects.toThrow('invalid refresh token');
      expect(refreshFn).toHaveBeenCalledTimes(1);

      // Further calls must reject immediately from cached terminal state —
      // no second network attempt.
      await expect(manager.getAccessToken()).rejects.toBeInstanceOf(SessionLostError);
      await expect(manager.refreshNow()).rejects.toBeInstanceOf(SessionLostError);
      expect(refreshFn).toHaveBeenCalledTimes(1);
    });

    it('clears in-memory tokens once terminally lost', async () => {
      const refreshFn = vi.fn(async () => {
        throw new Error('nope');
      });
      const manager = createSessionManager(refreshFn);
      manager.setTokens(expiredTokens());
      await expect(manager.getAccessToken()).rejects.toThrow();
      expect(manager.hasSession()).toBe(false);
      expect(manager.getRefreshTokenForLogout()).toBeNull();
    });

    it('lets a fresh setTokens recover from a terminally-lost state', async () => {
      const refreshFn = vi.fn(async () => {
        throw new Error('nope');
      });
      const manager = createSessionManager(refreshFn);
      manager.setTokens(expiredTokens());
      await expect(manager.getAccessToken()).rejects.toBeInstanceOf(Error);

      manager.setTokens(tokens({ accessToken: 'fresh' }));
      await expect(manager.getAccessToken()).resolves.toBe('fresh');
    });
  });

  describe('onSessionLost subscription', () => {
    it('stops delivering after unsubscribe', async () => {
      const refreshFn = vi.fn(async () => {
        throw new Error('nope');
      });
      const manager = createSessionManager(refreshFn);
      manager.setTokens(expiredTokens());

      const lost = vi.fn();
      const unsubscribe = manager.onSessionLost(lost);
      unsubscribe();

      await expect(manager.getAccessToken()).rejects.toThrow();
      expect(lost).not.toHaveBeenCalled();
    });

    it('supports multiple independent listeners, each called once', async () => {
      const refreshFn = vi.fn(async () => {
        throw new Error('nope');
      });
      const manager = createSessionManager(refreshFn);
      manager.setTokens(expiredTokens());

      const a = vi.fn();
      const b = vi.fn();
      manager.onSessionLost(a);
      manager.onSessionLost(b);

      await expect(
        Promise.allSettled([manager.getAccessToken(), manager.getAccessToken()]),
      ).resolves.toBeDefined();

      expect(a).toHaveBeenCalledTimes(1);
      expect(b).toHaveBeenCalledTimes(1);
    });
  });

  describe('clear', () => {
    it('drops tokens without notifying onSessionLost listeners', () => {
      const refreshFn = vi.fn<RefreshFn>();
      const manager = createSessionManager(refreshFn);
      manager.setTokens(tokens());
      const lost = vi.fn();
      manager.onSessionLost(lost);

      manager.clear();

      expect(manager.hasSession()).toBe(false);
      expect(lost).not.toHaveBeenCalled();
    });
  });
});

describe('expiry skew', () => {
  beforeEach(() => {
    vi.useRealTimers();
  });

  it('treats the token as due for renewal before the server-declared expiresIn elapses', async () => {
    vi.useFakeTimers();
    try {
      const refreshFn = vi.fn(async () => tokens({ accessToken: 'renewed' }));
      const manager = createSessionManager(refreshFn, { expirySkewMs: 10_000 });
      manager.setTokens(tokens({ expiresIn: 15 })); // expires in 15s, skew 10s -> stale after 5s

      vi.advanceTimersByTime(6_000);
      await expect(manager.getAccessToken()).resolves.toBe('renewed');
      expect(refreshFn).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });
});
