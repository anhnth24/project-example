// React wiring for the scope seam in `scope.ts` (see that file's module doc
// for the race this exists to close, per P2-06 / plans/markhand-web/phase-2-web-spa.md
// §P2.3). This component owns no org/session logic itself — it just exposes
// a `ScopeManager` singleton through context via `useSyncExternalStore`, the
// same pattern `RouterProvider.tsx` uses for router state.
//
// Handoff seam: the auth/shell agent calls `useScope().setScope(...)` (or
// holds a reference to the same `ScopeManager`) after login resolves, after
// an org switch completes, and with `null` on logout/session-lost. This
// file does not call `setScope` itself.
import {
  createContext,
  useContext,
  useMemo,
  useState,
  useSyncExternalStore,
  type ReactNode,
} from 'react';
import { createScopeManager, type Scope, type ScopeManager, type ScopeSnapshot } from './scope';

export interface ScopeContextValue extends ScopeSnapshot {
  /** Escape hatch for callers that need the full seam (registerAbortable, isCurrent) beyond scope/epoch. */
  readonly manager: ScopeManager;
  setScope: ScopeManager['setScope'];
  isCurrent: ScopeManager['isCurrent'];
  registerAbortable: ScopeManager['registerAbortable'];
}

const ScopeContext = createContext<ScopeContextValue | null>(null);

export interface ScopeProviderProps {
  children: ReactNode;
  /** Escape hatch for tests that need to drive/observe the manager directly. Defaults to a fresh manager per mount. */
  manager?: ScopeManager;
}

export function ScopeProvider({ children, manager: injected }: ScopeProviderProps) {
  // `useState`'s lazy initializer (not a ref) for the stable per-mount
  // singleton: it is computed exactly once and its result is a value React
  // considers safe to read during render, unlike a ref's `.current`.
  const [manager] = useState<ScopeManager>(() => injected ?? createScopeManager());

  const snapshot = useSyncExternalStore(
    manager.subscribe,
    manager.getSnapshot,
    manager.getSnapshot,
  );

  const value = useMemo<ScopeContextValue>(
    () => ({
      ...snapshot,
      manager,
      setScope: manager.setScope,
      isCurrent: manager.isCurrent,
      registerAbortable: manager.registerAbortable,
    }),
    [snapshot, manager],
  );

  return <ScopeContext.Provider value={value}>{children}</ScopeContext.Provider>;
}

export function useScope(): ScopeContextValue {
  const context = useContext(ScopeContext);
  if (!context) {
    throw new Error('useScope must be used within a ScopeProvider');
  }
  return context;
}

export type { Scope, ScopeManager, ScopeSnapshot };
