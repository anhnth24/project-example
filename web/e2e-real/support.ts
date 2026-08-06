// Shared helpers for the real-deployment Playwright suite (P2-20).
//
// SCOPE OF THIS SUITE (read this first):
//   - This drives a real Chromium against a *real* fileconv-server — the
//     compose dev stack (Postgres/Qdrant/MinIO/embedding) plus a built
//     `web/dist` served by the server itself (see `deploy/README.md`'s "Web
//     SPA static serving" section and `crates/server/src/spa.rs`). There is
//     no fetch mock here: every request is a real HTTP round-trip.
//   - Only `deploy/scripts/web-e2e-real.sh` is meant to run this suite: it
//     brings the stack up, migrates, seeds, builds the SPA, starts
//     fileconv-server plus convert/index/embedding/delete workers, and only
//     then runs `playwright test --project=real`. Running this suite any
//     other way requires reproducing all of that by hand.
//   - Smoke scope only, deliberately small: login against runtime fixture
//     credentials, library shell on the run-scoped collection, and upload →
//     indexed against the real conversion/indexing pipeline. Broader flows
//     (actions, org switch) belong to later tasks — see P2-20 plan.
//
// FIXTURE GROUND TRUTH: admin/viewer credentials and collection IDs come from
// `deploy/scripts/web_e2e_real_fixture.py` setup output files referenced by
// `MARKHAND_E2E_REAL_CREDENTIALS_FILE` and `MARKHAND_E2E_REAL_FIXTURE_FILE`.
// There is no fixed POC seed fallback — parsers fail closed when those env
// paths are missing or still point at POC seed values.
import { expect, type Page } from '@playwright/test';
import {
  loadRuntimeCredentials,
  loadRuntimeFixture,
  type RuntimeCredentials,
  type RuntimeFixture,
} from './runtime';

export function runtimeCredentials(): RuntimeCredentials {
  return loadRuntimeCredentials(process.env as Record<string, string | undefined>);
}

export function runtimeFixture(): RuntimeFixture {
  return loadRuntimeFixture(process.env as Record<string, string | undefined>);
}

/**
 * Logs in with runtime admin credentials via the real `/login` form and
 * waits until the in-app shell (the "Thư viện" rail link) is mounted.
 */
export async function login(page: Page): Promise<void> {
  const { adminEmail, adminPassword } = runtimeCredentials();
  await page.goto('/login');
  await page.getByLabel('Email').fill(adminEmail);
  await page.getByLabel('Mật khẩu').fill(adminPassword);
  await page.getByRole('button', { name: 'Đăng nhập' }).click();
  await expect(page.getByRole('link', { name: 'Thư viện' })).toBeVisible();
  await expect(page).not.toHaveURL(/\/login/);
}

/**
 * Logs out via the account menu (same interaction as mock `e2e/auth.spec.ts`).
 */
export async function logout(page: Page): Promise<void> {
  await page.getByRole('button', { name: /^Tài khoản:/ }).click();
  await page.getByRole('button', { name: 'Đăng xuất' }).click();
  await expect(page).toHaveURL(/\/login/);
}

/**
 * Opens the run-scoped collection from the library's collection nav.
 */
export async function openRunCollection(page: Page): Promise<void> {
  const { collectionName } = runtimeFixture();
  await page.getByRole('link', { name: 'Thư viện' }).click();
  await page
    .getByRole('navigation', { name: 'Điều hướng bộ sưu tập' })
    .getByRole('link', { name: collectionName })
    .click();
  await expect(page.getByLabel('Chọn tệp để tải lên')).toBeVisible();
}

/**
 * Delays matching requests then forwards them to the real backend. Never
 * fulfills synthetic responses — only `route.continue()` is allowed.
 */
export async function delayThenContinue(
  page: Page,
  urlGlob: string,
  delayMs: number,
): Promise<void> {
  await page.route(urlGlob, async (route) => {
    await new Promise((resolve) => setTimeout(resolve, delayMs));
    await route.continue();
  });
}
