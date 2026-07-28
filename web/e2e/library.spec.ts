// Library flows (P2-15 flows 3, 5, 6): per-document actions (preview /
// reindex / delete), permission-deny on a document mutation, and the
// actionable rate-limit / quota message.
//
// UNCOVERED HERE, and why (flows 2 & 6's upload half):
//   - The upload → indexed flow and the 429-on-upload quota flow CANNOT be
//     driven by this mock-based suite. `UploadPanel`/`uploadTransport.ts`
//     performs the multipart POST via `XMLHttpRequest` (deliberately — real
//     progress needs XHR, per the P2-08 brief), but the browser mock only
//     overrides `globalThis.fetch`; XHR is never intercepted. vite then
//     proxies `/api` to a backend that isn't running under E2E, so the upload
//     resolves to a transport/HTTP failure ("Tải tệp lên thất bại"), not the
//     mock's 201/429. Driving upload end-to-end belongs to the real-deployment
//     E2E half of P2-15, not this one. The quota *message shape* is still
//     exercised below via a document action that does go through `fetch`.
//   - Separately, the mock never advances a `pending` job, so even if an
//     upload landed, a fresh document would settle at `converting`, not
//     `indexed`; the `indexed` state is shown by the seeded document instead.
//
// FORCED-STATUS CONSTRAINT: `forceStatus` responses pass through the mock's
// drift guard (`assertStatusDeclared`), so a status can only be forced on an
// operation whose contract declares it. `deleteDocument`/`reindexDocument` do
// NOT declare 403 (see actionErrors.ts's own note), so the permission-deny
// below uses the download-capability action, which does declare 403.
import { expect, test } from '@playwright/test';
import { forceStatus, login, openEmployeeHandbook } from './support';

test('selecting a document previews sanitized markdown, reindex enqueues, delete removes the row', async ({
  page,
}) => {
  await login(page);
  await openEmployeeHandbook(page);

  const table = page.getByRole('table', { name: 'Danh sách tài liệu' });
  // The seeded `indexed` document is present in the list.
  await expect(table.getByRole('button', { name: /Onboarding Guide\.pdf/ })).toBeVisible();
  await expect(table.getByText('Đã lập chỉ mục')).toBeVisible();

  await table.getByRole('button', { name: /Onboarding Guide\.pdf/ }).click();

  // Preview renders through SafeMarkdown into the preview slot.
  const preview = page.getByTestId('document-preview-markdown');
  await expect(preview).toContainText('Mock preview content for version 1');
  // Sanitized markdown: the `#` heading became a real <h1>, not literal text.
  await expect(preview.getByRole('heading', { name: 'Onboarding Guide.pdf' })).toBeVisible();

  // Reindex enqueues and shows the success notice.
  await page.getByRole('button', { name: 'Lập chỉ mục lại' }).click();
  await expect(page.getByText('Đã đưa tài liệu vào hàng đợi lập chỉ mục.')).toBeVisible();

  // Delete asks for confirmation, then the row disappears after the refetch.
  await page.getByRole('button', { name: 'Xóa', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Xóa tài liệu này?' });
  await expect(dialog).toBeVisible();
  await dialog.getByRole('button', { name: 'Xóa tài liệu' }).click();

  await expect(table.getByRole('button', { name: /Onboarding Guide\.pdf/ })).toHaveCount(0);
});

test('a 403 on a document mutation surfaces the specific permission-denied message, not a crash', async ({
  page,
}) => {
  await login(page);
  await openEmployeeHandbook(page);

  await page
    .getByRole('table', { name: 'Danh sách tài liệu' })
    .getByRole('button', { name: /Onboarding Guide\.pdf/ })
    .click();

  // The download-capability action is the document mutation whose contract
  // declares 403 (deleteDocument/reindexDocument do not — see file header).
  await forceStatus(page, 'issueDownloadCapability', 403, 1);
  await page.getByRole('button', { name: 'Tải xuống' }).click();
  await page.getByRole('menuitem', { name: 'Markdown (.md)' }).click();

  await expect(
    page.getByText('Bạn không có quyền thực hiện thao tác này với tài liệu này.'),
  ).toBeVisible();
  // The document is untouched and still selectable.
  await expect(
    page
      .getByRole('table', { name: 'Danh sách tài liệu' })
      .getByRole('button', { name: /Onboarding Guide\.pdf/ }),
  ).toBeVisible();
});

test('a 429 on a document action surfaces the actionable rate-limit / quota message', async ({
  page,
}) => {
  await login(page);
  await openEmployeeHandbook(page);

  await page
    .getByRole('table', { name: 'Danh sách tài liệu' })
    .getByRole('button', { name: /Onboarding Guide\.pdf/ })
    .click();

  // reindexDocument declares 429; the mock's forced 429 carries a Retry-After,
  // which actionErrors.ts turns into an actionable "try again in N seconds"
  // message — the same quota/rate-limit copy path a 429-on-upload would use.
  await forceStatus(page, 'reindexDocument', 429, 1);
  await page.getByRole('button', { name: 'Lập chỉ mục lại' }).click();

  await expect(page.getByText(/Quá nhiều yêu cầu\. Vui lòng thử lại sau \d+ giây\./)).toBeVisible();
});
