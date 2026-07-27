// Adapts `useScopeSafeRequest` (an effect that fires on mount/deps-change,
// built for scope-safe *reads*) to click-triggered *mutations*: download,
// reindex, and delete are never auto-fired, only dispatched from an event
// handler. A dispatched action becomes a "ticket" in state; `deps=[ticket]`
// makes useScopeSafeRequest's generation/epoch machinery run it exactly like
// any other scope-safe request — same abort-on-org-switch, same
// stale-result-discard documented in that hook's module doc.
//
// On top of that, this hook guarantees `DocumentRowActions.tsx` needs for
// the single-use download capability: a dispatched ticket's `run` is
// invoked at most once. Two layers, and mutation-testing
// (`useSingleFlightAction.test.tsx`) says something different about each —
// recorded here rather than assumed:
//   1. `pendingRef`, checked and set synchronously inside `dispatch` before
//      any state update: THE guard. Confirmed load-bearing by mutation
//      test — deleting the `if (pendingRef.current) return false;` line
//      turns two tests red (the reentrancy test, and the StrictMode test
//      below). This is what actually stops a same-tick double dispatch
//      (two rapid clicks racing the disabled-button re-render) and what
//      makes a mount-effect's dispatch survive React 18 StrictMode
//      double-invoking that effect once at mount (mount -> cleanup ->
//      mount, dev-only, to surface missing-cleanup bugs).
//   2. `inFlightRef`'s `Map<ticketId, Promise>` cache in the `run`
//      callback below, guarding against the *same ticket's* effect body
//      running twice. Mutation-tested the other way: removing it does
//      *not* turn any current test red — `DocumentRowActions` only ever
//      dispatches from a click, never from a mount effect, so this hook's
//      own effect never actually re-runs for an already-dispatched ticket
//      under today's call pattern. Kept anyway as cheap, harmless
//      insurance against a future caller dispatching from somewhere
//      StrictMode's mount double-invoke *would* reach — not proven
//      necessary today, unlike (1).
import { useEffect, useRef, useState } from 'react';
import { useScopeSafeRequest } from '../../hooks/useScopeSafeRequest';

export type ActionPhase = 'idle' | 'pending' | 'success' | 'error';

interface Ticket<T> {
  id: number;
  kind: string;
  run: (signal: AbortSignal) => Promise<T>;
}

interface RanResult<T> {
  ticketId: number;
  value: T;
}

export interface SingleFlightAction<T> {
  phase: ActionPhase;
  /** The `kind` string passed to `dispatch` for whichever ticket is current (pending/settled), or `null` before the first dispatch. */
  kind: string | null;
  /** Populated once `phase === 'success'`. */
  value: T | undefined;
  /** Populated once `phase === 'error'`. */
  error: unknown;
  /**
   * Starts `run(signal)`. Returns `false` (and does nothing) if another
   * action from this hook instance is still pending — a synchronous,
   * ref-backed reentrancy guard, so a same-tick double-fire (e.g. two
   * `click` handlers racing before React re-renders the disabled button)
   * cannot start a second ticket while one is in flight.
   */
  dispatch(kind: string, run: (signal: AbortSignal) => Promise<T>): boolean;
  /** Returns to `idle`, forgetting the last result — lets the same row action be dispatched again from a clean state (e.g. after showing a success notice). */
  reset(): void;
}

export function useSingleFlightAction<T>(): SingleFlightAction<T> {
  const [ticket, setTicket] = useState<Ticket<T> | null>(null);
  const pendingRef = useRef(false);
  const nextIdRef = useRef(0);
  const inFlightRef = useRef(new Map<number, Promise<T>>());

  const result = useScopeSafeRequest<RanResult<T> | null>(
    async (signal) => {
      if (!ticket) return null;
      let promise = inFlightRef.current.get(ticket.id);
      if (!promise) {
        promise = ticket.run(signal);
        inFlightRef.current.set(ticket.id, promise);
      }
      const value = await promise;
      return { ticketId: ticket.id, value };
    },
    [ticket],
  );

  // Release the reentrancy guard once this ticket has settled (either way),
  // from an effect rather than during render — mutating `pendingRef` while
  // rendering would be an unnecessary impurity for a value nothing here
  // reads back during the same render.
  useEffect(() => {
    if (ticket && result.status !== 'loading') {
      pendingRef.current = false;
    }
  }, [ticket, result.status]);

  function dispatch(kind: string, run: (signal: AbortSignal) => Promise<T>): boolean {
    if (pendingRef.current) return false;
    pendingRef.current = true;
    const id = nextIdRef.current;
    nextIdRef.current += 1;
    setTicket({ id, kind, run });
    return true;
  }

  function reset(): void {
    pendingRef.current = false;
    setTicket(null);
  }

  if (!ticket) {
    return { phase: 'idle', kind: null, value: undefined, error: undefined, dispatch, reset };
  }
  if (result.status === 'loading') {
    return {
      phase: 'pending',
      kind: ticket.kind,
      value: undefined,
      error: undefined,
      dispatch,
      reset,
    };
  }
  if (result.status === 'error') {
    return {
      phase: 'error',
      kind: ticket.kind,
      value: undefined,
      error: result.error,
      dispatch,
      reset,
    };
  }
  // status === 'success': `useScopeSafeRequest` only ever renders a
  // `settled` value whose generation matches the ticket that produced it
  // (see that hook's module doc, guard 3), so `result.data` here always
  // corresponds to `ticket`, never a superseded one.
  return {
    phase: 'success',
    kind: ticket.kind,
    value: result.data?.value,
    error: undefined,
    dispatch,
    reset,
  };
}
