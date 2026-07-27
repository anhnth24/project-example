import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ApiClient } from '../api/client';
import { HttpApiError } from '../api/errors';
import type { SessionTokens } from '../api/session';
import { installMockFetch, mockControl, resetMockState, uninstallMockFetch } from '../mocks';
import { createApiClient } from '../api/client';
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
