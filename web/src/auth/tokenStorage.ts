// Where the rotating refresh token lives across a page reload.
//
// The access token stays memory-only (per plans/markhand-web/phase-2-web-spa.md
// §P2.3) and is always lost on reload — that part is not a decision, it is
// required. The refresh token is the actual decision: this deployment has no
// server-set cookie to fall back on (ground truth for P2-05: the refresh
// token travels in the JSON body, not a cookie — the ADR 0010 / P2.3 "if the
// deployment uses cookie refresh" branch does not apply here), so without
// persisting *something* browser-side, "am I still signed in?" can never
// survive an ordinary reload: there would be nothing to hand back to
// `POST /auth/refresh`.
//
// Decision: persist only the refresh token, in `sessionStorage`, under one
// fixed key.
//
//   - `sessionStorage` over `localStorage`: it survives a same-tab reload/
//     back-forward (the case this exists for) but is gone the moment the tab
//     or browser closes — no indefinitely-lived ambient credential sitting in
//     the browser profile after the user walks away.
//   - `sessionStorage`/`localStorage` over "nothing persisted": the
//     alternative is every reload silently logging the user out, which is a
//     real, simpler, *more secure* option — it was rejected here only because
//     the plan's P2-05 acceptance criteria (intended-route + expiry
//     restoration) call for surviving a reload, not because it is wrong.
//
// Cost, stated plainly: any script that runs in this origin (i.e. a
// successful XSS) can read this value and mint itself new access tokens by
// calling refresh, for as long as the refresh token/family stays valid — a
// HttpOnly cookie is the only thing that removes this exposure, and that is
// not available in this deployment. `sessionStorage`'s tab-lifetime bound and
// the CSP/sanitization work in P2.7/P2.13 are the mitigations; neither
// eliminates the risk. [Unverified] whether XSS is otherwise reachable in
// this app is outside this task's scope to assess.
const STORAGE_KEY = 'markhand.refreshToken';

function storage(): Storage | undefined {
  try {
    return typeof window === 'undefined' ? undefined : window.sessionStorage;
  } catch {
    // Some browser configurations (locked-down privacy settings, certain
    // private-mode combinations) throw on *access* to sessionStorage, not
    // just on read/write. Treat that the same as "no persistence available".
    return undefined;
  }
}

/** The persisted refresh token, or `null` if none is stored (or storage is unavailable). */
export function loadPersistedRefreshToken(): string | null {
  try {
    return storage()?.getItem(STORAGE_KEY) ?? null;
  } catch {
    return null;
  }
}

/** Persists `refreshToken` so a same-tab reload can restore the session. Best-effort. */
export function savePersistedRefreshToken(refreshToken: string): void {
  try {
    storage()?.setItem(STORAGE_KEY, refreshToken);
  } catch {
    // Storage full/blocked: the session just will not survive a reload.
    // In-memory login/logout/refresh behavior for the current tab is unaffected.
  }
}

/** Removes the persisted refresh token (logout, session loss, failed bootstrap). */
export function clearPersistedRefreshToken(): void {
  try {
    storage()?.removeItem(STORAGE_KEY);
  } catch {
    // ignore
  }
}
