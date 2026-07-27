import { act, render, renderHook, waitFor } from '@testing-library/react';
import { StrictMode, useEffect } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { createScopeManager, type Scope, type ScopeManager } from '../../state/scope';
import { ScopeProvider } from '../../state/ScopeProvider';
import { useSingleFlightAction } from './useSingleFlightAction';

afterEach(() => {
  vi.restoreAllMocks();
});

function scope(orgId: string): Scope {
  return { orgId, permissions: [], allowedCollectionIds: [] };
}

function wrapperFor(manager: ScopeManager, strict = false) {
  return function Wrapper({ children }: { children: React.ReactNode }) {
    const tree = <ScopeProvider manager={manager}>{children}</ScopeProvider>;
    return strict ? <StrictMode>{tree}</StrictMode> : tree;
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

describe('useSingleFlightAction', () => {
  it('starts idle and never calls run until dispatch', async () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-a'));
    const run = vi.fn(async () => 'done');

    const { result } = renderHook(() => useSingleFlightAction<string>(), {
      wrapper: wrapperFor(manager),
    });

    expect(result.current.phase).toBe('idle');
    expect(run).not.toHaveBeenCalled();
  });

  it('goes idle -> pending -> success and exposes the resolved value', async () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-a'));
    const gate = deferred<string>();
    const run = vi.fn(() => gate.promise);

    const { result } = renderHook(() => useSingleFlightAction<string>(), {
      wrapper: wrapperFor(manager),
    });

    act(() => {
      const started = result.current.dispatch('download', run);
      expect(started).toBe(true);
    });
    expect(result.current.phase).toBe('pending');
    expect(run).toHaveBeenCalledOnce();

    await act(async () => {
      gate.resolve('bytes-saved');
      await Promise.resolve();
      await Promise.resolve();
    });

    await waitFor(() => expect(result.current.phase).toBe('success'));
    expect(result.current.value).toBe('bytes-saved');
    expect(result.current.kind).toBe('download');
  });

  it('surfaces a rejection as phase "error" with the thrown value', async () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-a'));
    const failure = new Error('server said no');
    const run = vi.fn(() => Promise.reject(failure));

    const { result } = renderHook(() => useSingleFlightAction<string>(), {
      wrapper: wrapperFor(manager),
    });

    act(() => {
      result.current.dispatch('delete', run);
    });

    await waitFor(() => expect(result.current.phase).toBe('error'));
    expect(result.current.error).toBe(failure);
  });

  it('dispatch is a no-op while an action is already pending (reentrancy guard)', async () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-a'));
    const gate = deferred<string>();
    const firstRun = vi.fn(() => gate.promise);
    const secondRun = vi.fn(async () => 'second');

    const { result } = renderHook(() => useSingleFlightAction<string>(), {
      wrapper: wrapperFor(manager),
    });

    act(() => {
      expect(result.current.dispatch('reindex', firstRun)).toBe(true);
    });
    expect(result.current.phase).toBe('pending');

    act(() => {
      // A second dispatch while the first is still in flight must be rejected
      // synchronously — this is the guard `DocumentRowActions` relies on so a
      // same-tick double click cannot start a second network call.
      expect(result.current.dispatch('reindex', secondRun)).toBe(false);
    });
    expect(secondRun).not.toHaveBeenCalled();

    await act(async () => {
      gate.resolve('first-only');
      await Promise.resolve();
      await Promise.resolve();
    });
    await waitFor(() => expect(result.current.phase).toBe('success'));
    expect(result.current.value).toBe('first-only');

    // Once settled, a fresh dispatch is allowed again.
    act(() => {
      expect(result.current.dispatch('reindex', secondRun)).toBe(true);
    });
    await waitFor(() => expect(result.current.value).toBe('second'));
  });

  it('reset() returns to idle and allows a brand new dispatch', async () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-a'));
    const run = vi.fn(async () => 'ok');

    const { result } = renderHook(() => useSingleFlightAction<string>(), {
      wrapper: wrapperFor(manager),
    });

    act(() => {
      result.current.dispatch('reindex', run);
    });
    await waitFor(() => expect(result.current.phase).toBe('success'));

    act(() => result.current.reset());
    expect(result.current.phase).toBe('idle');
    expect(result.current.kind).toBeNull();
  });

  it('a mount-time dispatch survives StrictMode double-invoking that mount effect: run() still fires exactly once', async () => {
    // This is the one place a real (not simulated) React 18 StrictMode
    // mount-time double-invoke is reachable: a component that dispatches
    // from a `useEffect(..., [])` on its own mount has *that* effect
    // double-invoked by StrictMode (mount -> cleanup -> mount, dev-only),
    // so `dispatch` really is called twice from the same closure before
    // either ticket has settled. `DocumentRowActions` itself never
    // dispatches at mount (only from a click, after mount — see the hook's
    // module doc for why that path is not reachable there), but this proves
    // the guard actually holds when something does.
    const manager = createScopeManager();
    manager.setScope(scope('org-a'));
    const gate = deferred<string>();
    const run = vi.fn(() => gate.promise);
    const dispatchResults: boolean[] = [];
    let latestPhase = 'unset';

    function Harness() {
      const action = useSingleFlightAction<string>();
      latestPhase = action.phase;
      useEffect(() => {
        dispatchResults.push(action.dispatch('download-markdown', run));
        // eslint-disable-next-line react-hooks/exhaustive-deps
      }, []);
      return null;
    }

    render(
      <StrictMode>
        <ScopeProvider manager={manager}>
          <Harness />
        </ScopeProvider>
      </StrictMode>,
    );

    // The mount effect fired twice (StrictMode); only the first dispatch
    // could start a ticket, the second was rejected by the reentrancy guard.
    await waitFor(() => expect(dispatchResults).toEqual([true, false]));
    expect(run).toHaveBeenCalledTimes(1);

    await act(async () => {
      gate.resolve('bytes-saved-once');
      await Promise.resolve();
      await Promise.resolve();
    });
    await waitFor(() => expect(latestPhase).toBe('success'));
    // Settling doesn't retroactively invoke `run` again either.
    expect(run).toHaveBeenCalledTimes(1);
  });
});
