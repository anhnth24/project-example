// [inter-v2-06] Web Q&A UI: mock vs real provider
//
// ============================================================================
// GIẢI THÍCH: Mock Handler vs API Thật & Khác biệt vận hành
// ============================================================================
// 1. Mock Handler (`web/src/mocks/handlers/qa.ts`):
//    - Hoạt động ở tầng fetch mock / MSW trong browser (`VITE_MARKHAND_MOCK=1`).
//    - Trả về toàn bộ luồng SSE pre-serialized trong 1 response (`rawBody` text/event-stream)
//      gồm các frames (`ask.started` -> `ask.token`* -> `ask.citations` -> `ask.version_context`
//      -> `ask.completed` -> `stream.closed`).
//    - Deterministic, không có độ trễ mạng hay thời gian chờ LLM sinh token thực tế,
//      giúp E2E test chạy nhanh và ổn định (không bị sleep-race).
//
// 2. Real Backend API (`crates/server/src/services/qa/ask_stream.rs`):
//    - Kết nối với pipeline tìm kiếm ngữ nghĩa (Qdrant vector embeddings + PostgreSQL)
//      và streaming trực tiếp từ LLM provider (ví dụ OpenRouter) qua SSE bytes thực tế.
//    - Token được yield theo thời gian thực (wall-clock ticks) từ LLM stream.
//    - Cần các biến môi trường cấu hình backend cho LLM/Chat:
//        + `MARKHAND_CHAT_*` / `FILECONV_LLM_*` (ví dụ `FILECONV_LLM_API_KEY`, `FILECONV_LLM_MODEL`,
//          `FILECONV_LLM_BASE_URL`)
//        + Yêu cầu tài liệu đã được index thành công (embedding pipeline hoàn tất).
//
// 3. Quy tắc kiểm thử:
//    - Không dùng sleep/setTimeout — dùng Playwright web-first assertions (expect poll).
//    - Không kiểm tra React internal state — chỉ tương tác và kiểm tra qua role/text/DOM visible.
// ============================================================================

import { expect, test } from '@playwright/test';
import { login } from './support';

const QUESTION = 'Lộ trình quý 3 tập trung vào việc gì?';

test('Q&A happy path in mock mode: ask question, stream answer and verify citations/source', async ({
  page,
}) => {
  // 1. Đăng nhập và điều hướng đến trang Hỏi đáp
  await login(page);
  await page.getByRole('link', { name: 'Hỏi đáp' }).click();

  // 2. Nhập câu hỏi vào composer và gửi
  const questionBox = page.getByRole('textbox', { name: 'Câu hỏi' });
  await expect(questionBox).toBeVisible();
  await questionBox.fill(QUESTION);
  await page.getByRole('button', { name: 'Hỏi', exact: true }).click();

  // 3. Assert UI behavior: Câu trả lời hiển thị chính xác (grounded answer)
  const chatLog = page.getByRole('log', { name: 'Lịch sử hỏi đáp' });
  const answer = chatLog.getByTestId('qa-answer').first();
  await expect(answer).toBeVisible();
  await expect(answer).toContainText('Lộ trình quý 3 tập trung vào tối ưu hiệu năng lập chỉ mục.');

  // 4. Assert UI behavior: Nguồn trích dẫn (citation/source) hiển thị
  await expect(chatLog.getByText('Nguồn trích dẫn').first()).toBeVisible();
  await expect(chatLog.getByText('Roadmap.xlsx').first()).toBeVisible();

  // 5. Đảm bảo composer quay lại trạng thái sẵn sàng cho lượt hỏi tiếp theo
  await expect(questionBox).toBeEnabled();
});
