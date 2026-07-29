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
//
// Chat UI (this same P2-10 issue, chat panel follow-up): `AskPanel` became
// `ChatPanel` — a scrolling in-session history of turns instead of a single
// question/answer. The four scenarios below are unchanged in substance (each
// still asks exactly one question and checks the exact same copy/citations),
// just re-pointed at the chat log's per-turn `data-testid="qa-answer"` where
// a selector needed to stay unambiguous now that more than one can exist.
// The new multi-turn scenario is the one addition.
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

  // P2-10 gap close: `CitationPin` (contract.ts) now carries
  // `logicalDocumentId`/`versionId`/`collectionId` (`mocks/handlers/qa.ts`'s
  // `passageToCitation`), so the citation for "Roadmap.xlsx" deep-links
  // straight to its Library preview instead of the old dead-link note (see
  // `CitationCard.tsx`'s module doc for the contract gap this closes).
  // This question's tokens ("quý") also match the unrelated compare-doc
  // fixture, so a second citation/link can render here too — scope to the
  // CITE-0001 card specifically rather than assuming there is only one link.
  await page
    .locator('li.card', { hasText: 'CITE-0001' })
    .getByRole('link', { name: 'Xem trước tài liệu' })
    .click();
  await expect(page).toHaveURL(/\/library\/[^/]+\?doc=[^/]+$/);
  // `#library-preview-heading`, not a bare role query: the mock preview
  // markdown re-embeds the title as its own `<h1># Roadmap.xlsx`, so the
  // title text renders twice on this page (this panel's own heading, and
  // that one) — same ambiguity `LibraryPage.test.tsx` documents.
  await expect(page.locator('#library-preview-heading')).toHaveText('Roadmap.xlsx');
  await expect(page.getByTestId('document-preview-markdown')).toContainText(
    'Mock preview content for version',
  );
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

test('chat keeps multi-turn history: a second question is answered without disturbing the first turn', async ({
  page,
}) => {
  await login(page);
  await page.getByRole('link', { name: 'Hỏi đáp' }).click();

  const chatLog = page.getByRole('log', { name: 'Lịch sử hỏi đáp' });
  const questionBox = page.getByRole('textbox', { name: 'Câu hỏi' });

  await questionBox.fill(ASK_QUESTION);
  await page.getByRole('button', { name: 'Hỏi' }).click();
  await expect(chatLog.getByTestId('qa-answer').first()).toContainText(
    'Lộ trình quý 3 tập trung vào tối ưu hiệu năng lập chỉ mục.',
  );
  // The composer re-enables once the first turn settles.
  await expect(questionBox).toBeEnabled();

  await questionBox.fill('cau-hoi-khong-khop-bat-ky-tai-lieu-nao');
  await page.getByRole('button', { name: 'Hỏi' }).click();

  const answers = chatLog.getByTestId('qa-answer');
  await expect(answers).toHaveCount(2);
  // Both turns' own question + answer are still visible — the first turn was
  // never disturbed by the second (each owns its own stream state).
  await expect(chatLog).toContainText(ASK_QUESTION);
  await expect(answers.nth(0)).toContainText(
    'Lộ trình quý 3 tập trung vào tối ưu hiệu năng lập chỉ mục.',
  );
  await expect(answers.nth(1)).toContainText(
    'Không tìm thấy nội dung liên quan trong tài liệu đã lập chỉ mục để trả lời câu hỏi này.',
  );
});
