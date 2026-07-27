import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { App } from './App';
import { installMockFetch, resetMockState, uninstallMockFetch } from './mocks';

const DEMO_EMAIL = 'demo@markhand.test';
const DEMO_PASSWORD = 'demo-password';

function fillAndSubmitLogin(email: string, password: string) {
  fireEvent.change(screen.getByLabelText('Email'), { target: { value: email } });
  fireEvent.change(screen.getByLabelText('Mật khẩu'), { target: { value: password } });
  fireEvent.click(screen.getByRole('button', { name: 'Đăng nhập' }));
}

describe('App', () => {
  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
    window.history.pushState(null, '', '/');
  });

  it('renders readiness from the real API contract', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(
        new Response(
          JSON.stringify({ status: 'ok', requestId: '5b435d32-20a3-47c0-a615-aa0b9c5bcd28' }),
          {
            status: 200,
            headers: { 'Content-Type': 'application/json' },
          },
        ),
      ),
    );

    render(<App />);

    expect(
      screen.getByRole('heading', { name: 'Không gian làm việc đã sẵn sàng để kết nối.' }),
    ).toBeVisible();
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('Đã kết nối máy chủ'));
    expect(screen.getByText('5b435d32-20a3-47c0-a615-aa0b9c5bcd28')).toBeVisible();
    expect(fetch).toHaveBeenCalledWith(
      '/api/v1/health/ready',
      expect.objectContaining({ headers: { Accept: 'application/json' } }),
    );
  });

  it('shows a recoverable state when the backend is unavailable', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(null, { status: 503 })));

    render(<App />);

    await waitFor(() =>
      expect(screen.getByRole('status')).toHaveTextContent('Máy chủ chưa sẵn sàng'),
    );
    expect(screen.getByRole('button', { name: 'Kiểm tra kết nối' })).toBeVisible();
  });

  it('navigates to a P2.3 route by clicking the primary nav link', () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(null, { status: 503 })));

    render(<App />);
    fireEvent.click(screen.getByRole('link', { name: 'Trợ giúp' }));

    expect(window.location.pathname).toBe('/help');
    expect(screen.getByRole('heading', { name: 'Trợ giúp Markhand' })).toBeVisible();
  });

  it('redirects a deep-linked protected route to /login when anonymous, preserving it as ?next=', async () => {
    window.history.pushState(null, '', '/library/col-42');
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(null, { status: 503 })));

    render(<App />);

    await waitFor(() =>
      expect(window.location.pathname + window.location.search).toBe(
        '/login?next=%2Flibrary%2Fcol-42',
      ),
    );
    expect(screen.getByRole('heading', { name: 'Đăng nhập vào Markhand' })).toBeVisible();
    expect(screen.queryByRole('heading', { name: 'Bộ sưu tập col-42' })).not.toBeInTheDocument();
  });

  it('shows a not-found page for an unknown path and links back home', () => {
    window.history.pushState(null, '', '/does-not-exist');
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(null, { status: 503 })));

    render(<App />);

    expect(screen.getByRole('heading', { name: 'Không tìm thấy trang' })).toBeVisible();
    fireEvent.click(screen.getByRole('link', { name: 'Về trang chính' }));
    expect(window.location.pathname).toBe('/');
  });
});

describe('App / authenticated shell (P2-05 guard matrix + login/logout)', () => {
  beforeEach(() => {
    installMockFetch();
    resetMockState();
  });

  afterEach(() => {
    cleanup();
    uninstallMockFetch();
    window.sessionStorage.clear();
    window.history.pushState(null, '', '/');
  });

  it('logs in through the real mock server, lands on the intended deep-linked route, and shows session/org in the topbar', async () => {
    window.history.pushState(null, '', '/library/col-42');
    render(<App />);

    await waitFor(() =>
      expect(screen.getByRole('heading', { name: 'Đăng nhập vào Markhand' })).toBeVisible(),
    );
    expect(window.location.search).toBe('?next=%2Flibrary%2Fcol-42');

    fillAndSubmitLogin(DEMO_EMAIL, DEMO_PASSWORD);

    await waitFor(() =>
      expect(screen.getByRole('heading', { name: 'Bộ sưu tập col-42' })).toBeVisible(),
    );
    expect(screen.getByText('Demo User', { exact: false })).toBeVisible();
    expect(screen.getByRole('button', { name: 'Đăng xuất' })).toBeVisible();
  });

  it('rejects the wrong password and never renders the protected shell', async () => {
    window.history.pushState(null, '', '/library');
    render(<App />);
    await waitFor(() => expect(window.location.pathname).toBe('/login'));

    fillAndSubmitLogin(DEMO_EMAIL, 'not-the-password');

    await waitFor(() => expect(screen.getByText('Email hoặc mật khẩu không đúng.')).toBeVisible());
    expect(window.location.pathname).toBe('/login');
    expect(screen.queryByRole('button', { name: 'Đăng xuất' })).not.toBeInTheDocument();
  });

  it('logs out back to /login without leaving a half-authenticated shell', async () => {
    window.history.pushState(null, '', '/library');
    render(<App />);
    await waitFor(() => expect(window.location.pathname).toBe('/login'));

    fillAndSubmitLogin(DEMO_EMAIL, DEMO_PASSWORD);
    await waitFor(() => expect(screen.getByRole('button', { name: 'Đăng xuất' })).toBeVisible());

    fireEvent.click(screen.getByRole('button', { name: 'Đăng xuất' }));

    await waitFor(() => expect(window.location.pathname).toBe('/login'));
    expect(screen.queryByRole('button', { name: 'Đăng xuất' })).not.toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Đăng nhập vào Markhand' })).toBeVisible();
  });

  it('renders an in-shell notice — not the admin page, not a redirect — for a signed-in user without member.manage', async () => {
    window.history.pushState(null, '', '/admin/members');
    render(<App />);
    await waitFor(() => expect(window.location.pathname).toBe('/login'));

    fillAndSubmitLogin(DEMO_EMAIL, DEMO_PASSWORD);

    await waitFor(() => expect(screen.getByText(/không có quyền/)).toBeVisible());
    expect(
      screen.queryByRole('heading', { name: 'Thành viên và vai trò' }),
    ).not.toBeInTheDocument();
    // Still on the admin URL — this is a render decision, the server remains the authority.
    expect(window.location.pathname).toBe('/admin/members');
  });

  it('restores the session on unmount/remount without forcing a re-login (real page-reload-from-a-fresh-client case is covered in auth/AuthContext.test.tsx)', async () => {
    window.history.pushState(null, '', '/library');
    render(<App />);
    await waitFor(() => expect(window.location.pathname).toBe('/login'));
    fillAndSubmitLogin(DEMO_EMAIL, DEMO_PASSWORD);
    await waitFor(() => expect(screen.getByRole('button', { name: 'Đăng xuất' })).toBeVisible());

    // `App.tsx` always uses the shared `apiClient` singleton, so unmounting
    // and remounting here does not clear its in-memory tokens the way an
    // actual page reload (a brand-new JS heap) would — this only asserts the
    // shell doesn't second-guess a still-valid persisted session on remount.
    cleanup();
    window.history.pushState(null, '', '/library');
    render(<App />);

    await waitFor(() => expect(screen.getByRole('button', { name: 'Đăng xuất' })).toBeVisible());
    expect(window.location.pathname).toBe('/library');
  });
});
