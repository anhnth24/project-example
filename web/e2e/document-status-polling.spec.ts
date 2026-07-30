// P2-08 gap close (owner critique, 2026-07-29: "trạng thái document chưa
// đúng giai đoạn xử lý khi load lại trang hoặc mở chức năng khác rồi quay
// lại"): `LibraryPage` now polls `GET /documents` every 5s while the current
// page holds a non-terminal document, so the badge (and the preview panel,
// when that same document is open) advances on its own — no reload, no
// re-navigation.
//
// UPLOAD ITSELF still needs `page.route()`: `uploadTransport.ts` sends
// `POST /uploads` via `XMLHttpRequest`, which the in-page fetch mock
// (`src/mocks/browser.ts`) never sees — same mechanics as `upload.spec.ts`,
// see that file's own module doc for why the routed handler replays the
// submission through the page's own mock rather than reimplementing
// `createUpload`'s bookkeeping by hand. Once the document exists, though,
// `GET /documents` (the poll) goes through the in-page mock like any other
// `fetch` call, so `advanceDocument` (backed by
// `components/library/testSupport.ts`) is all that's needed to move it
// forward — nothing here calls `page.reload()`/`page.goto()` at any point,
// which is exactly what proves the badge updates via polling, not a fresh
// mount re-reading the URL.
import { expect, test, type Route } from '@playwright/test';
import { IDS, advanceDocument, login, openEmployeeHandbook } from './support';

const FILE_NAME = 'quarterly-notes.txt';
const FILE_CONTENTS = 'quarterly notes';

test('a document badge advances converting -> converted -> indexing -> indexed via polling, without a reload', async ({
  page,
}) => {
  await login(page);
  await openEmployeeHandbook(page);

  let uploadedDocumentId: string | undefined;

  await page.route('**/api/v1/uploads', async (route: Route) => {
    const request = route.request();
    if (request.method() !== 'POST') {
      await route.fallback();
      return;
    }
    const authorization = request.headers()['authorization'] ?? '';
    const replayed = await page.evaluate(
      async ({ authorization: auth, collectionId, fileName, base64Contents }) => {
        const bytes = Uint8Array.from(atob(base64Contents), (c) => c.charCodeAt(0));
        const form = new FormData();
        form.append('collectionId', collectionId);
        form.append('file', new File([bytes], fileName, { type: 'text/plain' }), fileName);
        const response = await fetch('/api/v1/uploads', {
          method: 'POST',
          headers: { Authorization: auth, Accept: 'application/json' },
          body: form,
        });
        return { status: response.status, text: await response.text() };
      },
      {
        authorization,
        collectionId: IDS.employeeHandbookCollection,
        fileName: FILE_NAME,
        base64Contents: Buffer.from(FILE_CONTENTS).toString('base64'),
      },
    );
    uploadedDocumentId = (JSON.parse(replayed.text) as { documentId?: string }).documentId;
    await route.fulfill({
      status: replayed.status,
      contentType: 'application/json',
      body: replayed.text,
    });
  });

  await page.getByLabel('Chọn tệp để tải lên').setInputFiles({
    name: FILE_NAME,
    mimeType: 'text/plain',
    buffer: Buffer.from(FILE_CONTENTS),
  });

  const table = page.getByRole('table', { name: 'Danh sách tài liệu' });
  const row = table.getByRole('row').filter({ hasText: FILE_NAME });
  await expect(row.getByText('Đang chuyển đổi')).toBeVisible();
  expect(uploadedDocumentId, 'createUpload response did not include a documentId').toBeTruthy();

  // Open the document's own preview so the badge there is proven too, not
  // just the list row's. `aria-labelledby="library-preview-heading"` scopes
  // to the preview panel itself (`DocumentPreview.tsx`), distinct from the
  // list row's own badge.
  await row.getByRole('button', { name: new RegExp(FILE_NAME) }).click();
  const previewBadge = page.locator('aside[aria-labelledby="library-preview-heading"]');

  // Poll interval is 5s (`POLL_BACKOFF_DELAYS_MS[0]` in LibraryPage.tsx); a
  // generous timeout accommodates that without an arbitrary sleep — `expect`
  // polls until the assertion holds.
  await advanceDocument(page, uploadedDocumentId!); // converting -> converted
  await expect(row.getByText('Đã chuyển đổi')).toBeVisible({ timeout: 10_000 });
  await expect(previewBadge.getByText('Đã chuyển đổi')).toBeVisible({ timeout: 10_000 });

  await advanceDocument(page, uploadedDocumentId!); // converted -> indexing
  await expect(row.getByText('Đang lập chỉ mục')).toBeVisible({ timeout: 10_000 });

  await advanceDocument(page, uploadedDocumentId!); // indexing -> indexed
  await expect(row.getByText('Đã lập chỉ mục')).toBeVisible({ timeout: 10_000 });
  await expect(previewBadge.getByText('Đã lập chỉ mục')).toBeVisible({ timeout: 10_000 });

  // Terminal now — no assertion for "polling stopped" here (that's covered
  // at the unit level, `LibraryPage.test.tsx`, with fake timers + a spy on
  // the underlying request); this spec only proves the visible behavior
  // end to end.
});
