// Browser-side mock bootstrap for the Playwright E2E suite (P2-15).
//
// The vitest suite installs the mock fetch per test (`installMockFetch()` in a
// `beforeEach`); Playwright drives a *real* browser against a served build, so
// the mock has to be installed once, at app bootstrap, before React renders
// and fires its first request. `main.tsx` calls this only when the app was
// built with `VITE_MARKHAND_MOCK` set, so none of `mocks/**` reaches a
// production bundle (main.tsx `import()`s it dynamically behind the flag).
//
// Each Playwright test starts from a fresh full page load, which re-imports
// this module and re-seeds the store, so tests are deterministic without a
// per-test reset hook. `window.__markhandMockReset` is also exposed so a test
// can re-seed mid-scenario (e.g. after a destructive flow) without a reload.
import { grantMemberManage } from '../components/admin/testSupport';
import { succeedJob } from '../components/upload/testSupport';
import { installMockFetch, mockControl, resetMockState } from './index';

declare global {
  interface Window {
    /** Re-seed the in-memory mock store to its initial fixtures. E2E-only. */
    __markhandMockReset?: () => void;
    /**
     * The same `mockControl` the vitest suite uses, exposed so a Playwright
     * test can force a status (`forceStatus('deleteDocument', 403)`) from the
     * page context to drive permission-deny / quota-exceed flows. E2E-only.
     */
    __markhandMockControl?: typeof mockControl;
    /**
     * Advances a job already registered in the mock store past `pending` —
     * see `components/upload/testSupport.ts`'s own doc for why this needs
     * its own seam rather than reusing `__markhandMockControl`. E2E-only.
     */
    __markhandMockJobs?: { succeed: typeof succeedJob };
  }
}

/**
 * Installs the fetch mock and seeds the fixture store. Grants the seeded demo
 * user `member.manage` so the P2-11/P2-12 admin flows are reachable in E2E —
 * the vitest suite deliberately does NOT grant it (it would change an
 * `App.test.tsx` assertion), but in mock-mode the demo user is the only user
 * and the admin pages are part of what E2E must cover.
 */
export function installBrowserMocks(): void {
  installMockFetch();
  resetMockState();
  grantMemberManage();
  window.__markhandMockReset = () => {
    resetMockState();
    grantMemberManage();
  };
  window.__markhandMockControl = mockControl;
  window.__markhandMockJobs = { succeed: succeedJob };
}
