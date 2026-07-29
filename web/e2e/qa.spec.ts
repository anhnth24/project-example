// P2-10 Q&A flow, re-pointed at the owner's chat-first redesign
// ("hiện tại phần hỏi đáp đang nhìn lộn xộn quá", 2026-07-29):
//   - "Tìm kiếm" (`SearchPanel`) is now the second tab, not a stacked block —
//     every search scenario below switches tabs first.
//   - Citations render as numbered footnotes (`[n]` inline + a "Nguồn trích
//     dẫn" block) instead of a flat `CitationCard` list — the raw
//     `CITE-0001` tag is no longer shown inline.
//   - Chat history (part A, `chat-history.spec.ts`) is its own file; this one
//     keeps the original streaming/citation/revoke/fallback/multi-turn
//     scenarios, re-pointed at the new markup.
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
  await page.getByRole('tab', { name: 'Tìm kiếm', exact: true }).click();

  await page.getByLabel('Từ khóa').fill('hội nhập');
  await page.getByRole('button', { name: 'Tìm kiếm', exact: true }).click();

  await expect(page.getByText('Onboarding Guide.pdf')).toBeVisible();
  await page.getByRole('button', { name: 'Xem trước', exact: true }).click();
  await expect(page.getByTestId('qa-preview-markdown')).toContainText(
    'Mock preview content for version',
  );
});

test('ask streams a grounded answer token-by-token, then a numbered footnote source', async ({
  page,
}) => {
  await login(page);
  await page.getByRole('link', { name: 'Hỏi đáp' }).click();

  await page.getByRole('textbox', { name: 'Câu hỏi' }).fill(ASK_QUESTION);
  await page.getByRole('button', { name: 'Hỏi', exact: true }).click();

  await expect(page.getByTestId('qa-answer').first()).toContainText(
    'Lộ trình quý 3 tập trung vào tối ưu hiệu năng lập chỉ mục.',
  );
  // The raw `[CITE-0001]` tag is gone — replaced by a `[1]` footnote marker
  // inline plus a numbered "Nguồn trích dẫn" block at the end of the turn.
  await expect(page.getByText('CITE-0001', { exact: true })).not.toBeVisible();
  await expect(page.getByText('Nguồn trích dẫn').first()).toBeVisible();
  // P2-19 gap close: the footnote now shows the real document title
  // (`documentTitle`, mirrors `passageCatalog()`'s seeded title in
  // `mocks/handlers/qa.ts`) instead of falling back to the collection name.
  await expect(page.getByText('Roadmap.xlsx').first()).toBeVisible();

  // P2-10 gap close (still true post-redesign): `CitationPin` carries
  // `logicalDocumentId`/`versionId`/`collectionId`, so the footnote item for
  // "Roadmap.xlsx" deep-links straight to its Library preview.
  // This question's tokens ("quý") also match the unrelated compare-doc
  // fixture, so a second footnote/link can render here too — scope to
  // whichever footnote card links to "Xem trước tài liệu" first.
  await page.getByRole('link', { name: 'Xem trước tài liệu' }).first().click();
  await expect(page).toHaveURL(/\/library\/[^/]+\?doc=[^/]+$/);
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
  await page.getByRole('button', { name: 'Hỏi', exact: true }).click();

  await expect(page.getByText(/Trích dẫn đã bị thu hồi giữa chừng/)).toBeVisible();
  await expect(page.getByTestId('qa-answer').first()).not.toHaveText('');
  // The revoke closes the stream before `ask.citations` ever arrives.
  await expect(page.getByText('Nguồn trích dẫn')).not.toBeVisible();
});

test('provider fallback still answers (extractive), labelled honestly, with a warning', async ({
  page,
}) => {
  await login(page);
  await page.getByRole('link', { name: 'Hỏi đáp' }).click();

  await page.getByRole('textbox', { name: 'Câu hỏi' }).fill(`${ASK_QUESTION} ${FALLBACK_MARKER}`);
  await page.getByRole('button', { name: 'Hỏi', exact: true }).click();

  await expect(page.getByText('Trả lời trích xuất (không qua LLM)')).toBeVisible();
  await expect(page.getByText(/Nhà cung cấp LLM tạm thời không khả dụng/)).toBeVisible();
  await expect(page.getByTestId('qa-answer').first()).toContainText(
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
  await page.getByRole('button', { name: 'Hỏi', exact: true }).click();
  await expect(chatLog.getByTestId('qa-answer').first()).toContainText(
    'Lộ trình quý 3 tập trung vào tối ưu hiệu năng lập chỉ mục.',
  );
  // The composer re-enables once the first turn settles.
  await expect(questionBox).toBeEnabled();

  await questionBox.fill('cau-hoi-khong-khop-bat-ky-tai-lieu-nao');
  await page.getByRole('button', { name: 'Hỏi', exact: true }).click();

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

test('multi-project picker narrows both search and ask to the selected project', async ({
  page,
}) => {
  await login(page);
  await page.getByRole('link', { name: 'Hỏi đáp' }).click();

  const scopeTrigger = page.getByRole('combobox', { name: 'Phạm vi dự án' });
  await expect(scopeTrigger).toHaveText('Tất cả dự án');
  await scopeTrigger.click();
  // "Nhân sự" (seeded org A project) is assigned only to the Employee
  // Handbook collection — "Roadmap.xlsx" lives in Product Specs, which is
  // NOT in that project, so a search/ask scoped to "Nhân sự" finds nothing
  // for a query that only matches Roadmap.xlsx.
  await page.getByRole('checkbox', { name: 'Nhân sự', exact: true }).click();
  await expect(scopeTrigger).toHaveText('1 dự án: Nhân sự');

  await page.getByRole('tab', { name: 'Tìm kiếm', exact: true }).click();
  await page.getByLabel('Từ khóa').fill('lộ trình quý 3');
  await page.getByRole('button', { name: 'Tìm kiếm', exact: true }).click();
  await expect(page.getByText(/Không tìm thấy kết quả phù hợp với "lộ trình quý 3"/)).toBeVisible();

  await page.getByRole('tab', { name: 'Hỏi đáp', exact: true }).click();
  await page.getByRole('textbox', { name: 'Câu hỏi' }).fill(ASK_QUESTION);
  await page.getByRole('button', { name: 'Hỏi', exact: true }).click();
  await expect(page.getByTestId('qa-answer').first()).toContainText(
    'Không tìm thấy nội dung liên quan trong tài liệu đã lập chỉ mục để trả lời câu hỏi này.',
  );
});
