import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { createScopeManager, type Scope, type ScopeManager } from '../state/scope';
import { ScopeProvider } from '../state/ScopeProvider';
import { useScopeSafeRequest } from './useScopeSafeRequest';

afterEach(() => {
  vi.restoreAllMocks();
});

function scope(orgId: string): Scope {
  return { orgId, permissions: [], allowedCollectionIds: [] };
}

/** A promise plus externally-callable resolve, so a test can control exactly when a "network" call settles relative to an org switch. */
function createDeferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => {
    resolve = res;
  });
  return { promise, resolve };
}

function wrapperFor(manager: ScopeManager) {
  return function Wrapper({ children }: { children: React.ReactNode }) {
    return <ScopeProvider manager={manager}>{children}</ScopeProvider>;
  };
}

async function flushMicrotasks(times = 3) {
  for (let i = 0; i < times; i += 1) {
    await Promise.resolve();
  }
}

describe('useScopeSafeRequest', () => {
  it('renders the resolved value when no switch happens', async () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-a'));

    const { result } = renderHook(() => useScopeSafeRequest(async () => 'org-a-data', []), {
      wrapper: wrapperFor(manager),
    });

    await waitFor(() => expect(result.current.status).toBe('success'));
    expect(result.current.data).toBe('org-a-data');
  });

  it('discards a late HTTP response from the old org after a switch (fn ignores the abort signal entirely)', async () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-a'));
    const deferredA = createDeferred<string>();
    const deferredB = createDeferred<string>();
    let call = 0;

    // `fn` deliberately never reads `signal` — proving the discard does not
    // depend on the request actually honoring AbortController. It returns a
    // distinct deferred per invocation, so we can resolve the abandoned
    // org-A request independently of the current org-B one.
    const { result } = renderHook(
      () =>
        useScopeSafeRequest(async () => {
          const deferred = call === 0 ? deferredA : deferredB;
          call += 1;
          return deferred.promise;
        }, []),
      { wrapper: wrapperFor(manager) },
    );

    await waitFor(() => expect(result.current.status).toBe('loading'));
    expect(call).toBe(1);

    // Switch org *before* the org-A request resolves. This starts a second,
    // current request (org-B's), abandoning the first.
    await act(async () => {
      manager.setScope(scope('org-b'));
      await flushMicrotasks();
    });
    expect(call).toBe(2);

    // The org-A response finally arrives late. It must never be rendered —
    // org-B's own request is still pending.
    await act(async () => {
      deferredA.resolve('org-a-data-arriving-late');
      await flushMicrotasks();
    });

    expect(result.current.data).toBeUndefined();
    expect(result.current.status).toBe('loading');

    // Sanity check: org-B's own (current) response does render once it settles.
    await act(async () => {
      deferredB.resolve('org-b-data');
      await flushMicrotasks();
    });
    expect(result.current.data).toBe('org-b-data');
    expect(result.current.status).toBe('success');
  });

  it('aborts the in-flight request signal the instant the org switches', async () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-a'));
    let capturedSignal: AbortSignal | undefined;
    const deferred = createDeferred<string>();

    renderHook(
      () =>
        useScopeSafeRequest(async (signal) => {
          capturedSignal = signal;
          return deferred.promise;
        }, []),
      { wrapper: wrapperFor(manager) },
    );

    await waitFor(() => expect(capturedSignal).toBeDefined());
    expect(capturedSignal?.aborted).toBe(false);

    manager.setScope(scope('org-b'));

    expect(capturedSignal?.aborted).toBe(true);
  });

  it('never renders a stale payload across rapid A -> B -> A switching', async () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-a'));

    const deferredByVisit = [
      createDeferred<string>(),
      createDeferred<string>(),
      createDeferred<string>(),
    ];

    const { result, rerender } = renderHook(
      ({ n }: { n: number }) => useScopeSafeRequest(async () => deferredByVisit[n].promise, [n]),
      { wrapper: wrapperFor(manager), initialProps: { n: 0 } },
    );
    await waitFor(() => expect(result.current.status).toBe('loading'));

    // Switch to org-b (n=1) before org-a-1 (visit 0) resolves.
    await act(async () => {
      manager.setScope(scope('org-b'));
      rerender({ n: 1 });
      await flushMicrotasks();
    });

    // Switch back to org-a (n=2, a *new* epoch, not the first org-a epoch) before org-b (visit 1) resolves.
    await act(async () => {
      manager.setScope(scope('org-a'));
      rerender({ n: 2 });
      await flushMicrotasks();
    });

    // Resolve everything out of order: the newest request last, stragglers first.
    await act(async () => {
      deferredByVisit[1].resolve('org-b-data');
      deferredByVisit[0].resolve('org-a-1-data-stale');
      await flushMicrotasks();
    });

    // Neither stale resolution may ever have been rendered — the current
    // (third) request is still pending.
    expect(result.current.data).not.toBe('org-b-data');
    expect(result.current.data).not.toBe('org-a-1-data-stale');
    expect(result.current.status).toBe('loading');

    await act(async () => {
      deferredByVisit[2].resolve('org-a-2-data-current');
      await flushMicrotasks();
    });
    expect(result.current.data).toBe('org-a-2-data-current');
    expect(result.current.status).toBe('success');
  });
});
