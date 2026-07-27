// A cache keyed by "current scope" for P2-06 (Gate: "Không render dữ liệu
// từ scope cũ"). Two protections, both required — see the module doc in
// `scope.ts` for why one alone is not enough:
//
// 1. Reactive clear: subscribed to the `ScopeManager`, so the instant the
//    epoch moves on (any org switch, login, or logout) the entire backing
//    map is dropped. Org A's entries are gone before org B's UI can read
//    them, and switching back to org A later starts from an empty map
//    rather than resurrecting pre-switch data.
// 2. Write-time epoch check: `set()` takes the epoch the value was fetched
//    for and silently discards the write if that epoch is no longer
//    current. This is the case (1) alone cannot cover: a request started
//    under org A that only *resolves* after the switch to org B — its
//    `.then()` calling `cache.set(oldEpoch, ...)` — must not repopulate the
//    (already-cleared) cache with org A data while org B is active.
import type { ScopeManager } from './scope';

export interface ScopeCache<T> {
  get(key: string): T | undefined;
  has(key: string): boolean;
  /**
   * Write-through, but only if `epoch` is still current. Pass the epoch
   * that was current when the underlying fetch was *started* (or, more
   * precisely, whatever epoch the data is known to belong to) — not the
   * epoch read at call time — so a write raced by a switch is discarded
   * rather than silently laundered into "current".
   */
  set(epoch: number, key: string, value: T): void;
  delete(key: string): void;
  /** Drops every entry regardless of scope. Mainly for tests; the cache already self-clears on every switch. */
  clear(): void;
}

export function createScopeCache<T>(manager: ScopeManager): ScopeCache<T> {
  let entries = new Map<string, T>();
  let syncedEpoch = manager.getSnapshot().epoch;

  function syncEpoch(): void {
    const current = manager.getSnapshot().epoch;
    if (current !== syncedEpoch) {
      entries = new Map();
      syncedEpoch = current;
    }
  }

  // Belt-and-suspenders: clears eagerly on notify (not just lazily on next
  // get/set), so nothing served between a switch and the next cache call
  // observes a stale generation's map.
  manager.subscribe(syncEpoch);

  return {
    get(key) {
      syncEpoch();
      return entries.get(key);
    },

    has(key) {
      syncEpoch();
      return entries.has(key);
    },

    set(epoch, key, value) {
      syncEpoch();
      if (!manager.isCurrent(epoch)) return; // discard: write belongs to a superseded scope
      entries.set(key, value);
    },

    delete(key) {
      syncEpoch();
      entries.delete(key);
    },

    clear() {
      entries = new Map();
    },
  };
}
