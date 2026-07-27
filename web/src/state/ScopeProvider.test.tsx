import { act, cleanup, render, renderHook, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vitest';
import { createScopeManager } from './scope';
import { ScopeProvider, useScope } from './ScopeProvider';

afterEach(() => {
  cleanup();
});

function Probe() {
  const { epoch, scope, setScope } = useScope();
  return (
    <div>
      <span data-testid="epoch">{epoch}</span>
      <span data-testid="org">{scope?.orgId ?? 'anonymous'}</span>
      <button
        type="button"
        onClick={() => setScope({ orgId: 'org-b', permissions: [], allowedCollectionIds: [] })}
      >
        switch to B
      </button>
    </div>
  );
}

describe('ScopeProvider', () => {
  it('throws when useScope is used outside a provider', () => {
    const { result } = renderHook(() => {
      try {
        return useScope();
      } catch (error) {
        return error;
      }
    });
    expect(result.current).toBeInstanceOf(Error);
  });

  it('starts anonymous at epoch 0', () => {
    render(
      <ScopeProvider>
        <Probe />
      </ScopeProvider>,
    );
    expect(screen.getByTestId('epoch')).toHaveTextContent('0');
    expect(screen.getByTestId('org')).toHaveTextContent('anonymous');
  });

  it('re-renders subscribers with the new epoch/scope after setScope', () => {
    render(
      <ScopeProvider>
        <Probe />
      </ScopeProvider>,
    );
    act(() => {
      screen.getByRole('button', { name: 'switch to B' }).click();
    });
    expect(screen.getByTestId('epoch')).toHaveTextContent('1');
    expect(screen.getByTestId('org')).toHaveTextContent('org-b');
  });

  it('accepts an injected manager so tests can drive/observe it directly', () => {
    const manager = createScopeManager();
    render(
      <ScopeProvider manager={manager}>
        <Probe />
      </ScopeProvider>,
    );
    act(() => {
      manager.setScope({ orgId: 'org-a', permissions: [], allowedCollectionIds: [] });
    });
    expect(screen.getByTestId('org')).toHaveTextContent('org-a');
    expect(manager.getSnapshot().epoch).toBe(1);
  });
});
