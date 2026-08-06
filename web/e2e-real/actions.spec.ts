// Real-backend document mutation scenarios (P2-20 Task 6): reindex on an
// indexed document, retry from the fixture failed document, and delete with
// confirm. No fetch mock, no `route.fulfill()`, no auth bypass.
//
// Reindex/retry both hit `POST /documents/{id}/reindex`. The orchestrator sets
// `MARKHAND_RATE_ROUTE_PER_MINUTE=1` for Task 7's deterministic 429, so this
// file spaces the two successful reindex calls by one token-bucket window.
import { expect, test } from '@playwright/test';
import { login, openRunCollection, runtimeFixture } from './support';

/** Wall-clock of the last successful reindex in this worker (serial real project). */
let lastSuccessfulReindexAtMs = 0;

/**
 * Waits until the route token bucket can accept another reindex. Capacity is 1
 * token / 60s under the real orchestrator's lowered knob; pad to 65s.
 */
async function ensureReindexRateWindow(): Promise<void> {
  if (lastSuccessfulReindexAtMs === 0) return;
  const elapsed = Date.now() - lastSuccessfulReindexAtMs;
  const waitMs = 65_000 - elapsed;
  if (waitMs > 0) {
    await new Promise((resolve) => setTimeout(resolve, waitMs));
  }
}

function markSuccessfulReindex(): void {
  lastSuccessfulReindexAtMs = Date.now();
}

test.describe.configure({ mode: 'serial' });

test('reindex on an indexed document shows the enqueue success notice', async ({ page }) => {
  test.slow();

  const fileName = `e2e-real-actions-reindex-${Date.now()}.txt`;
  const fileContents = `P2-20 actions reindex body ${Date.now()} unique.`;

  await login(page);
  await openRunCollection(page);

  await page.getByLabel('Chọn tệp để tải lên').setInputFiles({
    name: fileName,
    mimeType: 'text/plain',
    buffer: Buffer.from(fileContents),
  });

  const table = page.getByRole('table', { name: 'Danh sách tài liệu' });
  const row = table.getByRole('row').filter({ hasText: fileName });
  await expect(row).toBeVisible({ timeout: 15_000 });
  await row.getByRole('button', { name: new RegExp(fileName) }).click();
  await expect(row.getByText('Đã lập chỉ mục')).toBeVisible({ timeout: 60_000 });

  await ensureReindexRateWindow();
  await page.getByRole('button', { name: 'Lập chỉ mục lại' }).click();
  await expect(page.getByText('Đã đưa tài liệu vào hàng đợi lập chỉ mục.')).toBeVisible({
    timeout: 15_000,
  });
  markSuccessfulReindex();
});

test('fixture failed document shows the failed badge and retry enqueues reindex', async ({
  page,
}) => {
  test.slow();

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

  await ensureReindexRateWindow();
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
  markSuccessfulReindex();
});

test('delete with confirm removes the document row after refetch', async ({ page }) => {
  test.slow();

  const fileName = `e2e-real-actions-delete-${Date.now()}.txt`;
  const fileContents = `P2-20 actions delete body ${Date.now()} unique.`;

  await login(page);
  await openRunCollection(page);

  await page.getByLabel('Chọn tệp để tải lên').setInputFiles({
    name: fileName,
    mimeType: 'text/plain',
    buffer: Buffer.from(fileContents),
  });

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
