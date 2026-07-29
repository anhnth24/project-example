import { useState } from 'react';
import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ApiClient } from '../api/client';
import { HttpApiError } from '../api/errors';
import type { SessionTokens } from '../api/session';
import { installMockFetch, mockControl, resetMockState, uninstallMockFetch } from '../mocks';
import { createApiClient } from '../api/client';
import { ORG_A_ID, ORG_B_ID } from '../mocks/fixtures';
import { ScopeProvider, useScope } from '../state/ScopeProvider';
import { AuthProvider, useAuth } from './AuthContext';
import { loadPersistedRefreshToken } from './tokenStorage';

const DEMO_EMAIL = 'demo@markhand.test';
const DEMO_PASSWORD = 'demo-password';

/** Deferred promise helper for deterministic race tests (stale `me()` after logout, etc). */
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

const SAMPLE_ME = {
  userId: 'user-1',
  orgId: 'org-1',
  email: DEMO_EMAIL,
  displayName: 'Demo User',
  permissions: ['qa.history'],
  allowedCollectionIds: ['col-1'],
  sessionId: 'session-1',
};

function sampleTokens(overrides: Partial<SessionTokens> = {}): SessionTokens {
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

/**
 * A hand-built `ApiClient` double: only the members `AuthContext` actually
 * calls are implemented, each a directly-controllable `vi.fn()`/deferred so
 * races (stale response after logout, session-lost mid-flight) can be
 * driven deterministically without timing tricks against real fetch/timers.
 */
function createFakeClient() {
  let sessionLostListener: (() => void) | null = null;
  let rotationListener: ((tokens: SessionTokens) => void) | null = null;
  const setTokensCalls: SessionTokens[] = [];
  let currentRefreshToken: string | null = null;
  let clearedCount = 0;

  const client = {
    login: vi.fn(),
    logout: vi.fn(async () => {}),
    me: vi.fn(),
    request: vi.fn(),
    tokenProvider: {
      getAccessToken: vi.fn(),
      refreshNow: vi.fn(),
      onSessionLost(listener: () => void) {
        sessionLostListener = listener;
        return () => {
          if (sessionLostListener === listener) sessionLostListener = null;
        };
      },
    },
    sessionManager: {
      setTokens: vi.fn((tokens: SessionTokens) => {
        setTokensCalls.push(tokens);
        currentRefreshToken = tokens.refreshToken;
      }),
      clear: vi.fn(() => {
        clearedCount += 1;
        currentRefreshToken = null;
      }),
      hasSession: vi.fn(() => currentRefreshToken !== null),
      getRefreshTokenForLogout: vi.fn(() => currentRefreshToken),
      getAccessToken: vi.fn(),
      refreshNow: vi.fn(),
      onSessionLost(listener: () => void) {
        sessionLostListener = listener;
        return () => {
          if (sessionLostListener === listener) sessionLostListener = null;
        };
      },
      onTokensRotated(listener: (tokens: SessionTokens) => void) {
        rotationListener = listener;
        return () => {
          if (rotationListener === listener) rotationListener = null;
        };
      },
    },
  } as unknown as ApiClient;

  return {
    client,
    setTokensCalls,
    fireSessionLost: () => sessionLostListener?.(),
    fireRotation: (tokens: SessionTokens) => {
      currentRefreshToken = tokens.refreshToken;
      rotationListener?.(tokens);
    },
    hasRotationListener: () => rotationListener !== null,
    clearedCount: () => clearedCount,
    currentRefreshToken: () => currentRefreshToken,
  };
}

function Probe() {
  const { session } = useAuth();
  return <span data-testid="status">{session.status}</span>;
}

function ScopeProbe() {
  const { scope } = useScope();
  return <span data-testid="scope-org">{scope ? scope.orgId : 'null'}</span>;
}

afterEach(() => {
  cleanup();
  window.sessionStorage.clear();
  uninstallMockFetch();
});

describe('AuthContext (default/outside provider)', () => {
  it('defaults to an anonymous session when useAuth is called outside a provider', () => {
    render(<Probe />);
    expect(screen.getByTestId('status')).toHaveTextContent('anonymous');
  });
});

describe('AuthContext bootstrap', () => {
  it('goes straight to anonymous with no persisted refresh token, without calling me()', async () => {
    const { client } = createFakeClient();
    render(
      <ScopeProvider>
        <AuthProvider client={client}>
          <Probe />
        </AuthProvider>
      </ScopeProvider>,
    );
    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('anonymous'));
    expect(client.me).not.toHaveBeenCalled();
  });

  it('restores an authenticated session from a persisted refresh token', async () => {
    window.sessionStorage.setItem('markhand.refreshToken', 'stored-refresh');
    const { client, setTokensCalls } = createFakeClient();
    (client.me as ReturnType<typeof vi.fn>).mockResolvedValue(SAMPLE_ME);

    render(
      <ScopeProvider>
        <AuthProvider client={client}>
          <Probe />
        </AuthProvider>
      </ScopeProvider>,
    );

    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('authenticated'));
    // Seeded with the persisted refresh token behind an already-expired
    // access token, so the real session manager's own refresh path (not a
    // hand-rolled one here) is what would fetch a fresh access token.
    expect(setTokensCalls[0]).toMatchObject({ refreshToken: 'stored-refresh', expiresIn: -1 });
  });

  it('clears the persisted token and stays anonymous when the stored refresh token is rejected', async () => {
    window.sessionStorage.setItem('markhand.refreshToken', 'stale-refresh');
    const { client } = createFakeClient();
    (client.me as ReturnType<typeof vi.fn>).mockRejectedValue(
      new HttpApiError({
        status: 401,
        code: 'unauthorized',
        message: 'refresh token is invalid',
        requestId: 'r1',
      }),
    );

    render(
      <ScopeProvider>
        <AuthProvider client={client}>
          <Probe />
        </AuthProvider>
      </ScopeProvider>,
    );

    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('anonymous'));
    expect(loadPersistedRefreshToken()).toBeNull();
  });
});

describe('AuthContext login/logout', () => {
  it('login() populates the session from me() and persists the refresh token', async () => {
    const { client } = createFakeClient();
    (client.login as ReturnType<typeof vi.fn>).mockResolvedValue(sampleTokens());
    (client.me as ReturnType<typeof vi.fn>).mockResolvedValue(SAMPLE_ME);

    function Harness() {
      const { session, login } = useAuth();
      return (
        <>
          <span data-testid="status">{session.status}</span>
          <button onClick={() => void login(DEMO_EMAIL, DEMO_PASSWORD)}>go</button>
        </>
      );
    }

    render(
      <ScopeProvider>
        <AuthProvider client={client}>
          <Harness />
        </AuthProvider>
      </ScopeProvider>,
    );
    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('anonymous'));

    await act(async () => {
      screen.getByText('go').click();
      await Promise.resolve();
      await Promise.resolve();
    });

    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('authenticated'));
    expect(loadPersistedRefreshToken()).toBe('refresh-1');
  });

  it('login() surfaces a failed me() as an anonymous session and rethrows', async () => {
    const { client } = createFakeClient();
    (client.login as ReturnType<typeof vi.fn>).mockResolvedValue(sampleTokens());
    (client.me as ReturnType<typeof vi.fn>).mockRejectedValue(new Error('me failed'));

    let caught: unknown;
    function Harness() {
      const { session, login } = useAuth();
      return (
        <>
          <span data-testid="status">{session.status}</span>
          <button
            onClick={() => {
              login(DEMO_EMAIL, DEMO_PASSWORD).catch((e) => {
                caught = e;
              });
            }}
          >
            go
          </button>
        </>
      );
    }

    render(
      <ScopeProvider>
        <AuthProvider client={client}>
          <Harness />
        </AuthProvider>
      </ScopeProvider>,
    );
    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('anonymous'));

    await act(async () => {
      screen.getByText('go').click();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(screen.getByTestId('status')).toHaveTextContent('anonymous');
    expect(caught).toBeInstanceOf(Error);
    expect(loadPersistedRefreshToken()).toBeNull();
  });

  it('logout() clears the session immediately and calls the API best-effort', async () => {
    window.sessionStorage.setItem('markhand.refreshToken', 'stored-refresh');
    const { client } = createFakeClient();
    (client.me as ReturnType<typeof vi.fn>).mockResolvedValue(SAMPLE_ME);

    function Harness() {
      const { session, logout } = useAuth();
      return (
        <>
          <span data-testid="status">{session.status}</span>
          <button onClick={() => void logout()}>bye</button>
        </>
      );
    }

    render(
      <ScopeProvider>
        <AuthProvider client={client}>
          <Harness />
        </AuthProvider>
      </ScopeProvider>,
    );
    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('authenticated'));

    await act(async () => {
      screen.getByText('bye').click();
      await Promise.resolve();
    });

    expect(screen.getByTestId('status')).toHaveTextContent('anonymous');
    expect(client.logout).toHaveBeenCalledTimes(1);
    expect(loadPersistedRefreshToken()).toBeNull();
  });

  it('a me() response that resolves after logout() must not repopulate the shell', async () => {
    window.sessionStorage.setItem('markhand.refreshToken', 'stored-refresh');
    const { client } = createFakeClient();
    const pendingMe = deferred<typeof SAMPLE_ME>();
    (client.me as ReturnType<typeof vi.fn>).mockReturnValue(pendingMe.promise);

    function Harness() {
      const { session, logout } = useAuth();
      return (
        <>
          <span data-testid="status">{session.status}</span>
          <button onClick={() => void logout()}>bye</button>
        </>
      );
    }

    render(
      <ScopeProvider>
        <AuthProvider client={client}>
          <Harness />
        </AuthProvider>
      </ScopeProvider>,
    );
    // Bootstrap's me() call is in flight (never resolved yet).
    await waitFor(() => expect(client.me).toHaveBeenCalledTimes(1));

    await act(async () => {
      screen.getByText('bye').click();
      await Promise.resolve();
    });
    expect(screen.getByTestId('status')).toHaveTextContent('anonymous');

    // The stale bootstrap `me()` now resolves — after logout already ran.
    await act(async () => {
      pendingMe.resolve(SAMPLE_ME);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(screen.getByTestId('status')).toHaveTextContent('anonymous');
  });

  // Race hardening (PR #323 follow-up): `switchOrg` already guarded its own
  // out-of-order resolution with `if (epochRef.current !== epoch) return;`
  // right after its await; `login()` previously had no equivalent guard at
  // all, so a login that resolved late enough could install stale tokens
  // over a newer login or logout that had already won. These two tests drive
  // that same shape of race through `login()` — deliberately resolving the
  // *stale* attempt's `client.login()` only after the superseding
  // logout/login has already run, out of order, to prove the stale one is
  // discarded rather than merely "usually loses the race".
  describe('login() out-of-order resolution', () => {
    it('login vs logout: a login that resolves after logout() must not persist tokens or repopulate the shell', async () => {
      const { client } = createFakeClient();
      const pendingLogin = deferred<SessionTokens>();
      (client.login as ReturnType<typeof vi.fn>).mockReturnValue(pendingLogin.promise);
      (client.me as ReturnType<typeof vi.fn>).mockResolvedValue(SAMPLE_ME);

      function Harness() {
        const { session, login, logout } = useAuth();
        return (
          <>
            <span data-testid="status">{session.status}</span>
            <button onClick={() => void login(DEMO_EMAIL, DEMO_PASSWORD).catch(() => {})}>
              go
            </button>
            <button onClick={() => void logout()}>bye</button>
          </>
        );
      }

      render(
        <ScopeProvider>
          <AuthProvider client={client}>
            <Harness />
          </AuthProvider>
        </ScopeProvider>,
      );
      await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('anonymous'));

      await act(async () => {
        screen.getByText('go').click();
        await Promise.resolve();
      });
      expect(client.login).toHaveBeenCalledTimes(1);

      // logout() fires while the login's own client.login() call is still
      // pending — it supersedes the login (bumps the epoch) well before the
      // stale login ever resolves.
      await act(async () => {
        screen.getByText('bye').click();
        await Promise.resolve();
      });
      expect(screen.getByTestId('status')).toHaveTextContent('anonymous');

      // The stale login's client.login() now resolves successfully — after
      // logout already ran. Without the epoch guard right after this await,
      // this would call savePersistedRefreshToken() and go on to applyMe().
      await act(async () => {
        pendingLogin.resolve(sampleTokens());
        await Promise.resolve();
        await Promise.resolve();
        await Promise.resolve();
      });

      expect(screen.getByTestId('status')).toHaveTextContent('anonymous');
      expect(client.me).not.toHaveBeenCalled();
      expect(loadPersistedRefreshToken()).toBeNull();
    });

    it('login vs login: a rapid second login supersedes the first — the first stale resolution never applies', async () => {
      const { client } = createFakeClient();
      const loginCalls: Array<{ resolve: (tokens: SessionTokens) => void }> = [];
      (client.login as ReturnType<typeof vi.fn>).mockImplementation(() => {
        const pending = deferred<SessionTokens>();
        loginCalls.push(pending);
        return pending.promise;
      });
      (client.me as ReturnType<typeof vi.fn>).mockResolvedValue(SAMPLE_ME);

      function Harness() {
        const { session, login } = useAuth();
        return (
          <>
            <span data-testid="status">{session.status}</span>
            <button onClick={() => void login('first@markhand.test', 'pw').catch(() => {})}>
              first
            </button>
            <button onClick={() => void login('second@markhand.test', 'pw').catch(() => {})}>
              second
            </button>
          </>
        );
      }

      render(
        <ScopeProvider>
          <AuthProvider client={client}>
            <Harness />
          </AuthProvider>
        </ScopeProvider>,
      );
      await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('anonymous'));

      await act(async () => {
        screen.getByText('first').click();
        await Promise.resolve();
        screen.getByText('second').click();
        await Promise.resolve();
      });
      expect(loginCalls.length).toBe(2);

      // Resolve the FIRST (superseded) login's client.login() only after the
      // second one already started — out of order, to prove it's discarded
      // rather than merely "usually resolves last".
      await act(async () => {
        loginCalls[0].resolve(
          sampleTokens({ accessToken: 'access-first', refreshToken: 'refresh-first' }),
        );
        await Promise.resolve();
        await Promise.resolve();
      });
      // Discarded: no me() call and no persisted token from the stale first
      // login — only the second login's own client.login() can produce those.
      expect(client.me).not.toHaveBeenCalled();
      expect(loadPersistedRefreshToken()).not.toBe('refresh-first');

      await act(async () => {
        loginCalls[1].resolve(
          sampleTokens({ accessToken: 'access-second', refreshToken: 'refresh-second' }),
        );
        await Promise.resolve();
        await Promise.resolve();
        await Promise.resolve();
      });

      await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('authenticated'));
      expect(client.me).toHaveBeenCalledTimes(1);
      expect(loadPersistedRefreshToken()).toBe('refresh-second');
    });
  });
});

describe('AuthContext session loss', () => {
  it('onSessionLost mid-session clears the persisted token and becomes anonymous', async () => {
    window.sessionStorage.setItem('markhand.refreshToken', 'stored-refresh');
    const { client, fireSessionLost } = createFakeClient();
    (client.me as ReturnType<typeof vi.fn>).mockResolvedValue(SAMPLE_ME);

    render(
      <ScopeProvider>
        <AuthProvider client={client}>
          <Probe />
        </AuthProvider>
      </ScopeProvider>,
    );
    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('authenticated'));

    act(() => {
      fireSessionLost();
    });

    expect(screen.getByTestId('status')).toHaveTextContent('anonymous');
    expect(loadPersistedRefreshToken()).toBeNull();
  });

  // ADR 0010 rotates the refresh token on every use and revokes the family when
  // an already-rotated one is replayed, so a stored copy that misses a rotation
  // is worse than useless: its next use is indistinguishable from a replay
  // attack. The dangerous rotation is the one nobody asked for — an access
  // token expiring during ordinary use, rotated entirely inside session.ts.
  it('persists the rotation that happens inside a refresh, not just at login', async () => {
    window.sessionStorage.setItem('markhand.refreshToken', 'stored-refresh');
    const { client, fireRotation, hasRotationListener } = createFakeClient();
    (client.me as ReturnType<typeof vi.fn>).mockResolvedValue(SAMPLE_ME);

    render(
      <ScopeProvider>
        <AuthProvider client={client}>
          <Probe />
        </AuthProvider>
      </ScopeProvider>,
    );
    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('authenticated'));
    expect(hasRotationListener()).toBe(true);

    act(() => {
      fireRotation({
        accessToken: 'access-rotated',
        refreshToken: 'refresh-rotated',
        tokenType: 'Bearer',
        expiresIn: 900,
        orgId: SAMPLE_ME.orgId,
        userId: SAMPLE_ME.userId,
      });
    });

    expect(loadPersistedRefreshToken()).toBe('refresh-rotated');
    expect(screen.getByTestId('status')).toHaveTextContent('authenticated');
  });
});

describe('AuthContext / scope integration', () => {
  it('sets the scope on login and clears it on logout (per ScopeProvider.tsx handoff)', async () => {
    window.sessionStorage.setItem('markhand.refreshToken', 'stored-refresh');
    const { client } = createFakeClient();
    (client.me as ReturnType<typeof vi.fn>).mockResolvedValue(SAMPLE_ME);

    function Harness() {
      const { session, logout } = useAuth();
      return (
        <>
          <span data-testid="status">{session.status}</span>
          <button onClick={() => void logout()}>bye</button>
        </>
      );
    }

    render(
      <ScopeProvider>
        <AuthProvider client={client}>
          <Harness />
          <ScopeProbe />
        </AuthProvider>
      </ScopeProvider>,
    );

    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('authenticated'));
    expect(screen.getByTestId('scope-org')).toHaveTextContent('org-1');

    await act(async () => {
      screen.getByText('bye').click();
      await Promise.resolve();
    });
    expect(screen.getByTestId('scope-org')).toHaveTextContent('null');
  });
});

describe('AuthContext switchOrg (P2-06/P2-15 org switch)', () => {
  const ORG_B_ME = {
    ...SAMPLE_ME,
    orgId: 'org-2',
    permissions: ['org-b.permission'],
    allowedCollectionIds: ['col-b'],
  };

  function orgBTokens(): SessionTokens {
    return sampleTokens({
      accessToken: 'access-org-b',
      refreshToken: 'refresh-org-b',
      orgId: 'org-2',
    });
  }

  let lastSwitchError: unknown;

  function Harness() {
    const { session, switchOrg } = useAuth();
    const { scope } = useScope();
    return (
      <>
        <span data-testid="status">{session.status}</span>
        <span data-testid="scope-org">{scope ? scope.orgId : 'null'}</span>
        <button
          onClick={() => {
            switchOrg('org-2').catch((e) => {
              lastSwitchError = e;
            });
          }}
        >
          switch
        </button>
      </>
    );
  }

  async function renderAuthenticated(client: ApiClient) {
    render(
      <ScopeProvider>
        <AuthProvider client={client}>
          <Harness />
        </AuthProvider>
      </ScopeProvider>,
    );
    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('authenticated'));
  }

  it('swaps tokens atomically, re-applies me(), and bumps the scope to the new org', async () => {
    window.sessionStorage.setItem('markhand.refreshToken', 'stored-refresh');
    const { client, setTokensCalls } = createFakeClient();
    (client.me as ReturnType<typeof vi.fn>)
      .mockResolvedValueOnce(SAMPLE_ME) // bootstrap
      .mockResolvedValueOnce(ORG_B_ME); // post-switch
    (client.request as ReturnType<typeof vi.fn>).mockResolvedValue(orgBTokens());

    await renderAuthenticated(client);
    expect(screen.getByTestId('scope-org')).toHaveTextContent('org-1');

    await act(async () => {
      screen.getByText('switch').click();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(client.request).toHaveBeenCalledWith('post', '/orgs/switch', {
      body: { orgId: 'org-2' },
    });
    // The new pair replaces the old one in one call — never a separate
    // "clear, then set" pair of calls a concurrent reader could land between.
    expect(setTokensCalls[setTokensCalls.length - 1]).toMatchObject({
      orgId: 'org-2',
      accessToken: 'access-org-b',
    });
    await waitFor(() => expect(screen.getByTestId('scope-org')).toHaveTextContent('org-2'));
    expect(screen.getByTestId('status')).toHaveTextContent('authenticated');
  });

  it('leaves session/scope on the previous org when the switch request itself is denied', async () => {
    window.sessionStorage.setItem('markhand.refreshToken', 'stored-refresh');
    const { client } = createFakeClient();
    (client.me as ReturnType<typeof vi.fn>).mockResolvedValue(SAMPLE_ME);
    (client.request as ReturnType<typeof vi.fn>).mockRejectedValue(
      new HttpApiError({
        status: 403,
        code: 'membership_missing',
        message: 'not a member',
        requestId: 'r1',
      }),
    );

    await renderAuthenticated(client);
    lastSwitchError = undefined;

    await act(async () => {
      screen.getByText('switch').click();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(lastSwitchError).toBeInstanceOf(HttpApiError);
    expect(screen.getByTestId('status')).toHaveTextContent('authenticated');
    expect(screen.getByTestId('scope-org')).toHaveTextContent('org-1');
  });

  it('a switch that succeeds but whose follow-up me() fails ends up anonymous, not half-switched', async () => {
    window.sessionStorage.setItem('markhand.refreshToken', 'stored-refresh');
    const { client } = createFakeClient();
    (client.me as ReturnType<typeof vi.fn>)
      .mockResolvedValueOnce(SAMPLE_ME) // bootstrap
      .mockRejectedValueOnce(new Error('me failed after switch'));
    (client.request as ReturnType<typeof vi.fn>).mockResolvedValue(orgBTokens());

    await renderAuthenticated(client);

    await act(async () => {
      screen.getByText('switch').click();
      await Promise.resolve();
      await Promise.resolve();
      await Promise.resolve();
    });

    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('anonymous'));
    expect(screen.getByTestId('scope-org')).toHaveTextContent('null');
  });

  it('a rapid second switchOrg supersedes the first — the first stale resolution never applies', async () => {
    window.sessionStorage.setItem('markhand.refreshToken', 'stored-refresh');
    const { client } = createFakeClient();
    const meCalls: Array<{ resolve: (v: typeof SAMPLE_ME) => void }> = [];
    let meInvocations = 0;
    (client.me as ReturnType<typeof vi.fn>).mockImplementation(() => {
      meInvocations += 1;
      if (meInvocations === 1) return Promise.resolve(SAMPLE_ME); // bootstrap
      const pending = deferred<typeof SAMPLE_ME>();
      meCalls.push(pending);
      return pending.promise;
    });
    let switchCall = 0;
    (client.request as ReturnType<typeof vi.fn>).mockImplementation(() => {
      switchCall += 1;
      const call = switchCall;
      return Promise.resolve(
        sampleTokens({
          accessToken: `access-${call}`,
          refreshToken: `refresh-${call}`,
          orgId: `org-${call + 1}`,
        }),
      );
    });

    function RaceHarness() {
      const { switchOrg } = useAuth();
      const { scope } = useScope();
      return (
        <>
          <span data-testid="scope-org">{scope ? scope.orgId : 'null'}</span>
          <button onClick={() => void switchOrg('org-2').catch(() => {})}>first</button>
          <button onClick={() => void switchOrg('org-3').catch(() => {})}>second</button>
        </>
      );
    }

    render(
      <ScopeProvider>
        <AuthProvider client={client}>
          <RaceHarness />
        </AuthProvider>
      </ScopeProvider>,
    );
    await waitFor(() => expect(screen.getByTestId('scope-org')).toHaveTextContent('org-1'));

    await act(async () => {
      screen.getByText('first').click();
      await Promise.resolve();
      await Promise.resolve();
      screen.getByText('second').click();
      await Promise.resolve();
      await Promise.resolve();
    });

    // Both me() calls (for "first" and "second") are now pending; resolve the
    // FIRST one (the superseded one) first, out of order, to prove it's
    // discarded rather than merely "usually resolves last".
    expect(meCalls.length).toBe(2);
    await act(async () => {
      meCalls[0].resolve({ ...SAMPLE_ME, orgId: 'org-2' });
      await Promise.resolve();
    });
    expect(screen.getByTestId('scope-org')).not.toHaveTextContent('org-2');

    await act(async () => {
      meCalls[1].resolve({ ...SAMPLE_ME, orgId: 'org-3' });
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(screen.getByTestId('scope-org')).toHaveTextContent('org-3');
  });
});

describe('AuthContext end-to-end against the real API client + mock server', () => {
  beforeEach(() => {
    installMockFetch();
    resetMockState();
    mockControl.reset();
  });

  it('logs in through the real mock server, persists the refresh token, and a fresh provider restores the session from it on "reload"', async () => {
    const firstLoadClient = createApiClient({ baseUrl: '' });
    render(
      <ScopeProvider>
        <AuthProvider client={firstLoadClient}>
          <LoginHarness />
        </AuthProvider>
      </ScopeProvider>,
    );

    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('anonymous'));
    await act(async () => {
      screen.getByText('go').click();
    });
    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('authenticated'));
    expect(loadPersistedRefreshToken()).not.toBeNull();

    cleanup();

    // Simulate a reload: fresh AuthProvider around a brand-new `ApiClient`
    // instance (a real reload creates a fresh module instance too — nothing
    // is reused from `firstLoadClient`), with only the persisted refresh
    // token surviving (the access token is memory-only and is gone, as it
    // should be).
    const reloadedClient = createApiClient({ baseUrl: '' });

    render(
      <ScopeProvider>
        <AuthProvider client={reloadedClient}>
          <Probe />
        </AuthProvider>
      </ScopeProvider>,
    );

    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('authenticated'));
  });

  it('a rejected stored refresh token (401) lands on anonymous and clears storage', async () => {
    window.sessionStorage.setItem('markhand.refreshToken', 'not-a-real-refresh-token');
    const client = createApiClient({ baseUrl: '' });

    render(
      <ScopeProvider>
        <AuthProvider client={client}>
          <Probe />
        </AuthProvider>
      </ScopeProvider>,
    );

    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('anonymous'));
    expect(loadPersistedRefreshToken()).toBeNull();
  });

  it('switchOrg() against the real mock server moves scope/permissions to org B and persists the new refresh token', async () => {
    const client = createApiClient({ baseUrl: '' });
    render(
      <ScopeProvider>
        <AuthProvider client={client}>
          <LoginHarness />
          <SwitchHarness />
        </AuthProvider>
      </ScopeProvider>,
    );
    await act(async () => {
      screen.getByText('go').click();
    });
    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('authenticated'));
    expect(screen.getByTestId('scope-org')).toHaveTextContent(ORG_A_ID);
    const refreshBeforeSwitch = loadPersistedRefreshToken();

    await act(async () => {
      screen.getByText('switch-to-b').click();
    });

    await waitFor(() => expect(screen.getByTestId('scope-org')).toHaveTextContent(ORG_B_ID));
    expect(screen.getByTestId('status')).toHaveTextContent('authenticated');
    // A genuinely different, freshly-persisted refresh token for the new org
    // family — not the pre-switch org A token still sitting there.
    expect(loadPersistedRefreshToken()).not.toBe(refreshBeforeSwitch);
  });

  it('switchOrg() to an org the caller is not an active member of is denied (membership_missing) and leaves org A active', async () => {
    const client = createApiClient({ baseUrl: '' });
    render(
      <ScopeProvider>
        <AuthProvider client={client}>
          <LoginHarness />
          <SwitchHarness targetOrgId="00000000-0000-4000-8000-0000000000ff" />
        </AuthProvider>
      </ScopeProvider>,
    );
    await act(async () => {
      screen.getByText('go').click();
    });
    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('authenticated'));

    await act(async () => {
      screen.getByText('switch-to-b').click();
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(screen.getByTestId('switch-error')).toHaveTextContent('membership_missing');
    expect(screen.getByTestId('scope-org')).toHaveTextContent(ORG_A_ID);
    expect(screen.getByTestId('status')).toHaveTextContent('authenticated');
  });
});

function LoginHarness() {
  const { session, login } = useAuth();
  return (
    <>
      <span data-testid="status">{session.status}</span>
      <button onClick={() => void login(DEMO_EMAIL, DEMO_PASSWORD)}>go</button>
    </>
  );
}

/** `ORG_B_ID` unless `targetOrgId` overrides it (the "switch to an org the caller isn't in" test). */
function SwitchHarness({ targetOrgId }: { targetOrgId?: string } = {}) {
  const { switchOrg } = useAuth();
  const { scope } = useScope();
  const [error, setError] = useState<unknown>(undefined);
  return (
    <>
      <span data-testid="scope-org">{scope ? scope.orgId : 'null'}</span>
      <span data-testid="switch-error">{error instanceof HttpApiError ? error.code : ''}</span>
      <button
        onClick={() => {
          switchOrg(targetOrgId ?? ORG_B_ID).catch((e) => setError(e));
        }}
      >
        switch-to-b
      </button>
    </>
  );
}
