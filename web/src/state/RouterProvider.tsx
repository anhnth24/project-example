// Stateful wrapper around the pure matcher in `../lib/router`. This is a
// hand-rolled "smallest thing that answers the question", not a router: it
// tracks `window.location.pathname` (+ `search`, added for P2-07's `?doc=`
// deep-link — see `searchParams` below), reacts to back/forward navigation,
// and exposes a `navigate` function that does a history push. It does not do
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
  /** Parsed `?query` string of the current URL — e.g. `LibraryPage`'s `?doc=<documentId>` (P2-07). Kept alongside `pathname`/`match` rather than folded into `RouteMatch.params`, since query params are orthogonal to which route matched and `matchRoute` itself only ever needs the path. */
  searchParams: URLSearchParams;
  match: RouteMatch;
  /** `to` may include a `?query` string (e.g. `/library/{id}?doc={documentId}`) — passed straight to `history.pushState`, same as a path-only `to` always was. */
  navigate: (to: string) => void;
}

const RouterContext = createContext<RouterContextValue | null>(null);

function currentPathname(): string {
  return typeof window === 'undefined' ? '/' : window.location.pathname;
}

/** Full path the browser is currently at, `pathname` + `search` — the identity `navigate` compares against so a query-only change (same pathname, different `?doc=`) still pushes a new history entry instead of being treated as a no-op. */
function currentPath(): string {
  if (typeof window === 'undefined') return '/';
  return window.location.pathname + window.location.search;
}

function currentSearch(): string {
  return typeof window === 'undefined' ? '' : window.location.search;
}

export function RouterProvider({ children }: { children: ReactNode }) {
  const [pathname, setPathname] = useState(currentPathname);
  const [search, setSearch] = useState(currentSearch);

  useEffect(() => {
    const onPopState = () => {
      setPathname(currentPathname());
      setSearch(currentSearch());
    };
    window.addEventListener('popstate', onPopState);
    return () => window.removeEventListener('popstate', onPopState);
  }, []);

  const navigate = useCallback((to: string) => {
    if (to !== currentPath()) {
      window.history.pushState(null, '', to);
    }
    setPathname(currentPathname());
    setSearch(currentSearch());
  }, []);

  const match = useMemo(() => matchRoute(pathname), [pathname]);
  const searchParams = useMemo(() => new URLSearchParams(search), [search]);

  const value = useMemo<RouterContextValue>(
    () => ({ pathname, searchParams, match, navigate }),
    [pathname, searchParams, match, navigate],
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
