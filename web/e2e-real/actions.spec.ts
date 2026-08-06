// Real-backend document mutation + action-error scenarios (P2-20 Tasks 6–7):
// reindex, retry-from-failed, delete, viewer 403 on reindex, and real 429
// under the orchestrator's lowered `MARKHAND_RATE_ROUTE_PER_MINUTE`.
//
// No fetch mock, no `route.fulfill()`, no auth bypass. Rate-limit tokens for
// `route:reindex:…` / `route:upload:…` are shared via `support.ts` helpers.
import { expect, test, type Response } from '@playwright/test';
import {
  contentCanary,
  ensureRouteRateWindow,
  login,
  loginAsViewer,
  markRouteRateHit,
  openRunCollection,
  runtimeFixture,
  armRateLimitedScenario,
} from './support';

test.describe.configure({ mode: 'serial' });

/** Assert the lowered route bucket produced this 429 (not IP/user/org). */
async function assertRouteScoped429(response: Response): Promise<void> {
  expect(response.status()).toBe(429);
  const retryAfter = response.headers()['retry-after'];
  expect(retryAfter).toBeTruthy();
  const body = (await response.json()) as {
    details?: { scope?: string; retryAfterSeconds?: number };
  };
  expect(body.details?.scope).toBe('route');
  expect(String(body.details?.retryAfterSeconds)).toBe(String(retryAfter));
}

test('reindex on an indexed document shows the enqueue success notice', async ({ page }) => {
  test.slow();
  armRateLimitedScenario();

  const fileName = `e2e-real-actions-reindex-${Date.now()}.txt`;
  const fileContents = `P2-20 actions reindex body ${contentCanary()} ${Date.now()} unique.`;

  await login(page);
  await openRunCollection(page);

  await ensureRouteRateWindow('upload');
  await page.getByLabel('Chọn tệp để tải lên').setInputFiles({
    name: fileName,
    mimeType: 'text/plain',
    buffer: Buffer.from(fileContents),
  });
  markRouteRateHit('upload');

  const table = page.getByRole('table', { name: 'Danh sách tài liệu' });
  const row = table.getByRole('row').filter({ hasText: fileName });
  await expect(row).toBeVisible({ timeout: 15_000 });
  await row.getByRole('button', { name: new RegExp(fileName) }).click();
  await expect(row.getByText('Đã lập chỉ mục')).toBeVisible({ timeout: 60_000 });

  await ensureRouteRateWindow('reindex');
  await page.getByRole('button', { name: 'Lập chỉ mục lại' }).click();
  await expect(page.getByText('Đã đưa tài liệu vào hàng đợi lập chỉ mục.')).toBeVisible({
    timeout: 15_000,
  });
  markRouteRateHit('reindex');
});

test('fixture failed document shows the failed badge and retry enqueues reindex', async ({
  page,
}) => {
  test.slow();
  armRateLimitedScenario();

  const { collectionId, failedDocumentId, runId } = runtimeFixture();
  const failedTitle = `E2E Failed ${runId}`;

  await login(page);
  // Deep-link selects the runtime fixture failed row by ID (not list order).
  await page.goto(`/library/${collectionId}?doc=${failedDocumentId}`);
  await expect(page.getByLabel('Chọn tệp để tải lên')).toBeVisible({ timeout: 15_000 });

  const table = page.getByRole('table', { name: 'Danh sách tài liệu' });
  const row = table.getByRole('row').filter({ hasText: failedTitle });
  await expect(row).toBeVisible({ timeout: 15_000 });
  await expect(row.getByText('Lỗi chuyển đổi')).toBeVisible();

  // Actions mount in the preview panel for the ?doc= selection.
  await expect(page.getByRole('button', { name: 'Thử lại lập chỉ mục' })).toBeVisible();

  await ensureRouteRateWindow('reindex');
  const reindexResponsePromise = page.waitForResponse((response) => {
    if (response.request().method() !== 'POST') return false;
    return new URL(response.url()).pathname === `/api/v1/documents/${failedDocumentId}/reindex`;
  });

  await page.getByRole('button', { name: 'Thử lại lập chỉ mục' }).click();
  const reindexResponse = await reindexResponsePromise;
  expect(reindexResponse.status()).toBe(200);

  await expect(page.getByText('Đã đưa tài liệu vào hàng đợi lập chỉ mục.')).toBeVisible({
    timeout: 15_000,
  });
  markRouteRateHit('reindex');
});

test('delete with confirm removes the document row after refetch', async ({ page }) => {
  test.slow();
  armRateLimitedScenario();

  const fileName = `e2e-real-actions-delete-${Date.now()}.txt`;
  const fileContents = `P2-20 actions delete body ${contentCanary()} ${Date.now()} unique.`;

  await login(page);
  await openRunCollection(page);

  await ensureRouteRateWindow('upload');
  await page.getByLabel('Chọn tệp để tải lên').setInputFiles({
    name: fileName,
    mimeType: 'text/plain',
    buffer: Buffer.from(fileContents),
  });
  markRouteRateHit('upload');

  const table = page.getByRole('table', { name: 'Danh sách tài liệu' });
  const row = table.getByRole('row').filter({ hasText: fileName });
  await expect(row).toBeVisible({ timeout: 15_000 });
  await row.getByRole('button', { name: new RegExp(fileName) }).click();

  // Tombstone is allowed from any active state — no need to wait for indexed.
  await page.getByRole('button', { name: 'Xóa', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Xóa tài liệu này?' });
  await expect(dialog).toBeVisible();
  await dialog.getByRole('button', { name: 'Xóa tài liệu' }).click();

  // List refetches after delete; server list excludes tombstoned/purged rows.
  await expect(table.getByRole('button', { name: new RegExp(fileName) })).toHaveCount(0, {
    timeout: 30_000,
  });
});

test('viewer reindex is denied with a real HTTP 403 and the document remains', async ({ page }) => {
  test.slow();
  armRateLimitedScenario();

  const fileName = `e2e-real-actions-403-${Date.now()}.txt`;
  const fileContents = `P2-20 actions 403 body ${contentCanary()} ${Date.now()} unique.`;
  const { collectionId } = runtimeFixture();

  // Admin creates a durable indexed row the viewer can see but cannot reindex.
  await login(page);
  await openRunCollection(page);

  await ensureRouteRateWindow('upload');
  await page.getByLabel('Chọn tệp để tải lên').setInputFiles({
    name: fileName,
    mimeType: 'text/plain',
    buffer: Buffer.from(fileContents),
  });
  markRouteRateHit('upload');

  const table = page.getByRole('table', { name: 'Danh sách tài liệu' });
  const row = table.getByRole('row').filter({ hasText: fileName });
  await expect(row).toBeVisible({ timeout: 15_000 });
  await row.getByRole('button', { name: new RegExp(fileName) }).click();
  await expect(row.getByText('Đã lập chỉ mục')).toBeVisible({ timeout: 60_000 });

  const selectedDoc = new URL(page.url()).searchParams.get('doc');
  expect(selectedDoc, 'expected ?doc= after selecting the indexed row').toBeTruthy();

  await page.getByRole('button', { name: /^Tài khoản:/ }).click();
  await page.getByRole('button', { name: 'Đăng xuất' }).click();
  await expect(page).toHaveURL(/\/login/);

  // Permission check runs after the reindex route bucket — wait so a prior
  // reindex does not turn this into a 429 instead of 403.
  await ensureRouteRateWindow('reindex');

  await loginAsViewer(page);
  // Do not use openRunCollection: viewers lack `doc.upload`, so the upload
  // control that helper asserts is not rendered.
  await page.goto(`/library/${collectionId}?doc=${selectedDoc}`);

  await expect(row).toBeVisible({ timeout: 15_000 });
  await expect(page.getByRole('button', { name: 'Lập chỉ mục lại' })).toBeVisible();

  const reindex403 = page.waitForResponse((response) => {
    if (response.request().method() !== 'POST') return false;
    if (!/\/api\/v1\/documents\/[^/]+\/reindex$/.test(new URL(response.url()).pathname)) {
      return false;
    }
    return response.status() === 403;
  });

  await page.getByRole('button', { name: 'Lập chỉ mục lại' }).click();
  const denied = await reindex403;
  expect(denied.status()).toBe(403);
  // Denied path still consumed the expensive-route token (check_route before
  // require_permission) — record it so the 429 scenario can reuse the empty
  // bucket without an extra wait when ordered next.
  markRouteRateHit('reindex');

  await expect(page.getByRole('alert')).toContainText(
    'Bạn không có quyền thực hiện thao tác này với tài liệu này.',
  );
  await expect(row).toBeVisible();
  await expect(row.getByText('Đã lập chỉ mục')).toBeVisible();
});

test('reindex under the lowered route limit returns a real 429 with retry-after copy', async ({
  page,
}) => {
  test.slow();
  armRateLimitedScenario();

  const fileName = `e2e-real-actions-429-${Date.now()}.txt`;
  const fileContents = `P2-20 actions 429 body ${contentCanary()} ${Date.now()} unique.`;

  await login(page);
  await openRunCollection(page);

  await ensureRouteRateWindow('upload');
  await page.getByLabel('Chọn tệp để tải lên').setInputFiles({
    name: fileName,
    mimeType: 'text/plain',
    buffer: Buffer.from(fileContents),
  });
  markRouteRateHit('upload');

  const table = page.getByRole('table', { name: 'Danh sách tài liệu' });
  const row = table.getByRole('row').filter({ hasText: fileName });
  await expect(row).toBeVisible({ timeout: 15_000 });
  await row.getByRole('button', { name: new RegExp(fileName) }).click();
  await expect(row.getByText('Đã lập chỉ mục')).toBeVisible({ timeout: 60_000 });

  // Prefer exhausting the bucket with a successful reindex then an immediate
  // second call. If a prior test (viewer 403) already emptied the bucket and
  // we have not waited, the first call itself is the real 429.
  const reindexPath = /\/api\/v1\/documents\/[^/]+\/reindex$/;
  const firstReindex = page.waitForResponse((response) => {
    if (response.request().method() !== 'POST') return false;
    return reindexPath.test(new URL(response.url()).pathname);
  });

  await page.getByRole('button', { name: 'Lập chỉ mục lại' }).click();
  const first = await firstReindex;
  markRouteRateHit('reindex');

  if (first.status() === 200) {
    await expect(page.getByText('Đã đưa tài liệu vào hàng đợi lập chỉ mục.')).toBeVisible({
      timeout: 15_000,
    });

    const secondReindex = page.waitForResponse((response) => {
      if (response.request().method() !== 'POST') return false;
      return reindexPath.test(new URL(response.url()).pathname) && response.status() === 429;
    });
    await page.getByRole('button', { name: 'Lập chỉ mục lại' }).click();
    const limited = await secondReindex;
    await assertRouteScoped429(limited);
    markRouteRateHit('reindex');
  } else {
    await assertRouteScoped429(first);
  }

  await expect(page.getByRole('alert')).toHaveText(
    /Quá nhiều yêu cầu\. Vui lòng thử lại sau (\d+ giây|ít phút|khoảng \d+ phút)\./,
  );
});
