// P2-06 (plans/markhand-web/phase-2-web-spa.md §P2.3 + Gate: "Không render
// dữ liệu từ scope cũ") applied to SSE. Drives any abortable async-iterable
// stream (an `SseConnection` from `api/sse.ts`, or a fake in tests) tied to
// the scope epoch active when it was opened.
//
// Same two protections as `useScopeSafeRequest`, applied per-message instead
// of per-response:
//
//   1. The source is registered with the `ScopeManager` and `.abort()`ed the
//      instant the epoch moves on, so a well-behaved connection stops
//      real network work as early as possible.
//   2. Even if a message was already queued/decoded by the time abort()
//      fires — the async generator in `api/sse.ts` can yield one more item
//      out of its internal buffer after cancellation is requested — the
//      epoch is checked again immediately before calling `onMessage`. THIS
//      is the discard point: a message whose epoch is no longer current is
//      dropped and never delivered to the caller.
//
// Reopens the stream whenever an entry in `deps` changes or the scope epoch
// changes; the previous source is always aborted first.
import { useEffect, useRef, type DependencyList } from 'react';
import { useScope } from '../state/ScopeProvider';

/**
 * Structural shape this hook needs from a stream source — deliberately not
 * imported from `api/sse.ts` so this file has no compile-time coupling to
 * that module's internals, only to the two members it actually calls.
 * `SseConnection` satisfies this today.
 */
export interface ScopeSafeSseSource<M> {
  abort(): void;
  [Symbol.asyncIterator](): AsyncIterator<M>;
}

export function useScopeSafeSse<M>(
  /** Return `undefined` to skip opening a stream this run (e.g. no collection selected yet). */
  factory: () => ScopeSafeSseSource<M> | undefined,
  onMessage: (message: M) => void,
  deps: DependencyList,
): void {
  const { manager, epoch } = useScope();

  // "Latest callback" refs, updated only from effects — same convention as
  // `useFocusTrap`'s `onCloseRef`; never mutated during render.
  const factoryRef = useRef(factory);
  useEffect(() => {
    factoryRef.current = factory;
  }, [factory]);

  const onMessageRef = useRef(onMessage);
  useEffect(() => {
    onMessageRef.current = onMessage;
  }, [onMessage]);

  useEffect(() => {
    const source = factoryRef.current();
    if (!source) return;

    const requestEpoch = epoch;
    const unregister = manager.registerAbortable(requestEpoch, source);
    let discarded = false;

    void (async () => {
      try {
        for await (const message of source) {
          if (discarded || !manager.isCurrent(requestEpoch)) return; // stale scope: stop delivering, discard
          onMessageRef.current(message);
        }
      } catch {
        // Expected once aborted/cancelled. `api/sse.ts` surfaces its own
        // retry/close policy through the message stream itself (a `closed`
        // message), not through iterator rejection, so there is nothing
        // else to act on here.
      } finally {
        unregister();
      }
    })();

    return () => {
      discarded = true;
      source.abort();
      unregister();
    };
    // `deps` is caller-supplied and deliberately re-spread here: this effect
    // must reopen the stream on every entry in `deps` *and* whenever the
    // scope epoch moves on, so an org switch always aborts the previous
    // stream rather than continuing to read it under the new scope.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [manager, epoch, ...deps]);
}
