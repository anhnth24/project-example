// Real-deployment upload -> indexed (P2-15, real-deployment half). This is
// the real-backend counterpart to the mock-mode `e2e/upload.spec.ts` and
// `e2e/document-status-polling.spec.ts`: same UI, same assertions on the
// document state machine, but against a genuine `fileconv-server` — see
// `support.ts`'s module doc for the stack this suite assumes is already up
// (`deploy/scripts/web-e2e-real.sh` only).
//
// NO INTERCEPTION, NO TEST SEAMS: the mock suite needs `page.route()` to
// catch `POST /uploads` (sent via `XMLHttpRequest`, which the in-page fetch
// mock never sees — see `e2e/upload.spec.ts`'s own module doc) and
// `succeedUploadJob`/`advanceDocument` (`e2e/support.ts`) to nudge a job/
// document past a stage nothing in the mock advances on its own. None of
// that exists here: there is no mock to miss the XHR, and there is nothing
// to nudge, because the real worker really does convert and index the file
// by itself. `POST /uploads` and every poll are genuine round-trips to the
// real server, exercising the real storage (the file genuinely lives on
// disk/object storage server-side) and the real conversion pipeline
// (`fileconv-core`, per the repo's own CLAUDE.md).
//
// HOW THE WAIT WORKS: `LibraryPage.tsx` already polls
// `GET /collections/{collectionId}/documents` on a 5s/15s/30s backoff
// whenever the current page holds a non-terminal document (see that file's
// own "Live status polling" section) — this spec drives no polling itself,
// it just waits on the DOM state that polling produces, the same way a real
// user would watch the page. `GET /documents/{documentId}` is the request
// backing the open document's own preview badge (same file, `directDocument`/
// `previewResult`), so opening the preview below proves that request path
// too, not just the list one.
//
// TIMEOUT BUDGET: real conversion + real indexing (an actual embedding call)
// is not free the way the mock's synthetic instant transitions are, so the
// terminal-state assertions below carry generous explicit timeouts (up to
// the 60s the task allows for a real backend), not arbitrary padding. A
// plain `.txt` file is used deliberately so this run touches no OCR/pdfium/
// whisper path (see CLAUDE.md's architecture notes) — the slowest thing it
// still genuinely waits on is the indexing/embedding step.
import { expect, test } from '@playwright/test';
import { login, openPocLibrary } from './support';

const FILE_NAME = `e2e-real-upload-${Date.now()}.txt`;
const FILE_CONTENTS = 'Real-deployment upload smoke test contents.';

test('uploading a file against the real backend reaches indexed, and its preview renders', async ({
  page,
}) => {
  // Real conversion + indexing genuinely takes longer than the mock suite's
  // synthetic timings; `test.slow()` triples Playwright's default per-test
  // timeout so the generous waits below have room to actually complete.
  test.slow();

  await login(page);
  await openPocLibrary(page);

  await page.getByLabel('Chọn tệp để tải lên').setInputFiles({
    name: FILE_NAME,
    mimeType: 'text/plain',
    buffer: Buffer.from(FILE_CONTENTS),
  });

  const table = page.getByRole('table', { name: 'Danh sách tài liệu' });
  const row = table.getByRole('row').filter({ hasText: FILE_NAME });

  // 1. The row appears once the real `POST /uploads` response lands and the
  //    document list refetches. No progress-bar assertion here: on a real
  //    localhost round-trip for a tiny file, the upload can settle fast
  //    enough that an XHR-progress render is not reliably observable the way
  //    it is in the mock suite (which delays its routed response on
  //    purpose) — the state badge below is the stable signal instead.
  await expect(row).toBeVisible({ timeout: 15_000 });

  // 2. No intermediate-state assertion on purpose: with workers running, this
  //    tiny .txt often converts in well under the list's 5s poll tick, so the
  //    transient "Đang chuyển đổi" badge is not reliably observable. The
  //    state machine's intermediate steps stay covered by the mock suite
  //    (`e2e/document-status-polling.spec.ts`), which can freeze each stage;
  //    the real-deployment signal here is the terminal state below.

  // Open the document's own preview so the badge there is proven too, not
  // just the list row's — same shape as `document-status-polling.spec.ts`'s
  // mock counterpart.
  await row.getByRole('button', { name: new RegExp(FILE_NAME) }).click();
  const previewBadge = page.locator('aside[aria-labelledby="library-preview-heading"]');

  // 3. Terminal state: converted -> indexing -> indexed, driven entirely by
  //    the real backend. Both the row badge and the open preview's own badge
  //    must agree.
  await expect(row.getByText('Đã lập chỉ mục')).toBeVisible({ timeout: 60_000 });
  await expect(previewBadge.getByText('Đã lập chỉ mục')).toBeVisible({ timeout: 60_000 });

  // 4. Preview actually renders the real converted content, not just the
  //    badge — proving the round trip through the real converter
  //    (`fileconv-core`) end to end, the same way `DocumentPreview.tsx`
  //    renders it for a real user.
  const previewMarkdown = page.getByTestId('document-preview-markdown');
  await expect(previewMarkdown).toBeVisible({ timeout: 15_000 });
  await expect(previewMarkdown).toContainText(FILE_CONTENTS);
});
