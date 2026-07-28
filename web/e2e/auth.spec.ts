// Auth flows (P2-15 flow 1). The happy-path login is covered by
// smoke.spec.ts; this extends it with logout, an anonymous deep-link redirect
// that preserves `?next=`, and silent token-refresh recovery on a one-time
// 401.
import { expect, test } from '@playwright/test';
import { DEMO, forceStatus, login } from './support';

test('logout returns to /login', async ({ page }) => {
  await login(page);

  // The rail avatar is the account-menu trigger (UserMenu.tsx).
  await page.getByRole('button', { name: /^Tài khoản:/ }).click();
  await page.getByRole('button', { name: 'Đăng xuất' }).click();

  await expect(page).toHaveURL(/\/login/);
  await expect(page.getByRole('heading', { name: 'Đăng nhập vào Folyvo' })).toBeVisible();
  // The rail is chrome-free on /login — its destinations are gone.
  await expect(page.getByRole('link', { name: 'Thư viện' })).toHaveCount(0);
});

test('deep-linking a protected route while anonymous redirects to /login with ?next= preserved', async ({
  page,
}) => {
  // No login: a fresh context is anonymous. ProtectedRoute bounces to /login,
  // preserving the intended location (path AND query) as `?next=`.
  await page.goto('/library/some-collection?tab=recent');

  await expect(page).toHaveURL(/\/login\?next=/);
  await expect(page.getByRole('heading', { name: 'Đăng nhập vào Folyvo' })).toBeVisible();

  const next = new URL(page.url()).searchParams.get('next');
  expect(next).toBe('/library/some-collection?tab=recent');
});

test('a one-time 401 on an authenticated read is silently recovered via refresh (no bounce to /login)', async ({
  page,
}) => {
  // Verified against api/client.ts + api/session.ts: `authedFetch` retries a
  // request exactly once after a single `refreshNow()` on a 401, and the
  // session manager's single-flight refresh hits `POST /auth/refresh` (mocked
  // in handlers/auth.ts). Only a *failed* refresh triggers session-loss ->
  // anonymous -> /login; a recovered one is invisible.
  //
  // The read we force to 401 is `GET /auth/me` (`authMe`), performed as part
  // of the login round-trip (AuthContext.login -> client.me). It is used here
  // for a mock-harness reason, not by accident: forced responses go through
  // the mock's drift guard (fetchMock `emit` -> `assertStatusDeclared`), which
  // rejects any status an operation's contract doesn't declare. `authMe` is
  // one of the few operations whose contract actually declares 401, so it is
  // the operation that can be *forced* to 401 without the guard turning it
  // into a synthetic network error. At the point `me()` runs, login has
  // already installed valid tokens, so the client's single-flight refresh
  // recovers: attempt #1 (401) -> refresh -> attempt #2 (200) -> authenticated.
  await page.goto('/login');
  await forceStatus(page, 'authMe', 401, 1);

  await page.getByLabel('Email').fill(DEMO.email);
  await page.getByLabel('Mật khẩu').fill(DEMO.password);
  await page.getByRole('button', { name: 'Đăng nhập' }).click();

  // Login completed despite the injected 401 — the app landed in the shell
  // rather than surfacing a login error and staying on /login.
  await expect(page.getByRole('link', { name: 'Thư viện' })).toBeVisible();
  await expect(page).not.toHaveURL(/\/login/);
});
