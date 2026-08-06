// Real-backend auth scenarios (P2-20 Task 4): login, logout, anonymous
// deep-link `?next=` preservation, and one real 401 recovered via refresh.
//
// No fetch mock, no `route.fulfill()`, no auth bypass. Network shaping may
// only rewrite a request header and `route.continue()` to the real server.
// Failed-refresh → `/login` bounce is covered by unit tests in
// `web/src/api/client.test.ts` / session manager — not re-asserted here.
import { expect, test, type Response } from '@playwright/test';
import { login, logout, runtimeCredentials, runtimeFixture } from './support';

test('login with runtime credentials shows the app shell', async ({ page }) => {
  await page.goto('/login');
  await expect(page.getByRole('heading', { name: 'Đăng nhập vào Folyvo' })).toBeVisible();

  await login(page);

  await expect(page.getByRole('link', { name: 'Thư viện' })).toBeVisible();
  await expect(page).not.toHaveURL(/\/login/);
});

test('logout returns to /login with no library rail', async ({ page }) => {
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

  // Fresh context is anonymous. ProtectedRoute bounces to /login with the
  // intended location preserved as `?next=` (path + search).
  await page.goto(intended);

  await expect(page).toHaveURL(/\/login\?next=/);
  await expect(page.getByRole('heading', { name: 'Đăng nhập vào Folyvo' })).toBeVisible();

  const next = new URL(page.url()).searchParams.get('next');
  expect(next).toBe(intended);

  // Stay on the redirected /login?next=… page — do not `goto('/login')`,
  // which would drop the preserved next target.
  const { adminEmail, adminPassword } = runtimeCredentials();
  await page.getByLabel('Email').fill(adminEmail);
  await page.getByLabel('Mật khẩu').fill(adminPassword);
  await page.getByRole('button', { name: 'Đăng nhập' }).click();

  // PublicOnlyRoute navigates to sanitizeNextPath(?next=) after auth.
  await expect(page).toHaveURL((url) => url.pathname === intended);
  await expect(page.getByRole('link', { name: 'Thư viện' })).toBeVisible();
  await expect(page.getByLabel('Chọn tệp để tải lên')).toBeVisible();
});

test('a real backend 401 on GET /auth/me is recovered via refresh without bouncing to /login', async ({
  page,
}) => {
  // One-shot: corrupt only the first GET /auth/me bearer, then continue to
  // the real server. The client must POST /auth/refresh (real 200) and retry
  // GET /auth/me (real 200). Never fulfill() any of those responses.
  let corruptedMe = false;
  await page.route('**/api/v1/auth/me', async (route) => {
    if (route.request().method() !== 'GET' || corruptedMe) {
      await route.continue();
      return;
    }
    corruptedMe = true;
    const headers = {
      ...route.request().headers(),
      authorization: 'Bearer e2e-invalid-access-token',
    };
    await route.continue({ headers });
  });

  const authResponses: { method: string; path: string; status: number }[] = [];
  const onResponse = (response: Response) => {
    const url = new URL(response.url());
    const path = url.pathname;
    if (path === '/api/v1/auth/me' || path === '/api/v1/auth/refresh') {
      authResponses.push({
        method: response.request().method(),
        path,
        status: response.status(),
      });
    }
  };
  page.on('response', onResponse);

  await page.goto('/login');
  await expect(page.getByRole('heading', { name: 'Đăng nhập vào Folyvo' })).toBeVisible();

  const { adminEmail, adminPassword } = runtimeCredentials();
  await page.getByLabel('Email').fill(adminEmail);
  await page.getByLabel('Mật khẩu').fill(adminPassword);
  await page.getByRole('button', { name: 'Đăng nhập' }).click();

  // Login completed despite the injected invalid bearer — shell mounts and
  // we never bounce back to /login.
  await expect(page.getByRole('link', { name: 'Thư viện' })).toBeVisible();
  await expect(page).not.toHaveURL(/\/login/);

  page.off('response', onResponse);

  const meGets = authResponses.filter((r) => r.method === 'GET' && r.path === '/api/v1/auth/me');
  const refreshes = authResponses.filter(
    (r) => r.method === 'POST' && r.path === '/api/v1/auth/refresh',
  );

  expect(corruptedMe).toBe(true);
  expect(meGets.length).toBeGreaterThanOrEqual(2);
  expect(meGets[0]?.status).toBe(401);
  expect(refreshes.some((r) => r.status === 200)).toBe(true);
  expect(meGets.slice(1).some((r) => r.status === 200)).toBe(true);

  // Ordering: first me 401, then refresh 200, then a later me 200.
  const firstMe401 = authResponses.findIndex(
    (r) => r.method === 'GET' && r.path === '/api/v1/auth/me' && r.status === 401,
  );
  const refresh200 = authResponses.findIndex(
    (r, i) =>
      i > firstMe401 &&
      r.method === 'POST' &&
      r.path === '/api/v1/auth/refresh' &&
      r.status === 200,
  );
  const retriedMe200 = authResponses.findIndex(
    (r, i) =>
      i > refresh200 && r.method === 'GET' && r.path === '/api/v1/auth/me' && r.status === 200,
  );
  expect(firstMe401).toBeGreaterThanOrEqual(0);
  expect(refresh200).toBeGreaterThan(firstMe401);
  expect(retriedMe200).toBeGreaterThan(refresh200);
});
