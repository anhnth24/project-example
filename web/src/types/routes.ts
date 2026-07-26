/**
 * Route identifiers for the Markhand web SPA, from plan P2.3.
 *
 * `home` is not one of the P2.3 routes; it is the fallback shown at `/` until
 * a Wave 1 auth agent decides where an authenticated/anonymous visitor should
 * land by default.
 */
export type RouteName =
  'home' | 'login' | 'library' | 'qa' | 'adminMembers' | 'adminUsage' | 'help' | 'notFound';

export interface RouteParams {
  collectionId?: string;
}

export interface RouteMatch {
  name: RouteName;
  params: RouteParams;
}
