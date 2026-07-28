// Upload -> job-lifecycle flow (P2-15 flow 2 / P2-08). Previously deferred in
// the backlog (`plans/markhand-web/backlog/phase-2/issues/README.md`, P2-15)
// as "upload -> indexed: XHR không qua fetch-mock". This spec is that flow.
//
// WHY page.route() INSTEAD OF THE FETCH MOCK: `uploadTransport.ts` sends
// `POST /uploads` via `XMLHttpRequest`, not `fetch` — deliberately, for real
// upload-byte progress (see that file's own module doc) — and
// `src/mocks/browser.ts`'s `installBrowserMocks()` only ever patches
// `globalThis.fetch`, so the in-app mock never sees this request; that
// mismatch is exactly the reason this flow was deferred (see library.spec.ts's
// header for the same gap from the other side). Playwright's `page.route()`
// intercepts at the browser's real network layer, below both `fetch` and
// `XMLHttpRequest`, so it catches this request the same way it would catch a
// `fetch` one. It is also the only option at all for the 413 case in the
// second test below: `mockControl.forceStatus`'s `ForcedFailureKind`
// (401/403/404/409/429/503, see `mocks/control.ts`) doesn't even include 413,
// and `forceStatus` only ever applies to fetch-mock operations to begin with
// — an XHR request never reaches it regardless.
//
// ONE LAYER DEEPER, for the happy path below: a request the app makes via
// `fetch` (e.g. the job-status poll, `GET /jobs/{jobId}`) is *already*
// answered entirely inside the page's patched `globalThis.fetch` — it never
// becomes a real outgoing request, so `page.route()` never even sees it (this
// was confirmed empirically: routing `GET /jobs/{jobId}` here always went
// unhit). So instead of re-implementing `createUpload`'s bookkeeping by hand,
// the routed handler below replays the exact same multipart submission
// *through that same in-page mock* (a `page.evaluate`'d `fetch(...)` call),
// then relays its real response back as the XHR's answer. That registers a
// genuine job in the mock's own store, so the follow-up `GET /jobs/{jobId}`
// poll (already fetch-mocked, no routing needed) finds it and answers
// `pending` for real — no synthetic job-status responses to keep in sync by
// hand. The one gap that still leaves: nothing in the mock ever advances a
// job's `status` past `pending` on its own, so `succeedUploadJob` (backed by
// `components/upload/testSupport.ts`, wired onto `window` the same way
// `mockControl` already is) nudges the one job this test cares about to
// `succeeded` once the "converting" stage has been observed.
import { expect, test, type Route } from '@playwright/test';
import { IDS, login, openEmployeeHandbook, succeedUploadJob } from './support';

const FILE_NAME = 'onboarding-notes.txt';
const FILE_CONTENTS = 'hello world';

function apiErrorBody(code: string, message: string): Record<string, string> {
  return { code, message, requestId: '00000000-0000-4000-8000-0000000000fe' };
}

test('selecting a file uploads with progress, then the job lifecycle reaches converted', async ({
  page,
}) => {
  await login(page);
  await openEmployeeHandbook(page);

  let uploadedJobId: string | undefined;

  // The one POST /uploads this test's upload makes: replayed through the
  // page's own fetch mock (see module doc above), then relayed back as the
  // XHR's response — fulfilled a beat late so the "uploading" progress state
  // below is a real observable render, not something that flashes past
  // before Playwright gets to assert it.
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
    uploadedJobId = (JSON.parse(replayed.text) as { jobId?: string }).jobId;
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

  // 1. Client-side upload progress, driven by the real XHR against the
  //    routed response above — not a fabricated percentage.
  await expect(page.getByRole('progressbar', { name: `Đang tải lên ${FILE_NAME}` })).toBeVisible();

  // 2. The upload settled with a real jobId (a job the replayed request
  //    above genuinely registered in the mock store): the row switches to
  //    server-side job tracking, and the first `GET /jobs/{jobId}` poll —
  //    answered by the mock's own `getJob` handler, `pending` fresh out of
  //    `createUpload` — reports "converting".
  await expect(page.getByText('Đang chuyển đổi sang Markdown…')).toBeVisible();
  expect(uploadedJobId, 'createUpload response did not include a jobId').toBeTruthy();

  // 3. Nudge that same job to `succeeded` (nothing in the mock does this on
  //    its own — see module doc), then wait for the next poll to pick it up.
  await succeedUploadJob(page, uploadedJobId!);

  // Per jobLifecycle.ts's own documented scope limit, the client cannot
  // observe a separate indexing/indexed stage from just the convert job id,
  // so this "converted, indexing continues server-side" copy IS the honest
  // terminal UI state here, not a stand-in for a missing "indexed" one. The
  // generous timeout is the real `POLL_INTERVAL_MS` (4s) fallback firing —
  // not an arbitrary sleep, `expect` polls until it happens.
  await expect(
    page.getByText('Đã chuyển đổi sang Markdown; hệ thống đang hoàn thiện lập chỉ mục để hỏi đáp.'),
  ).toBeVisible({ timeout: 8_000 });
});

test('a 413 on upload surfaces the accessible "too large" message, not a crash', async ({
  page,
}) => {
  await login(page);
  await openEmployeeHandbook(page);

  // `createUpload` declares 413, not 415 (see api/generated/contract.ts) —
  // this is the one oversized-file response the real server contract
  // promises, so it's the failure case exercised here. No replay-through-mock
  // needed: a straight 413 doesn't touch the store either way.
  await page.route('**/api/v1/uploads', async (route: Route) => {
    if (route.request().method() !== 'POST') {
      await route.fallback();
      return;
    }
    await route.fulfill({
      status: 413,
      contentType: 'application/json',
      body: JSON.stringify(
        apiErrorBody('payload_too_large', 'Uploaded file exceeds the size limit.'),
      ),
    });
  });

  await page.getByLabel('Chọn tệp để tải lên').setInputFiles({
    name: 'huge-report.pdf',
    mimeType: 'application/pdf',
    buffer: Buffer.from('not actually huge, the mock just says so'),
  });

  // `role="alert"` per UploadItemRow.tsx: an accessible, live-announced
  // failure, not a silent/console-only one. `messages.ts`'s
  // `describeUploadFailure` appends the server's own message in parentheses
  // after the generic copy.
  const alert = page.getByRole('alert');
  await expect(alert).toContainText(
    'Tệp vượt quá dung lượng cho phép. Hãy nén hoặc chia nhỏ tệp rồi thử lại.',
  );
  await expect(alert).toContainText('Uploaded file exceeds the size limit.');

  // The row is still there, removable, and the panel accepted no phantom
  // upload — no crash, no stuck spinner.
  await expect(page.getByText('huge-report.pdf')).toBeVisible();
});
