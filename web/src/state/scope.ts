// The "current scope" seam for P2-06 (plans/markhand-web/phase-2-web-spa.md
// §P2.3 + Gate: "Không render dữ liệu từ scope cũ").
//
// Scope switching (org switch, login, logout) is a race, not a feature: a
// slow request/SSE stream started under org A can resolve/deliver *after*
// the UI has moved on to org B. The fix modeled here is a monotonic
// "epoch" — a generation counter for "which scope is currently trusted" —
// plus a central registry of things to abort the instant the epoch moves
// on. Nothing in this file knows about HTTP or SSE specifics; those seams
// live in `hooks/useScopeSafeRequest.ts` and `hooks/useScopeSafeSse.ts`,
// which both (a) register their `AbortController`/connection here so a
// switch aborts them centrally, and (b) re-check `isCurrent(epoch)` right
// before acting on a resolved value — because an abort is not guaranteed to
// stop a response that is already in flight (see those files for why both
// checks are needed).
//
// Seam handed to whoever drives org switching (the auth agent, per the
// P2-06 brief — this file does not do login/logout/org-picker UI): call
// `setScope(...)` exactly once per transition — after login, after an org
// switch completes, and with `null` on logout. Everything else reacts to
// that single call.

export interface Scope {
  readonly orgId: string;
  readonly permissions: readonly string[];
  readonly allowedCollectionIds: readonly string[];
}

export interface ScopeSnapshot {
  /**
   * Identifies the current scope generation. Bumped by every `setScope`
   * call that changes org — including anonymous <-> authenticated
   * transitions (login, logout) — and by re-logging into the same org
   * after a logout (there is no null epoch in between two different
   * *sessions*, only between two different *scopes*). Never reused, so an
   * epoch value captured once always means exactly one specific scope
   * generation forever.
   */
  readonly epoch: number;
  readonly scope: Scope | null;
}

/** Anything that can be told "stop, you're stale" — an `AbortController`, an `SseConnection`, or any other cancellable unit of in-flight work. */
export interface Abortable {
  abort(): void;
}

export interface ScopeManager {
  getSnapshot(): ScopeSnapshot;
  /** React `useSyncExternalStore`-compatible subscription. Returns an unsubscribe. */
  subscribe(listener: () => void): () => void;
  /**
   * Installs a new scope. Bumps the epoch — and aborts every `Abortable`
   * still registered under the previous epoch — whenever `scope`'s `orgId`
   * differs from the current one, or the null/non-null state changes
   * (login, logout, org switch). Setting a scope with the *same* `orgId`
   * (e.g. a `/auth/me` re-fetch that refreshes `permissions`/
   * `allowedCollectionIds` for the org already active) updates the
   * snapshot but does NOT bump the epoch — in-flight work for that org is
   * not stale and is left alone.
   */
  setScope(scope: Scope | null): void;
  /**
   * True if `epoch` is still the current one. This is the exact check a
   * caller makes right before rendering/applying a resolved response or
   * delivered SSE message — the discard point. A stale epoch here means
   * "this belongs to a scope the user has already switched away from";
   * the value must never be rendered.
   */
  isCurrent(epoch: number): boolean;
  /**
   * Registers `abortable` to be aborted the instant the epoch moves past
   * `epoch`. If `epoch` is already stale by the time this is called,
   * `abortable` is aborted synchronously and nothing is registered.
   * Returns an unregister function — callers must call it once the
   * abortable finishes on its own (success, error, or its own abort) so
   * the registry does not grow unbounded over a long session.
   */
  registerAbortable(epoch: number, abortable: Abortable): () => void;
}

function sameScope(a: Scope | null, b: Scope | null): boolean {
  if (a === b) return true;
  if (a === null || b === null) return false;
  return a.orgId === b.orgId;
}

export function createScopeManager(): ScopeManager {
  let epoch = 0;
  let scope: Scope | null = null;
  // Cached snapshot object: `getSnapshot()` must return a referentially
  // stable value between actual state changes (React's
  // `useSyncExternalStore` — which `ScopeProvider.tsx` uses — calls
  // `getSnapshot()` on every render to detect whether a re-render is
  // needed; a fresh object literal on every call would look like a change
  // every time and loop forever). Only replaced from inside `setScope`,
  // exactly when `epoch`/`scope` actually change.
  let snapshot: ScopeSnapshot = { epoch, scope };
  const listeners = new Set<() => void>();
  // One registry per still-open epoch. An epoch's entry is deleted as soon
  // as it is superseded (its abortables are aborted and the set dropped),
  // so this only ever holds the current epoch's abortables plus whatever
  // hasn't called its unregister function yet from the immediately
  // preceding switch.
  const abortables = new Map<number, Set<Abortable>>();

  function notify(): void {
    for (const listener of [...listeners]) listener();
  }

  return {
    getSnapshot() {
      return snapshot;
    },

    subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },

    setScope(next) {
      if (sameScope(scope, next)) {
        scope = next;
        snapshot = { epoch, scope };
        notify();
        return;
      }
      const previousEpoch = epoch;
      scope = next;
      epoch += 1;
      snapshot = { epoch, scope };
      const toAbort = abortables.get(previousEpoch);
      abortables.delete(previousEpoch);
      if (toAbort) {
        for (const abortable of toAbort) {
          abortable.abort();
        }
      }
      notify();
    },

    isCurrent(candidate) {
      return candidate === epoch;
    },

    registerAbortable(forEpoch, abortable) {
      if (forEpoch !== epoch) {
        abortable.abort();
        return () => {};
      }
      let set = abortables.get(forEpoch);
      if (!set) {
        set = new Set();
        abortables.set(forEpoch, set);
      }
      set.add(abortable);
      return () => {
        set.delete(abortable);
      };
    },
  };
}
