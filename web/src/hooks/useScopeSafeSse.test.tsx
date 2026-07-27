import { act, renderHook } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { createScopeManager, type Scope, type ScopeManager } from '../state/scope';
import { ScopeProvider } from '../state/ScopeProvider';
import { useScopeSafeSse, type ScopeSafeSseSource } from './useScopeSafeSse';

afterEach(() => {
  vi.restoreAllMocks();
});

function scope(orgId: string): Scope {
  return { orgId, permissions: [], allowedCollectionIds: [] };
}

function wrapperFor(manager: ScopeManager) {
  return function Wrapper({ children }: { children: React.ReactNode }) {
    return <ScopeProvider manager={manager}>{children}</ScopeProvider>;
  };
}

/**
 * A hand-rolled abortable async-iterable stream, standing in for
 * `SseConnection` (deliberately not mocked — see the P2-06 brief: "SSE is
 * deliberately not mocked [in mocks/**]; use your own fake for stream
 * tests"). `push` delivers a message to whichever `next()` call is
 * currently pending (or queues it).
 *
 * `abort()` is deliberately a plain spy that does NOT touch the iterator at
 * all — it neither cancels a pending `next()` nor stops a later `push()`
 * from being delivered. This is intentional, not a shortcut: it lets a test
 * simulate a message that the transport had already decoded/queued before
 * `abort()` was requested, arriving over the wire regardless of the abort
 * call, so a test can prove the *hook's* epoch check — not the transport's
 * cancellation — is what stops the message from reaching `onMessage`.
 */
function fakeSseSource<M>() {
  let pendingResolve: ((result: IteratorResult<M>) => void) | undefined;
  const queue: M[] = [];
  const abort = vi.fn();

  const source: ScopeSafeSseSource<M> = {
    abort,
    [Symbol.asyncIterator]() {
      return {
        next(): Promise<IteratorResult<M>> {
          if (queue.length > 0) {
            return Promise.resolve({ value: queue.shift() as M, done: false });
          }
          return new Promise<IteratorResult<M>>((resolve) => {
            pendingResolve = resolve;
          });
        },
      };
    },
  };

  return {
    source,
    abort,
    push(message: M) {
      if (pendingResolve) {
        const resolve = pendingResolve;
        pendingResolve = undefined;
        resolve({ value: message, done: false });
      } else {
        queue.push(message);
      }
    },
  };
}

async function flushMicrotasks(times = 3) {
  for (let i = 0; i < times; i += 1) {
    await Promise.resolve();
  }
}

describe('useScopeSafeSse', () => {
  it('delivers messages while the scope is current', async () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-a'));
    const fake = fakeSseSource<string>();
    const onMessage = vi.fn();

    renderHook(() => useScopeSafeSse(() => fake.source, onMessage, []), {
      wrapper: wrapperFor(manager),
    });

    await act(async () => {
      fake.push('hello-org-a');
      await flushMicrotasks();
    });

    expect(onMessage).toHaveBeenCalledWith('hello-org-a');
  });

  it('aborts the SSE stream from the old org the instant the scope switches', async () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-a'));
    const fake = fakeSseSource<string>();

    renderHook(() => useScopeSafeSse(() => fake.source, vi.fn(), []), {
      wrapper: wrapperFor(manager),
    });
    await flushMicrotasks();

    expect(fake.abort).not.toHaveBeenCalled();

    act(() => {
      manager.setScope(scope('org-b'));
    });

    // Called at least once — both the central `registerAbortable` registry
    // and the effect's own cleanup (its dependency array includes the
    // epoch) call `abort()` on a switch; `SseConnection.abort()` documents
    // itself as idempotent, so a double call is expected, not a bug.
    expect(fake.abort).toHaveBeenCalled();
  });

  it('delivers nothing after a switch, even for a message already in flight when abort() fired', async () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-a'));
    const fake = fakeSseSource<string>();
    const onMessage = vi.fn();
    let calls = 0;

    // Factory returns `undefined` (no new stream) on the second call, so the
    // org-B run doesn't open a *second* reader on the same fake object —
    // this test is isolated to whether org-A's single abandoned connection
    // can still leak a message, not to stream-reopening behavior (covered
    // by the "opens a fresh stream" test below).
    renderHook(
      () => useScopeSafeSse(() => (calls++ === 0 ? fake.source : undefined), onMessage, []),
      { wrapper: wrapperFor(manager) },
    );
    await flushMicrotasks();
    expect(calls).toBe(1);

    act(() => {
      manager.setScope(scope('org-b'));
    });
    await flushMicrotasks();
    expect(calls).toBe(2); // the effect re-ran for the new epoch and asked the factory again
    expect(fake.abort).toHaveBeenCalled();

    // A message from org A's now-abandoned connection lands right after the
    // switch — this fake's `abort()` deliberately does not cancel the
    // pending `next()`, simulating a message the transport had already
    // decoded before the abort request took effect. The hook's own epoch
    // check (not the abort call, and not "no new stream was opened") is
    // what must stop it from reaching `onMessage`.
    await act(async () => {
      fake.push('org-a-message-arriving-late');
      await flushMicrotasks();
    });

    expect(onMessage).not.toHaveBeenCalled();
  });

  it('opens a fresh stream (and aborts the old one) on rapid A -> B -> A switching, delivering only current-scope messages', async () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-a'));
    const fakeA1 = fakeSseSource<string>();
    const fakeB = fakeSseSource<string>();
    const fakeA2 = fakeSseSource<string>();
    const bySlot = [fakeA1, fakeB, fakeA2];
    const onMessage = vi.fn();

    const { rerender } = renderHook(
      ({ n }: { n: number }) => useScopeSafeSse(() => bySlot[n].source, onMessage, [n]),
      { wrapper: wrapperFor(manager), initialProps: { n: 0 } },
    );
    await flushMicrotasks();

    act(() => {
      manager.setScope(scope('org-b'));
      rerender({ n: 1 });
    });
    await flushMicrotasks();

    act(() => {
      manager.setScope(scope('org-a'));
      rerender({ n: 2 });
    });
    await flushMicrotasks();

    expect(fakeA1.abort).toHaveBeenCalled();
    expect(fakeB.abort).toHaveBeenCalled();

    // Stale sources still try to deliver — must be discarded.
    await act(async () => {
      fakeA1.push('stale-a1');
      fakeB.push('stale-b');
      await flushMicrotasks();
    });
    expect(onMessage).not.toHaveBeenCalled();

    // Only the current (third) connection's messages reach the callback.
    await act(async () => {
      fakeA2.push('current-a2');
      await flushMicrotasks();
    });
    expect(onMessage).toHaveBeenCalledTimes(1);
    expect(onMessage).toHaveBeenCalledWith('current-a2');
  });

  it('aborts the current stream on unmount', async () => {
    const manager = createScopeManager();
    manager.setScope(scope('org-a'));
    const fake = fakeSseSource<string>();

    const { unmount } = renderHook(() => useScopeSafeSse(() => fake.source, vi.fn(), []), {
      wrapper: wrapperFor(manager),
    });
    await flushMicrotasks();

    unmount();

    expect(fake.abort).toHaveBeenCalledTimes(1);
  });
});
