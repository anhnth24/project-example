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
//     credentials (login/logout/deep-link/401-refresh), library shell on the
//     run-scoped collection, and upload → indexed against the real
//     conversion/indexing pipeline. Broader flows (actions, org switch)
//     belong to later tasks — see P2-20 plan.
//
// FIXTURE GROUND TRUTH: admin/viewer credentials and collection IDs come from
// `deploy/scripts/web_e2e_real_fixture.py` setup output files referenced by
// `MARKHAND_E2E_REAL_CREDENTIALS_FILE` and `MARKHAND_E2E_REAL_FIXTURE_FILE`.
// There is no fixed POC seed fallback — parsers fail closed when those env
// paths are missing or still point at POC seed values.
import { expect, test, type Page } from '@playwright/test';
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

/** Stable default scanned by orchestrator artifact validation. */
export const DEFAULT_CONTENT_CANARY = 'P2-20-CONTENT-CANARY';

/**
 * Run-scoped content canary embedded in unique upload bodies. Prefer
 * `WEB_E2E_REAL_CONTENT_CANARY` when the orchestrator exports it; otherwise
 * the stable default so local/real runs stay aligned with
 * `WEB_E2E_REAL_CONTENT_CANARIES` validation.
 */
export function contentCanary(): string {
  const fromEnv = process.env.WEB_E2E_REAL_CONTENT_CANARY?.trim();
  return fromEnv && fromEnv.length > 0 ? fromEnv : DEFAULT_CONTENT_CANARY;
}

/**
 * Submits the login form with runtime admin credentials on the current page
 * (must already be `/login`, including `/login?next=…`) and waits until the
 * in-app shell (the "Thư viện" rail link) is mounted.
 */
export async function submitLoginForm(page: Page): Promise<void> {
  const { adminEmail, adminPassword } = runtimeCredentials();
  await page.getByLabel('Email').fill(adminEmail);
  await page.getByLabel('Mật khẩu').fill(adminPassword);
  await page.getByRole('button', { name: 'Đăng nhập' }).click();
  await expect(page.getByRole('link', { name: 'Thư viện' })).toBeVisible();
  await expect(page).not.toHaveURL(/\/login/);
}

/**
 * Logs in with runtime admin credentials via the real `/login` form and
 * waits until the in-app shell (the "Thư viện" rail link) is mounted.
 */
export async function login(page: Page): Promise<void> {
  await page.goto('/login');
  await submitLoginForm(page);
}

/**
 * Submits the login form with runtime viewer credentials (collection read,
 * no `doc.upload`) on the current page and waits for the in-app shell.
 */
export async function submitViewerLoginForm(page: Page): Promise<void> {
  const { viewerEmail, viewerPassword } = runtimeCredentials();
  await page.getByLabel('Email').fill(viewerEmail);
  await page.getByLabel('Mật khẩu').fill(viewerPassword);
  await page.getByRole('button', { name: 'Đăng nhập' }).click();
  await expect(page.getByRole('link', { name: 'Thư viện' })).toBeVisible();
  await expect(page).not.toHaveURL(/\/login/);
}

/**
 * Logs in with the runtime secondary viewer actor (no `doc.upload`).
 */
export async function loginAsViewer(page: Page): Promise<void> {
  await page.goto('/login');
  await submitViewerLoginForm(page);
}

/**
 * Logs out via the account menu (same interaction as mock `e2e/auth.spec.ts`).
 */
export async function logout(page: Page): Promise<void> {
  await page.getByRole('button', { name: /^Tài khoản:/ }).click();
  await page.getByRole('button', { name: 'Đăng xuất' }).click();
  await expect(page).toHaveURL(/\/login/);
  await expect(page.getByRole('link', { name: 'Thư viện' })).toHaveCount(0);
}

/**
 * Wall-clock of the last hit on the IP-scoped expensive route buckets
 * (`route:reindex:…` / `route:upload:…`). Shared across real specs in one
 * worker so Task 6/7 spacing survives file boundaries under
 * `MARKHAND_RATE_ROUTE_PER_MINUTE=1`.
 */
let lastReindexRouteHitAtMs = 0;
let lastUploadRouteHitAtMs = 0;

/** Pad past a 1-token / 60s bucket; slightly above 60s for CI scheduling jitter. */
const ROUTE_RATE_WINDOW_MS = 70_000;

/**
 * Headroom after a rate-window wait for login/upload/index/assertions.
 * `test.slow()` alone (~90s) is not enough when a prior hit forces a 65s sleep.
 */
const RATE_LIMITED_WORK_BUDGET_MS = 180_000;

/**
 * Arm a 5-minute ceiling for scenarios that may wait on the lowered route
 * bucket and then still need real convert/index time.
 */
export function armRateLimitedScenario(): void {
  test.setTimeout(5 * 60_000);
}

/**
 * Waits until the named expensive-route token bucket can accept another call.
 * Extends the current test timeout so the wait cannot consume the whole budget.
 */
export async function ensureRouteRateWindow(route: 'reindex' | 'upload'): Promise<void> {
  const last = route === 'reindex' ? lastReindexRouteHitAtMs : lastUploadRouteHitAtMs;
  if (last === 0) return;
  const waitMs = ROUTE_RATE_WINDOW_MS - (Date.now() - last);
  if (waitMs > 0) {
    const info = test.info();
    test.setTimeout(Math.max(info.timeout, waitMs + RATE_LIMITED_WORK_BUDGET_MS));
    await new Promise((resolve) => setTimeout(resolve, waitMs));
  }
}

/** Records that a real reindex/upload route call consumed a rate-limit token. */
export function markRouteRateHit(route: 'reindex' | 'upload'): void {
  const now = Date.now();
  if (route === 'reindex') {
    lastReindexRouteHitAtMs = now;
  } else {
    lastUploadRouteHitAtMs = now;
  }
}

export interface RealUploadFile {
  name: string;
  mimeType: string;
  buffer: Buffer;
}

function isUploadsPost(response: {
  url: () => string;
  request: () => { method: () => string };
}): boolean {
  if (response.request().method() !== 'POST') return false;
  return new URL(response.url()).pathname.endsWith('/api/v1/uploads');
}

/**
 * Selects a file in the upload panel, waits for the real `POST /uploads`, and
 * marks the upload route bucket from the response time (not `setInputFiles`).
 * Retries once after a full rate window when the lowered route bucket returns 429.
 *
 * `duringRequest` runs after the file is selected and before the response is
 * awaited — use it to assert in-flight UI (e.g. progress) under `delayThenContinue`.
 */
export async function uploadFileViaPanel(
  page: Page,
  file: RealUploadFile,
  expectedStatus: number,
  duringRequest?: () => Promise<void>,
): Promise<void> {
  for (let attempt = 0; attempt < 2; attempt += 1) {
    await ensureRouteRateWindow('upload');
    const responsePromise = page.waitForResponse(isUploadsPost);
    await page.getByLabel('Chọn tệp để tải lên').setInputFiles(file);
    if (duringRequest && attempt === 0) {
      await duringRequest();
    }
    const response = await responsePromise;
    markRouteRateHit('upload');
    if (response.status() === expectedStatus) {
      return;
    }
    if (response.status() === 429 && attempt === 0 && expectedStatus !== 429) {
      continue;
    }
    throw new Error(
      `POST /uploads expected status ${expectedStatus}, got ${response.status()} (attempt ${attempt + 1})`,
    );
  }
}

/** Observed real HTTP statuses for the one-shot invalid-bearer `/auth/me` recovery. */
export interface AuthMeRefreshRecovery {
  firstMeStatus: number;
  refreshStatus: number;
  retriedMeStatus: number;
}

/**
 * Installs a one-shot Playwright route on `GET /api/v1/auth/me` that replaces
 * only the first request's bearer with an invalid value, then
 * `route.continue()`s to the real server. Subsequent `/auth/me` calls are
 * forwarded unchanged. Never fulfills synthetic auth responses.
 *
 * Returns the observed real statuses (401 → refresh 200 → retried me 200)
 * after `trigger` completes. Failed-refresh → `/login` bounce remains covered
 * by `api/client` + `api/session` unit tests rather than a second real-stack
 * scenario here.
 */
export async function withOneShotInvalidAuthMeBearer(
  page: Page,
  trigger: () => Promise<void>,
): Promise<AuthMeRefreshRecovery> {
  let corruptedMe = false;
  await page.route('**/api/v1/auth/me', async (route) => {
    if (route.request().method() !== 'GET' || corruptedMe) {
      await route.continue();
      return;
    }
    corruptedMe = true;
    const headers = {
      ...route.request().headers(),
      authorization: 'Bearer e2e-real-invalid-access-token',
    };
    await route.continue({ headers });
  });

  const firstMe401 = page.waitForResponse(
    (response) =>
      response.url().includes('/api/v1/auth/me') &&
      response.request().method() === 'GET' &&
      response.status() === 401,
  );
  const refresh200 = page.waitForResponse(
    (response) =>
      response.url().includes('/api/v1/auth/refresh') &&
      response.request().method() === 'POST' &&
      response.status() === 200,
  );
  const retriedMe200 = page.waitForResponse(
    (response) =>
      response.url().includes('/api/v1/auth/me') &&
      response.request().method() === 'GET' &&
      response.status() === 200,
  );

  await trigger();
  const [firstMe, refresh, retriedMe] = await Promise.all([firstMe401, refresh200, retriedMe200]);

  return {
    firstMeStatus: firstMe.status(),
    refreshStatus: refresh.status(),
    retriedMeStatus: retriedMe.status(),
  };
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
