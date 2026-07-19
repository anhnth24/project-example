import { cleanup, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { App } from './App';

describe('App', () => {
  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
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
      screen.getByRole('heading', { name: 'Your workspace is ready to connect.' }),
    ).toBeVisible();
    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('Server connected'));
    expect(screen.getByText('5b435d32-20a3-47c0-a615-aa0b9c5bcd28')).toBeVisible();
    expect(fetch).toHaveBeenCalledWith(
      '/api/v1/health/ready',
      expect.objectContaining({ headers: { Accept: 'application/json' } }),
    );
  });

  it('shows a recoverable state when the backend is unavailable', async () => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(null, { status: 503 })));

    render(<App />);

    await waitFor(() => expect(screen.getByRole('status')).toHaveTextContent('Server unavailable'));
    expect(screen.getByRole('button', { name: 'Check connection' })).toBeVisible();
  });
});
