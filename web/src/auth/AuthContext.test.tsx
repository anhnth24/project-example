import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import { AuthProvider, useAuth } from './AuthContext';

afterEach(() => {
  cleanup();
});

function Probe() {
  const { session } = useAuth();
  return <span data-testid="status">{session.status}</span>;
}

describe('AuthContext', () => {
  it('defaults to an anonymous session', () => {
    render(
      <AuthProvider>
        <Probe />
      </AuthProvider>,
    );
    expect(screen.getByTestId('status')).toHaveTextContent('anonymous');
  });

  it('exposes an authenticated session when provided', () => {
    render(
      <AuthProvider session={{ status: 'authenticated', email: 'a@example.com', role: 'admin' }}>
        <Probe />
      </AuthProvider>,
    );
    expect(screen.getByTestId('status')).toHaveTextContent('authenticated');
  });

  it('defaults to anonymous when useAuth is called outside a provider', () => {
    render(<Probe />);
    expect(screen.getByTestId('status')).toHaveTextContent('anonymous');
  });
});
