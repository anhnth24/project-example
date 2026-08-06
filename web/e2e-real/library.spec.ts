// Real-backend library scenarios (P2-20 Task 5): run-collection navigation,
// upload → indexed preview against real workers, and Markdown download via
// real `issueDownloadCapability` + `redeemDownload`.
//
// No fetch mock, no `route.fulfill()`, no auth bypass. Upload and indexing
// wait on real convert/index/embedding workers (same stack contract as
// `upload.spec.ts`). Download completion is the browser download event plus
// real 200s — DocumentRowActions has no success Notice for download.
import { expect, test } from '@playwright/test';
import {
  contentCanary,
  ensureRouteRateWindow,
  login,
  markRouteRateHit,
  openRunCollection,
} from './support';

test('navigating to the run collection shows the upload panel', async ({ page }) => {
  await login(page);
  await openRunCollection(page);

  await expect(page.getByLabel('Chọn tệp để tải lên')).toBeVisible();
});

test('uploading a unique text document indexes and previews markdown', async ({ page }) => {
  // Real conversion + indexing needs headroom beyond the mock suite's instant
  // transitions (`test.slow()` triples the default timeout).
  test.slow();

  const fileName = `e2e-real-library-preview-${Date.now()}.txt`;
  const fileContents = `P2-20 library preview body ${contentCanary()} ${Date.now()} unique.`;

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

  const previewMarkdown = page.getByTestId('document-preview-markdown');
  await expect(previewMarkdown).toBeVisible({ timeout: 15_000 });
  await expect(previewMarkdown).toContainText(fileContents);
});

test('downloading Markdown issues a capability, redeems it, and does not log the token', async ({
  page,
}) => {
  test.slow();

  const fileName = `e2e-real-library-download-${Date.now()}.txt`;
  const fileContents = `P2-20 library download body ${contentCanary()} ${Date.now()} unique.`;

  const consoleLines: string[] = [];
  page.on('console', (msg) => {
    consoleLines.push(msg.text());
  });

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
  await expect(page.getByRole('button', { name: 'Tải xuống' })).toBeEnabled({ timeout: 15_000 });

  const issuePromise = page.waitForResponse((response) => {
    if (response.request().method() !== 'POST') return false;
    return /\/api\/v1\/documents\/[^/]+\/versions\/[^/]+\/download-capability$/.test(
      new URL(response.url()).pathname,
    );
  });
  const redeemPromise = page.waitForResponse((response) => {
    if (response.request().method() !== 'GET') return false;
    return new URL(response.url()).pathname.startsWith('/api/v1/downloads/');
  });
  const downloadPromise = page.waitForEvent('download');

  await page.getByRole('button', { name: 'Tải xuống' }).click();
  await page.getByRole('menuitem', { name: 'Markdown (.md)' }).click();

  const [issueResponse, redeemResponse, download] = await Promise.all([
    issuePromise,
    redeemPromise,
    downloadPromise,
  ]);

  expect(issueResponse.status()).toBe(200);
  expect(redeemResponse.status()).toBe(200);
  expect(download.suggestedFilename()).toMatch(/\.md$/);

  // Download has no success Notice; absence of an error alert plus the
  // observed issue/redeem 200s and browser download is completion.
  await expect(page.getByRole('alert')).toHaveCount(0);

  const issued = (await issueResponse.json()) as { capability?: string };
  expect(typeof issued.capability).toBe('string');
  expect(issued.capability!.length).toBeGreaterThan(0);

  const redeemPath = new URL(redeemResponse.url()).pathname;
  expect(redeemPath.startsWith('/api/v1/downloads/')).toBe(true);
  expect(redeemPath.includes(issued.capability!)).toBe(true);

  const capability = issued.capability!;
  const leakedToConsole = consoleLines.some((line) => line.includes(capability));
  expect(leakedToConsole, 'capability token must not appear in browser console logs').toBe(false);
});
