// Shared helpers for the real-deployment half of P2-15 / P2-20.
//
// SCOPE OF THIS SUITE (read this first):
//   - This drives a real Chromium against a *real* fileconv-server — the
//     compose dev stack (Postgres/Qdrant/MinIO/embedding) plus a built
//     `web/dist` served by the server itself (see `deploy/README.md`'s "Web
//     SPA static serving" section and `crates/server/src/spa.rs`). There is
//     no fetch mock here: every request is a real HTTP round-trip.
//   - Only `deploy/scripts/web-e2e-real.sh` is meant to run this suite: it
//     brings the stack up, migrates, seeds the run-scoped fixture, builds the
//     SPA, starts fileconv-server plus convert/index/embedding/delete workers,
//     and only then runs `playwright test --project=real`. Running this suite
//     any other way requires reproducing all of that by hand.
//   - Credentials and collection IDs come from runtime files exported by the
//     orchestrator (`MARKHAND_E2E_REAL_CREDENTIALS_FILE` /
//     `MARKHAND_E2E_REAL_FIXTURE_FILE`). There is no fixed-seed fallback to
//     `admin@poc.example` / "POC Library".
import { expect, type Page } from '@playwright/test';
import {
  loadRuntimeCredentials,
  loadRuntimeFixture,
  type RuntimeCredentials,
  type RuntimeFixture,
} from './runtime';

/** Load runtime credentials from the orchestrator-exported JSON path. */
export function runtimeCredentials(): RuntimeCredentials {
  return loadRuntimeCredentials(process.env);
}

/** Load run-scoped fixture names/IDs from the orchestrator-exported JSON path. */
export function runtimeFixture(): RuntimeFixture {
  return loadRuntimeFixture(process.env);
}

/**
 * Logs in with the runtime admin account via the real `/login` form and
 * waits until the in-app shell (the "Thư viện" rail link) is mounted.
 */
export async function login(page: Page): Promise<void> {
  const credentials = runtimeCredentials();
  await page.goto('/login');
  await page.getByLabel('Email').fill(credentials.adminEmail);
  await page.getByLabel('Mật khẩu').fill(credentials.adminPassword);
  await page.getByRole('button', { name: 'Đăng nhập' }).click();
  await expect(page.getByRole('link', { name: 'Thư viện' })).toBeVisible();
  await expect(page).not.toHaveURL(/\/login/);
}

/**
 * Logs out via the rail account menu and waits until `/login` is shown with
 * no library rail link (mirrors mock `e2e/auth.spec.ts`).
 */
export async function logout(page: Page): Promise<void> {
  await page.getByRole('button', { name: /^Tài khoản:/ }).click();
  await page.getByRole('button', { name: 'Đăng xuất' }).click();
  await expect(page).toHaveURL(/\/login/);
  await expect(page.getByRole('link', { name: 'Thư viện' })).toHaveCount(0);
}

/**
 * Opens the run-scoped collection from the library nav using the fixture
 * collection name (replaces the old fixed "POC Library" helper).
 */
export async function openRunCollection(page: Page): Promise<void> {
  const fixture = runtimeFixture();
  await page.getByRole('link', { name: 'Thư viện' }).click();
  await page
    .getByRole('navigation', { name: 'Điều hướng bộ sưu tập' })
    .getByRole('link', { name: fixture.collectionName })
    .click();
  // The upload panel only renders once a collection is open — a reliable
  // "collection is loaded" signal (same rationale as `openEmployeeHandbook`).
  await expect(page.getByLabel('Chọn tệp để tải lên')).toBeVisible();
}

/**
 * Delay matching requests briefly, then `route.continue()` to the real
 * backend. Never synthesizes a success/authorization response.
 */
export async function delayThenContinue(
  page: Page,
  urlGlob: string,
  delayMs: number,
): Promise<void> {
  await page.route(urlGlob, async (route) => {
    if (delayMs > 0) {
      await new Promise((resolve) => setTimeout(resolve, delayMs));
    }
    await route.continue();
  });
}
