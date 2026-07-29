import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { ApiClient } from '../api/client';
import { HttpApiError, NetworkError } from '../api/errors';
import { AuthProvider, useAuth } from '../auth/AuthContext';
import { ScopeProvider } from '../state/ScopeProvider';
import { LoginPage } from './LoginPage';

const SAMPLE_ME = {
  userId: 'user-1',
  orgId: 'org-1',
  email: 'demo@markhand.test',
  displayName: 'Demo User',
  permissions: ['qa.history'],
  allowedCollectionIds: ['col-1'],
  sessionId: 'session-1',
};

function createFakeClient(): ApiClient {
  return {
    login: vi.fn(),
    logout: vi.fn(async () => {}),
    me: vi.fn(),
    request: vi.fn(),
    tokenProvider: { getAccessToken: vi.fn(), refreshNow: vi.fn(), onSessionLost: () => () => {} },
    sessionManager: {
      setTokens: vi.fn(),
      clear: vi.fn(),
      hasSession: vi.fn(() => false),
      getRefreshTokenForLogout: vi.fn(() => null),
      getAccessToken: vi.fn(),
      refreshNow: vi.fn(),
      onSessionLost: () => () => {},
      onTokensRotated: () => () => {},
    },
  } as unknown as ApiClient;
}

function StatusProbe() {
  const { session } = useAuth();
  return <span data-testid="status">{session.status}</span>;
}

function renderLoginPage(client: ApiClient) {
  return render(
    <ScopeProvider>
      <AuthProvider client={client}>
        <StatusProbe />
        <LoginPage />
      </AuthProvider>
    </ScopeProvider>,
  );
}

afterEach(() => {
  cleanup();
  window.sessionStorage.clear();
});

describe('LoginPage', () => {
  it('renders email/password fields and a submit button', async () => {
    const client = createFakeClient();
    renderLoginPage(client);
    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('anonymous'));

    expect(screen.getByLabelText('Email')).toBeVisible();
    expect(screen.getByLabelText('Mật khẩu')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Đăng nhập' })).toBeVisible();
  });

  it('submits the entered credentials to login()', async () => {
    const client = createFakeClient();
    (client.login as ReturnType<typeof vi.fn>).mockResolvedValue({
      accessToken: 'a',
      refreshToken: 'r',
      tokenType: 'Bearer',
      expiresIn: 3600,
      orgId: 'org-1',
      userId: 'user-1',
    });
    (client.me as ReturnType<typeof vi.fn>).mockResolvedValue(SAMPLE_ME);
    renderLoginPage(client);
    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('anonymous'));

    fireEvent.change(screen.getByLabelText('Email'), { target: { value: 'demo@markhand.test' } });
    fireEvent.change(screen.getByLabelText('Mật khẩu'), { target: { value: 'demo-password' } });
    fireEvent.click(screen.getByRole('button', { name: 'Đăng nhập' }));

    await waitFor(() =>
      // A third `AbortSignal` argument now rides along (`AuthContext.tsx`'s
      // `login()` passes its own epoch-scoped controller so a superseded
      // login's request can be cancelled outright) — assert on credentials
      // only, not on the exact signal instance.
      expect(client.login).toHaveBeenCalledWith(
        { email: 'demo@markhand.test', password: 'demo-password' },
        expect.any(AbortSignal),
      ),
    );
    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('authenticated'));
  });

  it('shows a Vietnamese message for wrong credentials (401) and re-enables the form', async () => {
    const client = createFakeClient();
    (client.login as ReturnType<typeof vi.fn>).mockRejectedValue(
      new HttpApiError({
        status: 401,
        code: 'unauthorized',
        message: 'Email or password is incorrect.',
        requestId: 'r1',
      }),
    );
    renderLoginPage(client);
    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('anonymous'));

    fireEvent.change(screen.getByLabelText('Email'), { target: { value: 'demo@markhand.test' } });
    fireEvent.change(screen.getByLabelText('Mật khẩu'), { target: { value: 'wrong' } });
    fireEvent.click(screen.getByRole('button', { name: 'Đăng nhập' }));

    await waitFor(() => expect(screen.getByText('Email hoặc mật khẩu không đúng.')).toBeVisible());
    expect(screen.getByRole('button', { name: 'Đăng nhập' })).not.toBeDisabled();
  });

  it('shows a generic message for a network failure', async () => {
    const client = createFakeClient();
    (client.login as ReturnType<typeof vi.fn>).mockRejectedValue(
      new NetworkError('Network request failed'),
    );
    renderLoginPage(client);
    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('anonymous'));

    fireEvent.change(screen.getByLabelText('Email'), { target: { value: 'demo@markhand.test' } });
    fireEvent.change(screen.getByLabelText('Mật khẩu'), { target: { value: 'demo-password' } });
    fireEvent.click(screen.getByRole('button', { name: 'Đăng nhập' }));

    await waitFor(() =>
      expect(
        screen.getByText('Không thể kết nối máy chủ. Kiểm tra kết nối mạng và thử lại.'),
      ).toBeVisible(),
    );
  });

  it('disables the form while a login is pending', async () => {
    const client = createFakeClient();
    let resolveLogin!: (value: unknown) => void;
    (client.login as ReturnType<typeof vi.fn>).mockReturnValue(
      new Promise((resolve) => {
        resolveLogin = resolve;
      }),
    );
    renderLoginPage(client);
    await waitFor(() => expect(screen.getByTestId('status')).toHaveTextContent('anonymous'));

    fireEvent.change(screen.getByLabelText('Email'), { target: { value: 'demo@markhand.test' } });
    fireEvent.change(screen.getByLabelText('Mật khẩu'), { target: { value: 'demo-password' } });
    fireEvent.click(screen.getByRole('button', { name: 'Đăng nhập' }));

    await waitFor(() => expect(screen.getByRole('button', { name: 'Đăng nhập' })).toBeDisabled());
    expect(screen.getByLabelText('Email')).toBeDisabled();
    expect(screen.getByLabelText('Mật khẩu')).toBeDisabled();

    resolveLogin({
      accessToken: 'a',
      refreshToken: 'r',
      tokenType: 'Bearer',
      expiresIn: 3600,
      orgId: 'org-1',
      userId: 'user-1',
    });
  });
});
