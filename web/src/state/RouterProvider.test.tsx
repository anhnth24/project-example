import { act, cleanup, render, renderHook, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import { RouterProvider, useRouter } from './RouterProvider';

afterEach(() => {
  cleanup();
  window.history.pushState(null, '', '/');
});

function Probe() {
  const { pathname, match, navigate } = useRouter();
  return (
    <div>
      <span data-testid="pathname">{pathname}</span>
      <span data-testid="route">{match.name}</span>
      <button type="button" onClick={() => navigate('/help')}>
        go help
      </button>
    </div>
  );
}

describe('RouterProvider', () => {
  it('throws when useRouter is used outside a provider', () => {
    const { result } = renderHook(() => {
      try {
        return useRouter();
      } catch (error) {
        return error;
      }
    });
    expect(result.current).toBeInstanceOf(Error);
  });

  it('reflects the current path and matched route', () => {
    window.history.pushState(null, '', '/qa/col-1');
    render(
      <RouterProvider>
        <Probe />
      </RouterProvider>,
    );
    expect(screen.getByTestId('pathname')).toHaveTextContent('/qa/col-1');
    expect(screen.getByTestId('route')).toHaveTextContent('qa');
  });

  it('navigates via history.pushState and updates the match', () => {
    render(
      <RouterProvider>
        <Probe />
      </RouterProvider>,
    );
    act(() => {
      screen.getByRole('button', { name: 'go help' }).click();
    });
    expect(window.location.pathname).toBe('/help');
    expect(screen.getByTestId('route')).toHaveTextContent('help');
  });

  it('reacts to back/forward navigation via popstate', () => {
    render(
      <RouterProvider>
        <Probe />
      </RouterProvider>,
    );
    act(() => {
      window.history.pushState(null, '', '/admin/usage');
      window.dispatchEvent(new PopStateEvent('popstate'));
    });
    expect(screen.getByTestId('route')).toHaveTextContent('adminUsage');
  });
});
