// Real login/session implementation for P2-05 (plans/markhand-web/phase-2-web-spa.md
// §P2.3). This replaces the Wave-0 seam but keeps `useAuth()` as the single
// hook callers reach for; `session` is still the first thing on the returned
// value so the one existing caller (`LoginPage.tsx`) does not have to change
// how it reads status.
//
// Ground truth for this task: `MeResponse` has no single "role" field — only
// `permissions: string[]` (see plans/markhand-web/phase-1c-multi-org-security.md
// §P1C.2's permission constants/matrix). So `Session` surfaces permissions
// directly instead of inventing a role label the server does not send.
//
// This file depends on `../state/ScopeProvider`'s `useScope()` (owned by the
// concurrent org-switch agent, not edited here — only its already-committed
// public seam is used). `ScopeProvider.tsx`'s own module doc names this
// exact handoff: "the auth/shell agent calls `useScope().setScope(...)`
// after login resolves, after an org switch completes, and with `null` on
// logout/session-lost." This file owns the login/logout/session-lost calls;
// the org-switch call site belongs to whoever builds org switching.
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react';
import { apiClient, type ApiClient, type SessionTokens } from '../api/client';
import { isAbortError } from '../api/errors';
import type { components } from '../api/generated/contract';
import { useScope } from '../state/ScopeProvider';
import {
  clearPersistedRefreshToken,
  loadPersistedRefreshToken,
  savePersistedRefreshToken,
} from './tokenStorage';

type MeResponse = components['schemas']['MeResponse'];

/**
 * `checking` covers both "bootstrapping on page load" and "a login/logout is
 * in flight" — callers that only care about "may I render protected UI yet"
 * treat it like anonymous (do not render), while `RouteGuard.tsx` shows a
 * neutral loading state for it instead of bouncing to `/login` prematurely.
 */
export type Session =
  { status: 'checking' } | { status: 'anonymous' } | ({ status: 'authenticated' } & MeResponse);

export interface AuthContextValue {
  session: Session;
  /** `POST /auth/login` + populates the session from `GET /auth/me`. Rejects (and leaves the session anonymous) on any failure — callers show the error. */
  login(email: string, password: string): Promise<void>;
  /** Clears the session immediately (client-side), then best-effort calls `POST /auth/logout`. Never rejects. */
  logout(): Promise<void>;
  /** Re-fetches `GET /auth/me` and applies the result — e.g. after an org switch changes permissions/allowedCollectionIds without a full reload. */
  refreshSession(): Promise<void>;
  /**
   * `POST /orgs/switch` (1C-01) + atomic session swap for the org-switch UI
   * (`components/shell/OrgSwitch.tsx`). On success: the returned access+
   * refresh pair replaces the previous org's in one synchronous
   * `sessionManager.setTokens` call (no window where a concurrent request
   * could observe a half-updated session), then `GET /auth/me` is re-applied
   * exactly like `login`/`refreshSession` already do — which is what bumps
   * the scope epoch (`state/scope.ts`: a different `orgId` always bumps),
   * aborting every REST/SSE request still registered under the old org and
   * letting every `useScopeSafeRequest`/`useScopeSafeSse` caller restart
   * fresh under the new one. Rejects (session/scope left completely
   * untouched, still on the previous org) on a denied/network/rate-limited
   * switch, and also — mirroring `login`'s own failure path — if the
   * follow-up `/auth/me` fails even though the switch itself succeeded, so a
   * caller is never left holding new-org tokens with stale old-org session
   * state. A newer `switchOrg`/`login`/`logout`/`refreshSession` call
   * silently supersedes an older one in flight (same epoch-guard convention
   * every method here already uses) rather than letting the slower one win.
   */
  switchOrg(orgId: string): Promise<void>;
  /**
   * UI convenience only — never authorization. This only decides whether the
   * shell *renders* something; the server is always the one that decides
   * whether a request succeeds. A `true` here that turns out wrong just means
   * a control is shown that the next API call answers with a 403.
   */
  hasPermission(permission: string): boolean;
}

const defaultValue: AuthContextValue = {
  session: { status: 'anonymous' },
  login: () => Promise.reject(new Error('useAuth() called outside an AuthProvider')),
  logout: () => Promise.reject(new Error('useAuth() called outside an AuthProvider')),
  refreshSession: () => Promise.reject(new Error('useAuth() called outside an AuthProvider')),
  switchOrg: () => Promise.reject(new Error('useAuth() called outside an AuthProvider')),
  hasPermission: () => false,
};

const AuthContext = createContext<AuthContextValue>(defaultValue);

export interface AuthProviderProps {
  children: ReactNode;
  /** Injectable for tests; defaults to the app-wide singleton in `api/client.ts`. */
  client?: ApiClient;
}

export function AuthProvider({ children, client = apiClient }: AuthProviderProps) {
  // Lazy initializer (not an effect) so a visitor with no persisted refresh
  // token renders `anonymous` on the very first paint — no synchronous
  // setState-in-effect needed for that branch, and no flash of "checking"
  // for the common case of a plain, never-logged-in visit.
  const [session, setSession] = useState<Session>(() =>
    loadPersistedRefreshToken() ? { status: 'checking' } : { status: 'anonymous' },
  );
  const scope = useScope();

  // Guards against two known races:
  //  - a stale `me()` response arriving after `logout()` (or after a newer
  //    login/bootstrap superseded it) must not repopulate the shell;
  //  - `onSessionLost` firing while a `me()` call is still in flight must win.
  // Every attempt captures the epoch at its start and checks it again before
  // calling `setSession`; anything else bumps the epoch first.
  const epochRef = useRef(0);
  const abortRef = useRef<AbortController | null>(null);

  const applyMe = useCallback(
    (me: MeResponse) => {
      setSession({ status: 'authenticated', ...me });
      scope.setScope({
        orgId: me.orgId,
        permissions: me.permissions,
        allowedCollectionIds: me.allowedCollectionIds,
      });
    },
    [scope],
  );

  const becomeAnonymous = useCallback(() => {
    clearPersistedRefreshToken();
    setSession({ status: 'anonymous' });
    scope.setScope(null);
  }, [scope]);

  // Persists the pair handed to `onTokensRotated`, which fires synchronously
  // inside the rotation itself — the only point at which the stored copy is
  // guaranteed not to lag the server's view of the family.
  const persistRotatedTokens = useCallback((tokens: { refreshToken: string }) => {
    savePersistedRefreshToken(tokens.refreshToken);
  }, []);

  // Re-persists whatever refresh token the session manager is holding right
  // now. Still used at the login/bootstrap boundaries, where the caller wants
  // the write to have happened before it proceeds rather than as a side effect
  // of the rotation that just occurred.
  const syncPersistedRefreshToken = useCallback(() => {
    const current = client.sessionManager.getRefreshTokenForLogout();
    if (current) savePersistedRefreshToken(current);
  }, [client]);

  // --- Bootstrap: restore "am I signed in?" once per provider lifetime. ---
  // The "no stored token" case needs no work here at all — the lazy
  // `useState` initializer above already rendered `anonymous` on first paint.
  useEffect(() => {
    const stored = loadPersistedRefreshToken();
    if (!stored) return;
    const epoch = (epochRef.current += 1);
    const controller = new AbortController();
    abortRef.current = controller;
    (async () => {
      try {
        // Seed the session manager with the persisted refresh token behind
        // an already-expired dummy access token (the same
        // `expiresIn: -1` pattern `api/session.test.ts` uses to force a
        // refresh), so the very first `getAccessToken()` call inside
        // `client.me()` goes through the manager's own single-flight
        // refresh path — this file never re-implements refresh itself.
        client.sessionManager.setTokens({
          accessToken: '',
          refreshToken: stored,
          tokenType: 'Bearer',
          expiresIn: -1,
          orgId: '',
          userId: '',
        });
        const me = await client.me(controller.signal);
        if (epochRef.current !== epoch) return;
        applyMe(me);
        syncPersistedRefreshToken();
      } catch (cause) {
        if (epochRef.current !== epoch || isAbortError(cause)) return;
        becomeAnonymous();
      }
    })();
    return () => controller.abort();
    // Runs once per mount by design (bootstrap); `client`/`applyMe`/
    // `becomeAnonymous`/`syncPersistedRefreshToken` are stable for the
    // provider's lifetime (default client is a module singleton).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // --- Session loss mid-session (refresh rejected). ---
  useEffect(
    () =>
      client.tokenProvider.onSessionLost(() => {
        epochRef.current += 1;
        abortRef.current?.abort();
        becomeAnonymous();
      }),
    [client, becomeAnonymous],
  );

  // --- Keep the persisted refresh token from going stale. ---
  // Refresh tokens rotate on every use and replaying an already-rotated one
  // revokes the whole family (ADR 0010), so a persisted copy that misses a
  // rotation does not merely fail to restore the session — its next use looks
  // like a replay attack and signs the user out everywhere. An ordinary
  // in-session refresh rotates the pair entirely inside `session.ts`, so the
  // only correct place to write is the rotation itself: `onTokensRotated`
  // fires synchronously as part of applying the new pair, closing the window
  // rather than narrowing it.
  useEffect(() => client.sessionManager.onTokensRotated(persistRotatedTokens), [client]);

  const login = useCallback(
    async (email: string, password: string) => {
      abortRef.current?.abort();
      const epoch = (epochRef.current += 1);
      const controller = new AbortController();
      abortRef.current = controller;
      // `client.login()` (api/client.ts) is not side-effect-free like the
      // bare `client.request()` POST `switchOrg` above uses — on a
      // successful response it installs the returned tokens into the
      // session manager itself (`sessionManager.setTokens(tokens)`, asserted
      // by `api/client.test.ts`'s own "installs the returned tokens..."
      // case), before this function ever gets to look at the epoch. An
      // epoch guard placed only *after* `await client.login(...)` — the
      // `switchOrg` shape — would stop a stale login from being *rendered*,
      // but would not stop it from clobbering the session manager's tokens
      // out from under a newer login/logout/switchOrg that already won by
      // the time this one's HTTP response arrives. Passing `controller.signal`
      // here (previously only `me()` got a signal) is what actually closes
      // that: a login superseded by a newer call has this same controller
      // aborted at the top of that newer call, so `rawFetch` rethrows the
      // abort before `client.login()` ever reaches its `setTokens(...)`
      // line — the stale install never happens, rather than merely going
      // unrendered. (The one gap this can't close: if the response has
      // already been fully received by the time `abort()` runs, the browser
      // Fetch API's abort is a no-op on an already-settled request — the
      // same inherent limit every abort-based cancellation here has. The
      // epoch check right below still stops that outcome from being
      // *rendered*, which is the tightest guarantee available without
      // changing `client.login()`'s own documented contract.)
      let tokens: SessionTokens;
      try {
        tokens = await client.login({ email, password }, controller.signal);
      } catch (cause) {
        // A non-abort failure (wrong password, network, ...) touches
        // nothing here yet — same as before this change — so it just
        // propagates for the caller (e.g. `LoginPage.tsx`) to show.
        if (epochRef.current !== epoch || isAbortError(cause)) return;
        throw cause;
      }
      if (epochRef.current !== epoch) return;
      savePersistedRefreshToken(tokens.refreshToken);
      try {
        const me = await client.me(controller.signal);
        if (epochRef.current !== epoch) return;
        applyMe(me);
        syncPersistedRefreshToken();
      } catch (cause) {
        if (epochRef.current !== epoch || isAbortError(cause)) return;
        client.sessionManager.clear();
        becomeAnonymous();
        throw cause;
      }
    },
    [client, applyMe, becomeAnonymous, syncPersistedRefreshToken],
  );

  const logout = useCallback(async () => {
    epochRef.current += 1;
    abortRef.current?.abort();
    // Clear client-side first and unconditionally: logout must always take
    // effect locally even if the network call below fails (matches
    // `client.ts`'s own logout() contract).
    becomeAnonymous();
    try {
      await client.logout();
    } catch {
      // Best-effort; the local session is already cleared above.
    }
  }, [client, becomeAnonymous]);

  const refreshSession = useCallback(async () => {
    abortRef.current?.abort();
    const epoch = (epochRef.current += 1);
    const controller = new AbortController();
    abortRef.current = controller;
    try {
      const me = await client.me(controller.signal);
      if (epochRef.current !== epoch) return;
      applyMe(me);
      syncPersistedRefreshToken();
    } catch (cause) {
      if (epochRef.current !== epoch || isAbortError(cause)) return;
      becomeAnonymous();
    }
  }, [client, applyMe, becomeAnonymous, syncPersistedRefreshToken]);

  const switchOrg = useCallback(
    async (orgId: string) => {
      abortRef.current?.abort();
      const epoch = (epochRef.current += 1);
      // Mirrors `login`'s own shape exactly (mint tokens -> re-fetch `me()`
      // -> `applyMe`), because a switch IS a login into a different org
      // through a different endpoint — same failure handling applies.
      const tokens = await client.request('post', '/orgs/switch', { body: { orgId } });
      // A newer login/logout/switch superseded this one while the POST was in
      // flight: drop the minted tokens unused instead of installing them over
      // the newer call's session (same fate as an abandoned login response).
      if (epochRef.current !== epoch) return;
      // Atomic swap: this one synchronous call installs the new access AND
      // refresh token together, replacing the previous org's pair — no
      // partial state a concurrent `getAccessToken()`/`refreshNow()` could
      // observe mid-swap.
      client.sessionManager.setTokens(tokens);
      const controller = new AbortController();
      abortRef.current = controller;
      try {
        const me = await client.me(controller.signal);
        if (epochRef.current !== epoch) return;
        applyMe(me); // orgId differs from the previous scope -> bumps the scope epoch (state/scope.ts), which aborts every REST/SSE request still registered under the old org.
        syncPersistedRefreshToken();
      } catch (cause) {
        if (epochRef.current !== epoch || isAbortError(cause)) return;
        // The switch itself succeeded (we hold valid org-B tokens) but the
        // follow-up `me()` didn't — same call `login()` makes: don't leave
        // the app holding new tokens with no session to show for them.
        client.sessionManager.clear();
        becomeAnonymous();
        throw cause;
      }
    },
    [client, applyMe, becomeAnonymous, syncPersistedRefreshToken],
  );

  const hasPermission = useCallback(
    (permission: string) =>
      session.status === 'authenticated' && session.permissions.includes(permission),
    [session],
  );

  const value = useMemo<AuthContextValue>(
    () => ({ session, login, logout, refreshSession, switchOrg, hasPermission }),
    [session, login, logout, refreshSession, switchOrg, hasPermission],
  );

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useAuth(): AuthContextValue {
  return useContext(AuthContext);
}
