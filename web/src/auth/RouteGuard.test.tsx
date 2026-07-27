import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ApiClient } from '../api/client';
import type { SessionTokens } from '../api/session';
import { RouterProvider } from '../state/RouterProvider';
import { ScopeProvider } from '../state/ScopeProvider';
import { AuthProvider } from './AuthContext';
import { ProtectedRoute, PublicOnlyRoute, sanitizeNextPath } from './RouteGuard';

const SAMPLE_ME = {
  userId: 'user-1',
  orgId: 'org-1',
  email: 'demo@markhand.test',
  displayName: 'Demo User',
  permissions: ['qa.history'],
  allowedCollectionIds: ['col-1'],
  sessionId: 'session-1',
};

/** Same shape of fake client as AuthContext.test.tsx, trimmed to what these guard tests drive. */
function createFakeClient(): ApiClient {
  let currentRefreshToken: string | null = null;
  return {
    login: vi.fn(),
    logout: vi.fn(async () => {}),
    me: vi.fn(),
    request: vi.fn(),
    tokenProvider: {
      getAccessToken: vi.fn(),
      refreshNow: vi.fn(),
      onSessionLost: () => () => {},
    },
    sessionManager: {
      setTokens: vi.fn((tokens: SessionTokens) => {
        currentRefreshToken = tokens.refreshToken;
      }),
      clear: vi.fn(() => {
        currentRefreshToken = null;
      }),
      hasSession: vi.fn(() => currentRefreshToken !== null),
      getRefreshTokenForLogout: vi.fn(() => currentRefreshToken),
      getAccessToken: vi.fn(),
      refreshNow: vi.fn(),
      onSessionLost: () => () => {},
      onTokensRotated: () => () => {},
    },
  } as unknown as ApiClient;
}

function renderWithProviders(children: ReactNode, client: ApiClient) {
  return render(
    <RouterProvider>
      <ScopeProvider>
        <AuthProvider client={client}>{children}</AuthProvider>
      </ScopeProvider>
    </RouterProvider>,
  );
}

afterEach(() => {
  cleanup();
  window.sessionStorage.clear();
  window.history.pushState(null, '', '/');
});

describe('sanitizeNextPath', () => {
  it('falls back for null/empty input', () => {
    expect(sanitizeNextPath(null)).toBe('/library');
    expect(sanitizeNextPath(null, '/help')).toBe('/help');
  });

  it('accepts a same-origin root-relative path', () => {
    expect(sanitizeNextPath('/qa/col-42')).toBe('/qa/col-42');
  });

  it('preserves a query string on the sanitized path', () => {
    expect(sanitizeNextPath('/library?foo=bar')).toBe('/library?foo=bar');
  });

  it('rejects a foreign absolute URL (open-redirect guard)', () => {
    expect(sanitizeNextPath('https://evil.example/steal')).toBe('/library');
  });

  it('rejects a protocol-relative URL', () => {
    expect(sanitizeNextPath('//evil.example/steal')).toBe('/library');
  });

  it('falls back for an unparsable value', () => {
    expect(sanitizeNextPath('javascript:alert(1)')).toBe('/library');
  });
});

describe('ProtectedRoute', () => {
  it('shows a neutral loading state while the session is checking', async () => {
    window.sessionStorage.setItem('markhand.refreshToken', 'stored-refresh');
    const client = createFakeClient();
    (client.me as ReturnType<typeof vi.fn>).mockReturnValue(new Promise(() => {})); // never resolves

    renderWithProviders(
      <ProtectedRoute>
        <p>secret</p>
      </ProtectedRoute>,
      client,
    );

    expect(screen.getByRole('status')).toHaveTextContent('Đang kiểm tra phiên đăng nhập');
    expect(screen.queryByText('secret')).not.toBeInTheDocument();
  });

  it('renders children once authenticated', async () => {
    window.sessionStorage.setItem('markhand.refreshToken', 'stored-refresh');
    const client = createFakeClient();
    (client.me as ReturnType<typeof vi.fn>).mockResolvedValue(SAMPLE_ME);

    renderWithProviders(
      <ProtectedRoute>
        <p>secret</p>
      </ProtectedRoute>,
      client,
    );

    await waitFor(() => expect(screen.getByText('secret')).toBeVisible());
  });

  it('redirects an anonymous visitor to /login with ?next= set to the current path', async () => {
    window.history.pushState(null, '', '/library/col-9');
    const client = createFakeClient();

    renderWithProviders(
      <ProtectedRoute>
        <p>secret</p>
      </ProtectedRoute>,
      client,
    );

    await waitFor(() =>
      expect(window.location.pathname + window.location.search).toBe(
        '/login?next=%2Flibrary%2Fcol-9',
      ),
    );
    expect(screen.queryByText('secret')).not.toBeInTheDocument();
  });

  it('renders an in-shell notice (not a redirect) when authenticated but missing the required permission', async () => {
    window.sessionStorage.setItem('markhand.refreshToken', 'stored-refresh');
    const client = createFakeClient();
    (client.me as ReturnType<typeof vi.fn>).mockResolvedValue(SAMPLE_ME); // has only 'qa.history'

    renderWithProviders(
      <ProtectedRoute permission="member.manage">
        <p>secret</p>
      </ProtectedRoute>,
      client,
    );

    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent(/không có quyền/));
    expect(screen.queryByText('secret')).not.toBeInTheDocument();
    // Still on the same URL — this is a render decision, not a redirect.
    expect(window.location.pathname).toBe('/');
  });

  it('renders children when authenticated and the required permission is present', async () => {
    window.sessionStorage.setItem('markhand.refreshToken', 'stored-refresh');
    const client = createFakeClient();
    (client.me as ReturnType<typeof vi.fn>).mockResolvedValue(SAMPLE_ME); // has 'qa.history'

    renderWithProviders(
      <ProtectedRoute permission="qa.history">
        <p>secret</p>
      </ProtectedRoute>,
      client,
    );

    await waitFor(() => expect(screen.getByText('secret')).toBeVisible());
  });
});

describe('PublicOnlyRoute', () => {
  it('renders children (the login form) when anonymous', async () => {
    const client = createFakeClient();

    renderWithProviders(
      <PublicOnlyRoute>
        <p>login-form</p>
      </PublicOnlyRoute>,
      client,
    );

    await waitFor(() => expect(screen.getByText('login-form')).toBeVisible());
  });

  it('redirects an already-authenticated visitor away, honoring a sanitized ?next=', async () => {
    window.history.pushState(null, '', '/login?next=%2Fqa%2Fcol-1');
    window.sessionStorage.setItem('markhand.refreshToken', 'stored-refresh');
    const client = createFakeClient();
    (client.me as ReturnType<typeof vi.fn>).mockResolvedValue(SAMPLE_ME);

    renderWithProviders(
      <PublicOnlyRoute>
        <p>login-form</p>
      </PublicOnlyRoute>,
      client,
    );

    await waitFor(() => expect(window.location.pathname).toBe('/qa/col-1'));
    expect(screen.queryByText('login-form')).not.toBeInTheDocument();
  });

  it('ignores a foreign ?next= and falls back to the default destination', async () => {
    window.history.pushState(null, '', '/login?next=https%3A%2F%2Fevil.example');
    window.sessionStorage.setItem('markhand.refreshToken', 'stored-refresh');
    const client = createFakeClient();
    (client.me as ReturnType<typeof vi.fn>).mockResolvedValue(SAMPLE_ME);

    renderWithProviders(
      <PublicOnlyRoute>
        <p>login-form</p>
      </PublicOnlyRoute>,
      client,
    );

    await act(async () => {});
    await waitFor(() => expect(window.location.pathname).toBe('/library'));
  });
});
