// Stateful wrapper around the pure matcher in `../lib/router`. This is a
// hand-rolled "smallest thing that answers the question", not a router: it
// tracks `window.location.pathname`, reacts to back/forward navigation, and
// exposes a `navigate` function that does a history push. It does not do
// route guards, data loading, nested layouts, or scroll restoration — those
// belong to whichever agent builds P2.3/P2.5 auth+scope guards on top of
// this seam.
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react';
import { matchRoute } from '../lib/router';
import type { RouteMatch } from '../types/routes';

interface RouterContextValue {
  pathname: string;
  match: RouteMatch;
  navigate: (to: string) => void;
}

const RouterContext = createContext<RouterContextValue | null>(null);

function currentPathname(): string {
  return typeof window === 'undefined' ? '/' : window.location.pathname;
}

export function RouterProvider({ children }: { children: ReactNode }) {
  const [pathname, setPathname] = useState(currentPathname);

  useEffect(() => {
    const onPopState = () => setPathname(currentPathname());
    window.addEventListener('popstate', onPopState);
    return () => window.removeEventListener('popstate', onPopState);
  }, []);

  const navigate = useCallback((to: string) => {
    if (to !== currentPathname()) {
      window.history.pushState(null, '', to);
    }
    setPathname(currentPathname());
  }, []);

  const match = useMemo(() => matchRoute(pathname), [pathname]);

  const value = useMemo<RouterContextValue>(
    () => ({ pathname, match, navigate }),
    [pathname, match, navigate],
  );

  return <RouterContext.Provider value={value}>{children}</RouterContext.Provider>;
}

export function useRouter(): RouterContextValue {
  const context = useContext(RouterContext);
  if (!context) {
    throw new Error('useRouter must be used within a RouterProvider');
  }
  return context;
}
