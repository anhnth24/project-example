// Part A of the owner's Q&A redesign spec: private per-user chat history
// (P2-19 API + this round's web UI). Server-backed now (`chat-sessions`
// mock, `mocks/handlers/chatSessions.ts`) — a completed/revoked turn is
// persisted, survives switching to a different conversation, and replays
// (with clickable citations) when reopened from the sidebar. This is the one
// thing genuinely new versus the old in-memory-only chat log
// (`qa.spec.ts`'s own scenarios), so it gets its own file rather than being
// folded into that one.
import { expect, test } from '@playwright/test';
import { login } from './support';

const FIRST_QUESTION = 'Lộ trình quý 3 này tập trung nhiều nhất vào việc gì vậy nhỉ?';
const SECOND_QUESTION = 'cau-hoi-khong-khop-bat-ky-tai-lieu-nao';

test('two questions, "Cuộc trò chuyện mới", one more question, then reopening the first session from the sidebar replays its transcript with a clickable citation', async ({
  page,
}) => {
  await login(page);
  await page.getByRole('link', { name: 'Hỏi đáp' }).click();

  const sidebar = page.getByRole('navigation', { name: 'Lịch sử hỏi đáp' });
  const questionBox = page.getByRole('textbox', { name: 'Câu hỏi' });
  const chatLog = page.getByRole('log', { name: 'Lịch sử hỏi đáp' });

  // Question 1 — creates a brand-new session (no session was open yet).
  await questionBox.fill(FIRST_QUESTION);
  await page.getByRole('button', { name: 'Hỏi', exact: true }).click();
  await expect(chatLog.getByTestId('qa-answer').first()).toContainText(
    'Lộ trình quý 3 tập trung vào tối ưu hiệu năng lập chỉ mục.',
  );
  await expect(sidebar.getByRole('button', { name: FIRST_QUESTION, exact: true })).toBeVisible();

  // Question 2 — appended to the SAME session (still open), not a new one.
  await expect(questionBox).toBeEnabled();
  await questionBox.fill(SECOND_QUESTION);
  await page.getByRole('button', { name: 'Hỏi', exact: true }).click();
  await expect(chatLog.getByTestId('qa-answer').nth(1)).toContainText(
    'Không tìm thấy nội dung liên quan trong tài liệu đã lập chỉ mục để trả lời câu hỏi này.',
  );

  // "Cuộc trò chuyện mới" — starts a fresh, unsaved conversation.
  await sidebar.getByRole('button', { name: 'Cuộc trò chuyện mới', exact: true }).click();
  await expect(
    page.getByText('Chưa có câu hỏi nào trong cuộc trò chuyện này — đặt câu hỏi bên dưới.'),
  ).toBeVisible();

  // Ask one more question in this new conversation — a second, separate
  // session, never touching the first one's two turns above. Deliberately
  // reworded from the seeded fixture's own "Nhân viên mới cần hoàn thành gì
  // trong 30 ngày đầu?" title (`mocks/fixtures.ts`'s
  // `DEMO_CHAT_SESSION_ONBOARDING_ID`) so the two titles never collide.
  const THIRD_QUESTION = 'Nhân viên mới thì cần hoàn thành xong việc gì trong 30 ngày đầu vậy?';
  await questionBox.fill(THIRD_QUESTION);
  await page.getByRole('button', { name: 'Hỏi', exact: true }).click();
  await expect(chatLog.getByTestId('qa-answer').first()).toContainText(
    'Nhân viên mới cần hoàn thành khóa đào tạo hội nhập trong 30 ngày đầu tiên.',
  );
  await expect(sidebar.getByRole('button', { name: THIRD_QUESTION, exact: true })).toBeVisible();

  // Reopen the FIRST session from the sidebar — its whole transcript (both
  // turns) must still be there, citations intact and clickable.
  await sidebar.getByRole('button', { name: FIRST_QUESTION, exact: true }).click();
  await expect(page.getByText('Phiên đã lưu — tài liệu có thể đã thay đổi.')).toBeVisible();
  const reopenedAnswers = chatLog.getByTestId('qa-answer');
  await expect(reopenedAnswers).toHaveCount(2);
  await expect(reopenedAnswers.nth(0)).toContainText(
    'Lộ trình quý 3 tập trung vào tối ưu hiệu năng lập chỉ mục.',
  );
  await expect(reopenedAnswers.nth(1)).toContainText(
    'Không tìm thấy nội dung liên quan trong tài liệu đã lập chỉ mục để trả lời câu hỏi này.',
  );

  await page.getByRole('link', { name: 'Xem trước tài liệu' }).first().click();
  await expect(page).toHaveURL(/\/library\/[^/]+\?doc=[^/]+$/);
  await expect(page.getByTestId('document-preview-markdown')).toContainText(
    'Mock preview content for version',
  );
});

test('renaming and deleting a session from the sidebar', async ({ page }) => {
  await login(page);
  await page.getByRole('link', { name: 'Hỏi đáp' }).click();

  const sidebar = page.getByRole('navigation', { name: 'Lịch sử hỏi đáp' });
  // Seeded demo history (`mocks/fixtures.ts`'s `seedChatSessions`) — no need
  // to ask anything first.
  const roadmapTitle = 'Lộ trình quý 3 tập trung vào việc gì?';
  const onboardingTitle = 'Nhân viên mới cần hoàn thành gì trong 30 ngày đầu?';
  await expect(sidebar.getByRole('button', { name: roadmapTitle, exact: true })).toBeVisible();

  await sidebar.getByRole('button', { name: `Đổi tên phiên ${roadmapTitle}`, exact: true }).click();
  await sidebar.getByLabel(`Tên mới cho phiên ${roadmapTitle}`).fill('Phiên đổi tên qua E2E');
  await sidebar.getByRole('button', { name: 'Lưu', exact: true }).click();
  await expect(
    sidebar.getByRole('button', { name: 'Phiên đổi tên qua E2E', exact: true }),
  ).toBeVisible();

  await sidebar.getByRole('button', { name: `Xóa phiên ${onboardingTitle}`, exact: true }).click();
  await page.getByRole('button', { name: 'Xóa cuộc trò chuyện', exact: true }).click();
  await expect(
    sidebar.getByRole('button', { name: onboardingTitle, exact: true }),
  ).not.toBeVisible();
  // The renamed session survives the unrelated delete.
  await expect(
    sidebar.getByRole('button', { name: 'Phiên đổi tên qua E2E', exact: true }),
  ).toBeVisible();
});
