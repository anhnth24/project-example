// Real-deployment upload scenarios (P2-20 Tasks 3/7): happy-path upload →
// indexed preview, throttled upload progress via delay-then-continue, and a
// real backend 413 under `MARKHAND_MAX_UPLOAD_BYTES=4096`.
//
// NO FETCH MOCK / NO `route.fulfill()` FOR ERRORS: network shaping may only
// delay then `route.continue()` to the real server (`delayThenContinue`). The
// 413 path is a genuine oversized body rejected by the process upload limit —
// never a synthesized Playwright response.
import { expect, test } from '@playwright/test';
import {
  contentCanary,
  delayThenContinue,
  ensureRouteRateWindow,
  login,
  markRouteRateHit,
  openRunCollection,
} from './support';

test.describe.configure({ mode: 'serial' });

test('uploading a file against the real backend reaches indexed, and its preview renders', async ({
  page,
}) => {
  // Real conversion + indexing genuinely takes longer than the mock suite's
  // synthetic timings; `test.slow()` triples Playwright's default per-test
  // timeout so the generous waits below have room to actually complete.
  test.slow();

  const fileName = `e2e-real-upload-${Date.now()}.txt`;
  const fileContents = `Real-deployment upload smoke test contents. ${contentCanary()}`;

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

  // 1. The row appears once the real `POST /uploads` response lands and the
  //    document list refetches.
  await expect(row).toBeVisible({ timeout: 15_000 });

  // Open the document's own preview so the badge there is proven too, not
  // just the list row's — same shape as `document-status-polling.spec.ts`'s
  // mock counterpart.
  await row.getByRole('button', { name: new RegExp(fileName) }).click();
  const previewBadge = page.locator('aside[aria-labelledby="library-preview-heading"]');

  // 2. Terminal state: converted -> indexing -> indexed, driven entirely by
  //    the real backend. Both the row badge and the open preview's own badge
  //    must agree.
  await expect(row.getByText('Đã lập chỉ mục')).toBeVisible({ timeout: 60_000 });
  await expect(previewBadge.getByText('Đã lập chỉ mục')).toBeVisible({ timeout: 60_000 });

  // 3. Preview actually renders the real converted content, not just the
  //    badge — proving the round trip through the real converter
  //    (`fileconv-core`) end to end.
  const previewMarkdown = page.getByTestId('document-preview-markdown');
  await expect(previewMarkdown).toBeVisible({ timeout: 15_000 });
  await expect(previewMarkdown).toContainText(fileContents);
});

test('a delayed POST /uploads shows upload progress then reaches indexed preview', async ({
  page,
}) => {
  test.slow();

  const fileName = `e2e-real-upload-throttled-${Date.now()}.txt`;
  const fileContents = `P2-20 throttled upload body ${contentCanary()} ${Date.now()} unique.`;

  await login(page);
  await openRunCollection(page);

  // Delay then continue to the real backend — never fulfill a synthetic body.
  // 1.5s is long enough for the XHR progress UI to paint on a tiny payload.
  await delayThenContinue(page, '**/api/v1/uploads', 1_500);

  await ensureRouteRateWindow('upload');
  await page.getByLabel('Chọn tệp để tải lên').setInputFiles({
    name: fileName,
    mimeType: 'text/plain',
    buffer: Buffer.from(fileContents),
  });

  await expect(page.getByRole('progressbar', { name: `Đang tải lên ${fileName}` })).toBeVisible({
    timeout: 5_000,
  });
  await expect(page.getByText('Đang tải lên…')).toBeVisible();

  markRouteRateHit('upload');

  const table = page.getByRole('table', { name: 'Danh sách tài liệu' });
  const row = table.getByRole('row').filter({ hasText: fileName });
  await expect(row).toBeVisible({ timeout: 30_000 });
  await row.getByRole('button', { name: new RegExp(fileName) }).click();
  await expect(row.getByText('Đã lập chỉ mục')).toBeVisible({ timeout: 60_000 });

  const previewMarkdown = page.getByTestId('document-preview-markdown');
  await expect(previewMarkdown).toBeVisible({ timeout: 15_000 });
  await expect(previewMarkdown).toContainText(fileContents);
});

test('a real oversized upload returns 413 and the too-large alert without an indexed row', async ({
  page,
}) => {
  test.slow();

  // Orchestrator sets MARKHAND_MAX_UPLOAD_BYTES=4096 for this process only.
  const fileName = `e2e-real-upload-413-${Date.now()}.bin`;
  const oversized = Buffer.alloc(5_000, 0x61);

  await login(page);
  await openRunCollection(page);

  // Oversized bodies are rejected by axum's DefaultBodyLimit before the
  // upload handler (and its route bucket) runs — no rate-window wait needed.
  const upload413 = page.waitForResponse((response) => {
    if (response.request().method() !== 'POST') return false;
    if (!new URL(response.url()).pathname.endsWith('/api/v1/uploads')) return false;
    return response.status() === 413;
  });

  await page.getByLabel('Chọn tệp để tải lên').setInputFiles({
    name: fileName,
    mimeType: 'application/octet-stream',
    buffer: oversized,
  });

  const rejected = await upload413;
  expect(rejected.status()).toBe(413);

  const alert = page.getByRole('alert');
  await expect(alert).toContainText(
    'Tệp vượt quá dung lượng cho phép. Hãy nén hoặc chia nhỏ tệp rồi thử lại.',
  );

  // Upload panel still shows the failed file; the library table must not grow
  // a phantom indexed document for this name.
  await expect(page.getByText(fileName)).toBeVisible();
  const table = page.getByRole('table', { name: 'Danh sách tài liệu' });
  await expect(table.getByRole('row').filter({ hasText: fileName })).toHaveCount(0);
});
