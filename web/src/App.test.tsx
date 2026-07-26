import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { App } from './App';

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

  it('renders the library page for a deep-linked collection path', () => {
    window.history.pushState(null, '', '/library/col-42');
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(null, { status: 503 })));

    render(<App />);

    expect(screen.getByRole('heading', { name: 'Bộ sưu tập col-42' })).toBeVisible();
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
