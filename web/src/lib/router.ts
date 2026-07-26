import type { RouteMatch, RouteName } from '../types/routes';

interface RouteDefinition {
  name: Exclude<RouteName, 'home' | 'notFound'>;
  pattern: RegExp;
  param?: 'collectionId';
}

// Order matters: more specific patterns (admin/*) must be checked before any
// pattern that could shadow them.
const routeDefinitions: RouteDefinition[] = [
  { name: 'login', pattern: /^\/login\/?$/ },
  { name: 'adminMembers', pattern: /^\/admin\/members\/?$/ },
  { name: 'adminUsage', pattern: /^\/admin\/usage\/?$/ },
  { name: 'library', pattern: /^\/library(?:\/([^/]+))?\/?$/, param: 'collectionId' },
  { name: 'qa', pattern: /^\/qa(?:\/([^/]+))?\/?$/, param: 'collectionId' },
  { name: 'help', pattern: /^\/help\/?$/ },
];

/**
 * Pure path -> route matcher for the P2.3 routes. `/` matches `home`
 * (the pre-auth landing content already in `App.tsx`); anything else that
 * does not match a known route is `notFound`.
 *
 * This is intentionally not a router: no nested routes, no layouts, no data
 * loading. It only answers "given this pathname, which page and which
 * params" so `App.tsx` can pick a page to render.
 */
export function matchRoute(pathname: string): RouteMatch {
  if (pathname === '' || pathname === '/') {
    return { name: 'home', params: {} };
  }
  for (const route of routeDefinitions) {
    const result = route.pattern.exec(pathname);
    if (result) {
      const params: RouteMatch['params'] = {};
      if (route.param && result[1] !== undefined) {
        params[route.param] = decodeURIComponent(result[1]);
      }
      return { name: route.name, params };
    }
  }
  return { name: 'notFound', params: {} };
}

/** Builds a path for `/library/:collectionId?` or `/qa/:collectionId?`. */
export function buildScopedPath(
  base: 'library' | 'qa',
  collectionId?: string,
): string {
  return collectionId ? `/${base}/${encodeURIComponent(collectionId)}` : `/${base}`;
}
