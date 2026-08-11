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
  // whichever footnote item links to "Xem trước" first (the item already
  // carries the document title next to the link).
  await page.getByRole('link', { name: 'Xem trước' }).first().click();
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

  await expect(page.getByText('Dùng đoạn nguồn (câu mô hình không đạt kiểm chứng)')).toBeVisible();
  // The readable Vietnamese summary is visible up front; the raw provider
  // warning is preserved verbatim under the collapsible technical details.
  await expect(
    page.getByText(/Nhà cung cấp mô hình tạm thời không khả dụng; đang hiện các đoạn nguồn/),
  ).toBeVisible();
  await page.getByTestId('qa-warning-details').locator('summary').click();
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

test.describe('as_of timezone portability', () => {
  test.use({ timezoneId: 'America/Los_Angeles' });

  test('as_of between budget versions cites v1 10-million content with a non-current warning', async ({
    page,
  }) => {
    // Scope-wide as-of (owner option 1): timestamp only — no document picker.
    // Budget fixture v1 effectiveFrom = mockTimestamp(50), v2 = mockTimestamp(95);
    // fixed 01:10Z is between them. This scenario uses a non-UTC browser zone
    // and derives the datetime-local wall time for that instant, so it cannot
    // accidentally shift to v2 on a runner configured outside UTC.
    const expectedAsOf = '2026-01-01T01:10:00.000Z';
    await login(page);
    await page.getByRole('link', { name: 'Hỏi đáp' }).click();

    await page.getByRole('combobox', { name: 'Chế độ truy vấn' }).click();
    await page.getByRole('option', { name: 'Tại một thời điểm (as-of)' }).click();

    const askButton = page.getByRole('button', { name: 'Hỏi', exact: true });
    const asOfInput = page.getByLabel('Thời điểm (as-of)');
    await expect(asOfInput).toHaveAttribute('required', '');

    await page.getByRole('textbox', { name: 'Câu hỏi' }).fill('Ngân sách vận hành là bao nhiêu?');
    await expect(askButton).toBeDisabled();

    const localInputForFixedInstant = await page.evaluate((iso) => {
      const instant = new Date(iso);
      const pad = (value: number) => String(value).padStart(2, '0');
      return (
        `${instant.getFullYear()}-${pad(instant.getMonth() + 1)}-${pad(instant.getDate())}` +
        `T${pad(instant.getHours())}:${pad(instant.getMinutes())}`
      );
    }, expectedAsOf);
    await asOfInput.fill(localInputForFixedInstant);
    await expect(askButton).toBeEnabled();
    expect(
      await asOfInput.evaluate((input: HTMLInputElement) => new Date(input.value).toISOString()),
    ).toBe(expectedAsOf);
    await askButton.click();

    const turn = page.getByRole('log', { name: 'Lịch sử hỏi đáp' }).locator('.chat-turn').first();
    await expect(turn.getByTestId('qa-answer')).toContainText('10 triệu');
    await expect(turn.getByTestId('qa-answer')).not.toContainText('15 triệu');
    await expect(
      turn.getByText(
        /Phiên bản 1 của "Chính sách ngân sách vận hành\.pdf" không phải phiên bản hiện hành/,
      ),
    ).toBeVisible();
    await expect(turn.getByText(/đã được giải quyết ở phiên bản 2/)).toBeVisible();
    await expect(turn.getByText(/15 triệu/)).not.toBeVisible();
  });
});

test('multi-mode conflict warnings display correctly (compare/history)', async ({ page }) => {
  // P2-10 conflict-warning demo for compare/history. The seeded multi-version
  // document (`QA_COMPARE_DOCUMENT_ID`, "Chính sách ngân sách vận hành.pdf" —
  // see `mocks/fixtures.ts`) doubles as a live "BA's claim (v1, 10 triệu/quý)
  // vs. thiết kế mới (v2, 15 triệu/quý, current)" conflict. Scope-wide `as_of`
  // (timestamp only, no document picker) is covered by the dedicated as_of
  // scenario above; this spec drives the two modes that still use the
  // compare/history document picker end-to-end.
  await login(page);
  await page.getByRole('link', { name: 'Hỏi đáp' }).click();

  // The compare/history document picker only offers documents a search
  // already turned up (`ChatPanel.tsx`'s module doc) — surface the compare
  // document as a hit first, same prerequisite the "multi-project picker"
  // scenario below already relies on for its own search step.
  await page.getByRole('tab', { name: 'Tìm kiếm', exact: true }).click();
  await page.getByLabel('Từ khóa').fill('ngân sách');
  await page.getByRole('button', { name: 'Tìm kiếm', exact: true }).click();
  await expect(page.getByText('Chính sách ngân sách vận hành.pdf')).toBeVisible();

  await page.getByRole('tab', { name: 'Hỏi đáp', exact: true }).click();
  const chatLog = page.getByRole('log', { name: 'Lịch sử hỏi đáp' });

  // --- compare mode: v1 (BA, 10 triệu) vs. v2 (thiết kế mới, current) ------
  await page.getByRole('combobox', { name: 'Chế độ truy vấn' }).click();
  await page.getByRole('option', { name: 'So sánh 2 phiên bản' }).click();
  await page.getByRole('combobox', { name: 'Tài liệu để so sánh hoặc xem lịch sử' }).click();
  await page.getByRole('option', { name: 'Chính sách ngân sách vận hành.pdf' }).click();
  await page.getByRole('combobox', { name: 'Phiên bản A' }).click();
  await page.getByRole('option', { name: 'Phiên bản 1' }).click();
  await page.getByRole('combobox', { name: 'Phiên bản B' }).click();
  await page.getByRole('option', { name: 'Phiên bản 2 (hiện hành)' }).click();

  await page.getByRole('textbox', { name: 'Câu hỏi' }).fill('Ngân sách vận hành thay đổi thế nào?');
  await page.getByRole('button', { name: 'Hỏi', exact: true }).click();

  const compareTurn = chatLog.locator('.chat-turn').nth(0);
  await expect(
    compareTurn.getByText(
      /Phiên bản 1 của "Chính sách ngân sách vận hành\.pdf" không phải phiên bản hiện hành/,
    ),
  ).toBeVisible();
  await expect(compareTurn.getByText(/đã được giải quyết ở phiên bản 2/)).toBeVisible();
  // v2 IS the current version — only v1's side of the comparison gets its
  // own warning here, proving the mock warns per-version rather than always
  // emitting a fixed pair.
  await expect(
    compareTurn.getByText(
      /Phiên bản 2 của "Chính sách ngân sách vận hành\.pdf" không phải phiên bản hiện hành/,
    ),
  ).not.toBeVisible();

  // The composer re-enables once the compare turn settles, same as every
  // other multi-turn scenario in this file.
  await expect(page.getByRole('textbox', { name: 'Câu hỏi' })).toBeEnabled();

  // --- history mode: same document, one "resolved in v2" summary warning --
  await page.getByRole('combobox', { name: 'Chế độ truy vấn' }).click();
  await page.getByRole('option', { name: 'Lịch sử phiên bản' }).click();

  await page.getByRole('textbox', { name: 'Câu hỏi' }).fill('Lịch sử ngân sách vận hành thế nào?');
  await page.getByRole('button', { name: 'Hỏi', exact: true }).click();

  const historyTurn = chatLog.locator('.chat-turn').nth(1);
  await expect(historyTurn.getByText(/có xung đột dữ liệu giữa các phiên bản/)).toBeVisible();
  await expect(historyTurn.getByText(/đã được giải quyết ở phiên bản 2/)).toBeVisible();
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
