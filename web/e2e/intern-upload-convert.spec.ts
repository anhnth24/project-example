// intern-21 / #359 — upload → convert → preview (mock-mode happy path).
//
// WHY page.route() FOR POST /uploads: same as `upload.spec.ts` — upload uses
// `XMLHttpRequest`, not `fetch`, so the in-page mock never sees it; Playwright
// routes at the network layer and replays through the page's fetch mock to
// register a real job/document in the store.
//
// Preview content is mock markdown (`Mock preview content for version N`), not
// the uploaded file bytes — we assert user-visible converted output only.
import { expect, test, type Route } from '@playwright/test';
import { IDS, advanceDocument, login, openEmployeeHandbook, succeedUploadJob } from './support';

const FILE_NAME = 'notes.txt';
const FILE_CONTENTS = 'hello from intern e2e';

test('upload converts and preview shows markdown output', async ({ page }) => {
  await login(page);
  await openEmployeeHandbook(page);

  let uploadedJobId: string | undefined;
  let uploadedDocumentId: string | undefined;

  await page.route('**/api/v1/uploads', async (route: Route) => {
    const request = route.request();
    if (request.method() !== 'POST') {
      await route.fallback();
      return;
    }
    const authorization = request.headers()['authorization'] ?? '';
    await new Promise((resolve) => setTimeout(resolve, 200));
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
    const body = JSON.parse(replayed.text) as { jobId?: string; documentId?: string };
    uploadedJobId = body.jobId;
    uploadedDocumentId = body.documentId;
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

  await expect(page.getByRole('progressbar', { name: `Đang tải lên ${FILE_NAME}` })).toBeVisible();
  await expect(page.getByText('Đang chuyển đổi sang Markdown…')).toBeVisible();
  expect(uploadedJobId, 'createUpload response did not include a jobId').toBeTruthy();
  expect(uploadedDocumentId, 'createUpload response did not include a documentId').toBeTruthy();

  await succeedUploadJob(page, uploadedJobId!);
  await expect(
    page.getByText('Đã chuyển đổi sang Markdown; hệ thống đang hoàn thiện lập chỉ mục để hỏi đáp.'),
  ).toBeVisible({ timeout: 8_000 });

  const table = page.getByRole('table', { name: 'Danh sách tài liệu' });
  const row = table.getByRole('row').filter({ hasText: FILE_NAME });
  await expect(row).toBeVisible();

  await row.getByRole('button', { name: new RegExp(FILE_NAME) }).click();

  await advanceDocument(page, uploadedDocumentId!);
  await expect(row.getByText('Đã chuyển đổi')).toBeVisible({ timeout: 10_000 });

  const preview = page.getByTestId('document-preview-markdown');
  await expect(preview).toContainText('Mock preview content for version');
});
