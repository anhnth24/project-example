// P2-10 Q&A flow (P2-15 flow addition). Previously absent from this suite —
// `web/e2e/support.ts`'s module doc used to note "ask -> citation is
// deliberately ABSENT... QaPage.tsx is a placeholder" (owner gate,
// R02/R03/R05 not yet live-evidenced). The owner lowered that gate
// 2026-07-29 (`plans/markhand-web/backlog/phase-2/issues/README.md`, P2-10):
// build on the OpenAPI contract + mock server like every other P2-0x flow,
// don't wait for full backend live-evidence. This is that flow.
//
// `POST /ask/stream` reaches the SPA's real fetch-based SSE transport
// (`api/sse.ts`'s `SseConnection`, never native `EventSource`) the same way
// any other fetch-mocked operation does here — unlike `upload.spec.ts`'s
// XHR gap, there is no `page.route()` layer needed: `mocks/handlers/qa.ts`
// registers `askStream` against the same in-page `fetch` patch every other
// mocked operation uses, so Playwright's page-context `fetch` override
// already covers it.
import { expect, test } from '@playwright/test';
import { login } from './support';

const ASK_QUESTION = 'Lộ trình quý 3 tập trung vào việc gì?';
// Mirrors `mocks/handlers/qa.ts`'s `QA_STREAM_MARKERS` — kept as a literal
// here (rather than importing the mock module into Playwright's Node
// process) since e2e drives the built app through the browser, not the
// mock's TS source directly; a drift between the two strings would just
// make these scenarios stop matching, which is exactly what the assertions
// below would catch.
const REVOKE_MARKER = '[[qa-e2e:citation-revoked]]';
const FALLBACK_MARKER = '[[qa-e2e:provider-fallback]]';

test('search finds an indexed document and opens its sanitized preview', async ({ page }) => {
  await login(page);
  await page.getByRole('link', { name: 'Hỏi đáp' }).click();

  await page.getByLabel('Từ khóa').fill('hội nhập');
  await page.getByRole('button', { name: 'Tìm kiếm' }).click();

  await expect(page.getByText('Onboarding Guide.pdf')).toBeVisible();
  await page.getByRole('button', { name: 'Xem trước' }).click();
  await expect(page.getByTestId('qa-preview-markdown')).toContainText(
    'Mock preview content for version',
  );
});

test('ask streams a grounded answer token-by-token, then citations', async ({ page }) => {
  await login(page);
  await page.getByRole('link', { name: 'Hỏi đáp' }).click();

  await page.getByRole('textbox', { name: 'Câu hỏi' }).fill(ASK_QUESTION);
  await page.getByRole('button', { name: 'Hỏi' }).click();

  await expect(page.getByTestId('qa-answer')).toContainText(
    'Lộ trình quý 3 tập trung vào tối ưu hiệu năng lập chỉ mục.',
  );
  await expect(page.getByText('CITE-0001', { exact: true })).toBeVisible();
  // No preview deep-link here on purpose: `CitationPin` (contract.ts) carries
  // no document/version id, so `AskPanel` cannot resolve one — it says so
  // instead of a silently-dead "Xem trước" button (see `CitationCard.tsx`'s
  // module doc for the verified contract gap this documents).
  await expect(
    page.getByText(/Trích dẫn ở đây chưa kèm định danh tài liệu\/phiên bản/),
  ).toBeVisible();
});

test('citation_revoked mid-answer surfaces an accessible revoked notice without losing the partial answer', async ({
  page,
}) => {
  await login(page);
  await page.getByRole('link', { name: 'Hỏi đáp' }).click();

  await page.getByRole('textbox', { name: 'Câu hỏi' }).fill(`${ASK_QUESTION} ${REVOKE_MARKER}`);
  await page.getByRole('button', { name: 'Hỏi' }).click();

  await expect(page.getByText(/Trích dẫn đã bị thu hồi giữa chừng/)).toBeVisible();
  await expect(page.getByTestId('qa-answer')).not.toHaveText('');
  // The revoke closes the stream before `ask.citations` ever arrives.
  await expect(page.getByText('CITE-0001', { exact: true })).not.toBeVisible();
});

test('provider fallback still answers (extractive), labelled honestly, with a warning', async ({
  page,
}) => {
  await login(page);
  await page.getByRole('link', { name: 'Hỏi đáp' }).click();

  await page.getByRole('textbox', { name: 'Câu hỏi' }).fill(`${ASK_QUESTION} ${FALLBACK_MARKER}`);
  await page.getByRole('button', { name: 'Hỏi' }).click();

  await expect(page.getByText('Trả lời trích xuất (không qua LLM)')).toBeVisible();
  await expect(page.getByText(/Nhà cung cấp LLM tạm thời không khả dụng/)).toBeVisible();
  await expect(page.getByTestId('qa-answer')).toContainText(
    'Lộ trình quý 3 tập trung vào tối ưu hiệu năng lập chỉ mục.',
  );
});
