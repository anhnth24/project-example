// Real-backend auth scenarios (P2-20 Task 4): login, logout, anonymous
// deep-link `?next=` preservation through successful login, and one real
// 401 recovered via refresh/retry.
//
// No fetch mock, no `route.fulfill()`, no auth bypass. Network shaping may
// only rewrite a request header and `route.continue()` to the real server.
// Failed-refresh → `/login` bounce is covered by unit tests in
// `web/src/api/client.test.ts` / session manager — not re-asserted here.
import { expect, test } from '@playwright/test';
import {
  login,
  logout,
  runtimeFixture,
  submitLoginForm,
  withOneShotInvalidAuthMeBearer,
} from './support';

test('login with runtime credentials shows the in-app shell', async ({ page }) => {
  await page.goto('/login');
  await expect(page.getByRole('heading', { name: 'Đăng nhập vào Folyvo' })).toBeVisible();

  await submitLoginForm(page);

  await expect(page.getByRole('link', { name: 'Thư viện' })).toBeVisible();
  await expect(page).not.toHaveURL(/\/login/);
});

test('logout returns to /login without the library rail', async ({ page }) => {
  await login(page);
  await logout(page);

  await expect(page).toHaveURL(/\/login/);
  await expect(page.getByRole('heading', { name: 'Đăng nhập vào Folyvo' })).toBeVisible();
  await expect(page.getByRole('link', { name: 'Thư viện' })).toHaveCount(0);
});

test('anonymous deep-link to the run collection preserves ?next= through login', async ({
  page,
}) => {
  const { collectionId } = runtimeFixture();
  const intended = `/library/${collectionId}`;

  // Fresh context is anonymous. ProtectedRoute bounces to /login, preserving
  // the intended location as `?next=` (sanitized by PublicOnlyRoute after login).
  await page.goto(intended);

  await expect(page).toHaveURL(/\/login\?next=/);
  await expect(page.getByRole('heading', { name: 'Đăng nhập vào Folyvo' })).toBeVisible();

  const next = new URL(page.url()).searchParams.get('next');
  expect(next).toBe(intended);

  // Stay on `/login?next=…` — do not navigate to bare `/login` or next is lost.
  await submitLoginForm(page);

  await expect(page).toHaveURL((url) => url.pathname === intended);
  await expect(page.getByRole('link', { name: 'Thư viện' })).toBeVisible();
  await expect(page.getByLabel('Chọn tệp để tải lên')).toBeVisible();
});

test('a one-shot invalid bearer on GET /auth/me recovers via real refresh without /login bounce', async ({
  page,
}) => {
  // After POST /auth/login installs tokens, the follow-up GET /auth/me is the
  // first authenticated read. Corrupt only that first request's bearer and
  // continue() to the real server so we observe a genuine 401, then the
  // client's single-flight refresh + retry against the real backend.
  await page.goto('/login');
  await expect(page.getByRole('heading', { name: 'Đăng nhập vào Folyvo' })).toBeVisible();

  const recovery = await withOneShotInvalidAuthMeBearer(page, async () => {
    await submitLoginForm(page);
  });

  expect(recovery.firstMeStatus).toBe(401);
  expect(recovery.refreshStatus).toBe(200);
  expect(recovery.retriedMeStatus).toBe(200);

  await expect(page.getByRole('link', { name: 'Thư viện' })).toBeVisible();
  await expect(page).not.toHaveURL(/\/login/);
});
