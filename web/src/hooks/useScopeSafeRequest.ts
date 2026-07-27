// P2-06 (plans/markhand-web/phase-2-web-spa.md §P2.3 + Gate: "Không render
// dữ liệu từ scope cũ"). Runs an async request tied to the *scope epoch*
// active when it starts (see `state/scope.ts` for what an epoch is and why).
//
// Layered protections. Verified by mutation-testing each one in isolation
// (strip it, watch `useScopeSafeRequest.test.tsx` go red, restore it — see
// the P2-06 report for the transcript):
//
//   1. The `AbortController` passed to `fn` is registered with the
//      `ScopeManager` and aborted the instant the epoch moves on (org
//      switch/logout), so a well-behaved request stops real network work
//      as early as possible. Does not by itself stop a response already in
//      flight from resolving and calling `.then()`.
//   2. `discarded` (set by this effect's own cleanup) and
//      `manager.isCurrent(requestEpoch)` both gate the `setSettled()` calls
//      in `.then()`/`.catch()`. Either one alone already prevents a stale
//      resolution from ever reaching `settled` state — confirmed by
//      mutation-testing them independently — so they are deliberately
//      redundant with each other and with (3) below, not decorative.
//   3. THE decisive guard, proven necessary by mutation testing (removing
//      it, and only it, is the one change that turns
//      `useScopeSafeRequest.test.tsx` red): the render-time comparison
//      `settled.generation !== generation` below. Because it is a pure
//      value comparison evaluated at render — not a callback racing against
//      React's update scheduling — it holds regardless of exactly when (2)
//      lets a write through: a write tagged with a superseded generation
//      can never match the current one again, so it can be written to
//      state and still never be the value the hook returns.
//
// Re-runs whenever an entry in `deps` changes, or whenever the scope epoch
// changes — a switch always invalidates and restarts the request under the
// new scope rather than leaving the old one to finish unread.
//
// Implementation note: this hook never calls `setState` synchronously
// inside the effect body (only from the async `.then()`/`.catch()`
// callbacks once a request actually settles). The "a new request just
// started, so render as loading" transition instead uses React's
// documented "adjust state while rendering" idiom (see
// https://react.dev/reference/react/useState#storing-information-from-previous-renders):
// `useRequestGeneration` compares the incoming epoch/deps against what was
// rendered last and, if they differ, calls `setState` *during render*,
// which React explicitly supports and re-renders once more before paint —
// no extra effect, no synchronous-setState-in-effect lint violation.
import { useEffect, useRef, useState, type DependencyList } from 'react';
import { useScope } from '../state/ScopeProvider';

export type ScopeSafeRequestStatus = 'loading' | 'success' | 'error';

export interface ScopeSafeRequestState<T> {
  readonly status: ScopeSafeRequestStatus;
  readonly data: T | undefined;
  readonly error: unknown;
}

interface Settled<T> {
  readonly generation: number;
  readonly status: 'success' | 'error';
  readonly data: T | undefined;
  readonly error: unknown;
}

function shallowEqualDeps(a: DependencyList, b: DependencyList): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i += 1) {
    if (!Object.is(a[i], b[i])) return false;
  }
  return true;
}

interface Tracked {
  readonly epoch: number;
  readonly deps: DependencyList;
  readonly generation: number;
}

/**
 * A monotonically increasing "which request should be active" counter,
 * bumped whenever `epoch` or any entry of `deps` changes relative to the
 * previous render. Encodes both triggers (scope switch, caller-supplied
 * deps change) into the single number the effect below actually depends on.
 */
function useRequestGeneration(epoch: number, deps: DependencyList): number {
  const [tracked, setTracked] = useState<Tracked>(() => ({ epoch, deps, generation: 0 }));
  if (tracked.epoch !== epoch || !shallowEqualDeps(tracked.deps, deps)) {
    const next: Tracked = { epoch, deps, generation: tracked.generation + 1 };
    setTracked(next);
    return next.generation;
  }
  return tracked.generation;
}

export function useScopeSafeRequest<T>(
  fn: (signal: AbortSignal) => Promise<T>,
  deps: DependencyList,
): ScopeSafeRequestState<T> {
  const { manager, epoch } = useScope();

  // "Latest callback" ref, updated only from an effect (never mutated
  // during render) — same convention as `useFocusTrap`'s `onCloseRef`.
  const fnRef = useRef(fn);
  useEffect(() => {
    fnRef.current = fn;
  }, [fn]);

  const generation = useRequestGeneration(epoch, deps);
  const [settled, setSettled] = useState<Settled<T> | null>(null);

  useEffect(() => {
    const requestEpoch = epoch;
    const requestGeneration = generation;
    const controller = new AbortController();
    const unregister = manager.registerAbortable(requestEpoch, controller);
    let discarded = false;

    fnRef
      .current(controller.signal)
      .then((data) => {
        if (discarded || !manager.isCurrent(requestEpoch)) return; // superseded: skip the write (belt; see module doc's guard (3) for the suspenders)
        setSettled({ generation: requestGeneration, status: 'success', data, error: undefined });
      })
      .catch((error: unknown) => {
        if (discarded || !manager.isCurrent(requestEpoch)) return; // superseded: skip the write (belt; see module doc's guard (3) for the suspenders)
        setSettled({ generation: requestGeneration, status: 'error', data: undefined, error });
      })
      .finally(() => {
        unregister();
      });

    return () => {
      discarded = true;
      controller.abort();
      unregister();
    };
  }, [manager, epoch, generation]);

  if (settled === null || settled.generation !== generation) {
    // Either the very first request for this hook instance, or a newer
    // generation has started (scope switch or `deps` change) and hasn't
    // settled yet — render as loading either way, never the previous
    // generation's leftover data.
    return { status: 'loading', data: undefined, error: undefined };
  }
  return { status: settled.status, data: settled.data, error: settled.error };
}
