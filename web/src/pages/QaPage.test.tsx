import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { createApiClient, type ApiClient } from '../api/client';
import { installMockFetch, mockControl, resetMockState, uninstallMockFetch } from '../mocks';
import { QA_STREAM_MARKERS } from '../mocks/handlers/qa';
import { mockUuid } from '../mocks/ids';
import { RouterProvider } from '../state/RouterProvider';
import { ScopeProvider } from '../state/ScopeProvider';
import { createScopeManager, type ScopeManager } from '../state/scope';
import { QaPage } from './QaPage';

const DEMO_EMAIL = 'demo@markhand.test';
const DEMO_PASSWORD = 'demo-password';
const HANDBOOK_COLLECTION_ID = mockUuid(10);

async function loggedInClient(): Promise<ApiClient> {
  const client = createApiClient({ baseUrl: '' });
  await client.login({ email: DEMO_EMAIL, password: DEMO_PASSWORD });
  return client;
}

function renderQa(client: ApiClient, collectionId?: string, manager?: ScopeManager) {
  return render(
    <RouterProvider>
      <ScopeProvider manager={manager}>
        <QaPage collectionId={collectionId} client={client} />
      </ScopeProvider>
    </RouterProvider>,
  );
}

function chatLog() {
  return screen.getByRole('log', { name: 'Lịch sử hỏi đáp' });
}

function sidebar() {
  return screen.getByRole('navigation', { name: 'Lịch sử hỏi đáp' });
}

async function askQuestion(question: string) {
  fireEvent.change(screen.getByLabelText('Câu hỏi'), { target: { value: question } });
  fireEvent.click(screen.getByRole('button', { name: 'Hỏi' }));
}

async function switchToSearchTab() {
  fireEvent.click(screen.getByRole('tab', { name: 'Tìm kiếm' }));
  await screen.findByLabelText('Từ khóa');
}

describe('QaPage', () => {
  beforeEach(() => {
    installMockFetch();
    resetMockState();
  });

  afterEach(() => {
    cleanup();
    uninstallMockFetch();
  });

  describe('Tìm kiếm tab', () => {
    it('searches indexed documents and previews a hit with its version badge', async () => {
      const client = await loggedInClient();
      renderQa(client, HANDBOOK_COLLECTION_ID);
      await switchToSearchTab();

      fireEvent.change(screen.getByLabelText('Từ khóa'), { target: { value: 'hội nhập' } });
      fireEvent.click(screen.getByRole('button', { name: 'Tìm kiếm' }));

      expect(await screen.findByText('Onboarding Guide.pdf')).toBeVisible();
      expect(screen.getAllByText(/khóa đào tạo hội nhập/).length).toBeGreaterThan(0);

      fireEvent.click(screen.getByRole('button', { name: 'Xem trước' }));
      expect(await screen.findByTestId('qa-preview-markdown')).toHaveTextContent(
        'Mock preview content for version 1',
      );
    });

    it('shows "no results" copy for a query that matches nothing indexed', async () => {
      const client = await loggedInClient();
      renderQa(client, HANDBOOK_COLLECTION_ID);
      await switchToSearchTab();

      fireEvent.change(screen.getByLabelText('Từ khóa'), {
        target: { value: 'khong-co-gi-khop-cau-nay' },
      });
      fireEvent.click(screen.getByRole('button', { name: 'Tìm kiếm' }));

      expect(
        await screen.findByText(/Không tìm thấy kết quả phù hợp với "khong-co-gi-khop-cau-nay"/),
      ).toBeVisible();
    });
  });

  describe('Hỏi đáp tab — streaming + footnote citations', () => {
    it('streams a grounded answer with numbered footnote markers and a matching source block', async () => {
      const client = await loggedInClient();
      renderQa(client);

      await askQuestion('Lộ trình quý 3 tập trung vào việc gì?');

      await waitFor(() => {
        expect(within(chatLog()).getByTestId('qa-answer')).toHaveTextContent(
          'Lộ trình quý 3 tập trung vào tối ưu hiệu năng lập chỉ mục.',
        );
      });
      // The raw `[CITE-0001]` token is replaced by a `[1]` footnote marker —
      // the raw tag itself is never shown inline any more.
      expect(within(chatLog()).getByTestId('qa-answer')).toHaveTextContent('[1]');
      expect(within(chatLog()).queryByText('CITE-0001')).not.toBeInTheDocument();
      expect(await screen.findByText('Nguồn trích dẫn')).toBeVisible();
      // Quotes stay collapsed by default; expanding one confirms the footnote
      // still carries the passage text for deep inspection.
      const expandButtons = screen.getAllByRole('button', { name: 'Hiện đoạn trích' });
      expect(expandButtons.length).toBeGreaterThan(0);
      fireEvent.click(expandButtons[0]);
      expect(screen.getAllByTestId('qa-footnote-quote').length).toBeGreaterThan(0);
    });

    it('citation_revoked mid-answer keeps the partial answer + notice visible, and IS saved to history (only a genuine error/cancel is skipped)', async () => {
      const client = await loggedInClient();
      renderQa(client);

      await askQuestion(`Câu hỏi sẽ bị thu hồi giữa chừng ${QA_STREAM_MARKERS.citationRevoked}`);

      await waitFor(() => {
        expect(screen.getByText(/Trích dẫn đã bị thu hồi giữa chừng/)).toBeVisible();
      });
      expect(within(chatLog()).getByTestId('qa-answer')).toHaveTextContent(/\S/);

      await waitFor(() => {
        expect(
          within(sidebar()).getByRole('button', { name: /^Câu hỏi sẽ bị thu hồi giữa chừng/ }),
        ).toBeVisible();
      });
    });

    it('provider-fallback still answers (extractive), and IS saved to history', async () => {
      const client = await loggedInClient();
      renderQa(client);

      await askQuestion(`Câu hỏi dùng fallback ${QA_STREAM_MARKERS.providerFallback}`);

      await waitFor(() => {
        expect(
          screen.getByText('Dùng đoạn nguồn (câu mô hình không đạt kiểm chứng)'),
        ).toBeVisible();
      });
      await waitFor(() => {
        expect(
          within(sidebar()).getByRole('button', { name: /^Câu hỏi dùng fallback/ }),
        ).toBeVisible();
      });
    });

    it('"Hủy" aborts an in-flight turn and does NOT save it to history', async () => {
      const client = await loggedInClient();
      renderQa(client);
      const sessionsBefore = (await within(sidebar()).findAllByRole('listitem')).map(
        (li) => li.textContent,
      );

      fireEvent.change(screen.getByLabelText('Câu hỏi'), {
        target: { value: 'Câu hỏi sẽ bị hủy giữa chừng' },
      });
      fireEvent.click(screen.getByRole('button', { name: 'Hỏi' }));
      fireEvent.click(screen.getByRole('button', { name: 'Hủy' }));

      expect(screen.getByText(/Đã hủy câu trả lời này/)).toBeVisible();
      // Give any (incorrect) history-append attempt a chance to have shown up.
      await waitFor(() => {
        const sessionsAfter = within(sidebar())
          .getAllByRole('listitem')
          .map((li) => li.textContent);
        expect(sessionsAfter).toEqual(sessionsBefore);
      });
    });

    it('a network/HTTP error turn does NOT get saved to history', async () => {
      const client = await loggedInClient();
      renderQa(client);
      mockControl.forceStatus('askStream', 404, { times: 1 });

      await askQuestion('Câu hỏi sẽ lỗi mạng');

      await waitFor(() => {
        expect(screen.getByRole('alert')).toBeVisible();
      });
      expect(
        within(sidebar()).queryByRole('button', { name: /^Câu hỏi sẽ lỗi mạng/ }),
      ).not.toBeInTheDocument();
    });
  });

  describe('Ghi lịch sử (part A)', () => {
    it('a completed turn creates a session titled from the question, and reopening it from the sidebar replays the transcript', async () => {
      const client = await loggedInClient();
      renderQa(client);
      // Deliberately reworded from the seeded fixture's own "Lộ trình quý 3
      // tập trung vào việc gì?" title (`mocks/fixtures.ts`'s
      // `DEMO_CHAT_SESSION_ROADMAP_ID`) so the two never collide as the same
      // title, while still sharing enough tokens with "Roadmap.xlsx"'s
      // seeded passage to get a real grounded citation back.
      const question = 'Lộ trình quý 3 này tập trung nhiều nhất vào việc gì vậy nhỉ?';

      await askQuestion(question);
      await waitFor(() => {
        expect(within(chatLog()).getByTestId('qa-answer')).toHaveTextContent(
          'Lộ trình quý 3 tập trung vào tối ưu hiệu năng lập chỉ mục.',
        );
      });

      await waitFor(() => {
        expect(within(sidebar()).getByRole('button', { name: question })).toBeVisible();
      });

      // Start a brand-new conversation, ask a second (unrelated) question —
      // it must NOT be appended to the just-created session above.
      fireEvent.click(within(sidebar()).getByRole('button', { name: 'Cuộc trò chuyện mới' }));
      await waitFor(() =>
        expect(
          screen.getByText('Chưa có câu hỏi nào trong cuộc trò chuyện này — đặt câu hỏi bên dưới.'),
        ).toBeVisible(),
      );

      await askQuestion('cau-hoi-khong-khop-bat-ky-tai-lieu-nao');
      await waitFor(() => {
        expect(within(chatLog()).getByTestId('qa-answer')).toHaveTextContent(
          'Không tìm thấy nội dung liên quan trong tài liệu đã lập chỉ mục để trả lời câu hỏi này.',
        );
      });
      await waitFor(() => {
        expect(
          within(sidebar()).getByRole('button', { name: 'cau-hoi-khong-khop-bat-ky-tai-lieu-nao' }),
        ).toBeVisible();
      });

      // Reopen the first session — its transcript (question + answer +
      // citation) must still be there, untouched by the second conversation.
      fireEvent.click(within(sidebar()).getByRole('button', { name: question }));
      await waitFor(() => {
        expect(screen.getByText('Phiên đã lưu — tài liệu có thể đã thay đổi.')).toBeVisible();
      });
      await waitFor(() => {
        expect(screen.getByTestId('qa-answer')).toHaveTextContent(
          'Lộ trình quý 3 tập trung vào tối ưu hiệu năng lập chỉ mục.',
        );
      });
      expect(screen.getByText('Nguồn trích dẫn')).toBeVisible();
    });
  });

  describe('Sidebar — seeded history, rename, delete', () => {
    it('lists seeded sessions, most recently active first', async () => {
      const client = await loggedInClient();
      renderQa(client);

      const items = await within(sidebar()).findAllByRole('listitem');
      expect(items.length).toBeGreaterThanOrEqual(2);
      // "Lộ trình quý 3..." (activityRank 2) was seeded more recently-active
      // than "Nhân viên mới..." (activityRank 1).
      expect(items[0]).toHaveTextContent('Lộ trình quý 3 tập trung vào việc gì?');
      expect(items[1]).toHaveTextContent('Nhân viên mới cần hoàn thành gì trong 30 ngày đầu?');
    });

    it('renames a session inline', async () => {
      const client = await loggedInClient();
      renderQa(client);

      await within(sidebar()).findByRole('button', {
        name: 'Lộ trình quý 3 tập trung vào việc gì?',
      });
      fireEvent.click(
        within(sidebar()).getByRole('button', {
          name: 'Đổi tên phiên Lộ trình quý 3 tập trung vào việc gì?',
        }),
      );
      const input = within(sidebar()).getByLabelText(
        'Tên mới cho phiên Lộ trình quý 3 tập trung vào việc gì?',
      );
      fireEvent.change(input, { target: { value: 'Tên phiên mới' } });
      fireEvent.click(within(sidebar()).getByRole('button', { name: 'Lưu' }));

      await waitFor(() => {
        expect(within(sidebar()).getByRole('button', { name: 'Tên phiên mới' })).toBeVisible();
      });
    });

    it('deletes a session after confirming', async () => {
      const client = await loggedInClient();
      renderQa(client);

      await within(sidebar()).findByRole('button', {
        name: 'Nhân viên mới cần hoàn thành gì trong 30 ngày đầu?',
      });
      fireEvent.click(
        within(sidebar()).getByRole('button', {
          name: 'Xóa phiên Nhân viên mới cần hoàn thành gì trong 30 ngày đầu?',
        }),
      );
      fireEvent.click(screen.getByRole('button', { name: 'Xóa cuộc trò chuyện' }));

      await waitFor(() => {
        expect(
          within(sidebar()).queryByRole('button', {
            name: 'Nhân viên mới cần hoàn thành gì trong 30 ngày đầu?',
          }),
        ).not.toBeInTheDocument();
        // The other seeded session is untouched — checked in the same poll so
        // a transient "list momentarily empty while refetching" state can't
        // make this pass for the wrong reason.
        expect(
          within(sidebar()).getByRole('button', { name: 'Lộ trình quý 3 tập trung vào việc gì?' }),
        ).toBeVisible();
      });
    });
  });

  describe('Footnote — page number + multi-document note (part C)', () => {
    it('shows "Trang X" when a citation carries a page, and "Tổng hợp từ N tài liệu" when more than one document is cited', async () => {
      const client = await loggedInClient();
      renderQa(client);

      fireEvent.click(
        await within(sidebar()).findByRole('button', {
          name: 'Nhân viên mới cần hoàn thành gì trong 30 ngày đầu?',
        }),
      );

      const answers = await screen.findAllByTestId('qa-answer');
      expect(answers).toHaveLength(2);
      // Second historical turn cites two different documents.
      expect(screen.getByText('Tổng hợp từ 2 tài liệu.')).toBeVisible();
      expect(screen.getAllByText(/Trang 3/).length).toBeGreaterThan(0);
    });
  });

  describe('Picker đa dự án (part B)', () => {
    it('narrows both search and ask to the selected project, and clears back to "Tất cả dự án"', async () => {
      const client = await loggedInClient();
      renderQa(client);

      const scopeTrigger = screen.getByRole('combobox', { name: 'Phạm vi dự án' });
      expect(scopeTrigger).toHaveTextContent('Tất cả dự án');
      fireEvent.click(scopeTrigger);
      fireEvent.click(await screen.findByRole('checkbox', { name: 'Nhân sự' }));
      expect(scopeTrigger).toHaveTextContent('1 dự án: Nhân sự');

      await switchToSearchTab();
      fireEvent.change(screen.getByLabelText('Từ khóa'), {
        target: { value: 'lộ trình quý 3' },
      });
      fireEvent.click(screen.getByRole('button', { name: 'Tìm kiếm' }));
      expect(
        await screen.findByText(/Không tìm thấy kết quả phù hợp với "lộ trình quý 3"/),
      ).toBeVisible();

      fireEvent.click(screen.getByRole('tab', { name: 'Hỏi đáp' }));
      await askQuestion('Lộ trình quý 3 tập trung vào việc gì?');
      await waitFor(() => {
        expect(within(chatLog()).getByTestId('qa-answer')).toHaveTextContent(
          'Không tìm thấy nội dung liên quan trong tài liệu đã lập chỉ mục để trả lời câu hỏi này.',
        );
      });

      // The popover stayed open the whole time (multi-select never
      // auto-closes on a single pick) — selecting a second project updates
      // the trigger label without needing to reopen it.
      fireEvent.click(await screen.findByRole('checkbox', { name: 'Sản phẩm' }));
      expect(scopeTrigger).toHaveTextContent('2 dự án: Nhân sự, Sản phẩm');

      // "Tất cả dự án" checkbox clears every selection.
      fireEvent.click(screen.getByRole('checkbox', { name: 'Tất cả dự án' }));
      expect(scopeTrigger).toHaveTextContent('Tất cả dự án');
    });

    it('resets to "Tất cả dự án" (and clears the sidebar/chat) when the org switches', async () => {
      const client = await loggedInClient();
      const manager = createScopeManager();
      manager.setScope({ orgId: 'org-a', permissions: [], allowedCollectionIds: [] });
      renderQa(client, undefined, manager);

      const scopeTrigger = screen.getByRole('combobox', { name: 'Phạm vi dự án' });
      fireEvent.click(scopeTrigger);
      fireEvent.click(await screen.findByRole('checkbox', { name: 'Nhân sự' }));
      expect(scopeTrigger).toHaveTextContent('1 dự án: Nhân sự');

      await within(sidebar()).findByRole('button', {
        name: 'Lộ trình quý 3 tập trung vào việc gì?',
      });

      act(() => {
        manager.setScope({ orgId: 'org-b', permissions: [], allowedCollectionIds: [] });
      });

      expect(screen.getByRole('combobox', { name: 'Phạm vi dự án' })).toHaveTextContent(
        'Tất cả dự án',
      );
      await waitFor(() => {
        expect(
          within(sidebar()).queryByRole('button', {
            name: 'Lộ trình quý 3 tập trung vào việc gì?',
          }),
        ).not.toBeInTheDocument();
      });
    });
  });

  describe('as_of mode — timestamp required, scope-wide request shape', () => {
    async function selectAsOfMode() {
      fireEvent.click(screen.getByRole('combobox', { name: 'Chế độ truy vấn' }));
      fireEvent.click(await screen.findByRole('option', { name: 'Tại một thời điểm (as-of)' }));
    }

    it('keeps submit disabled until a timestamp is supplied, and marks the control required', async () => {
      const client = await loggedInClient();
      renderQa(client);
      await selectAsOfMode();

      const asOfInput = screen.getByLabelText('Thời điểm (as-of)');
      expect(asOfInput).toBeRequired();

      fireEvent.change(screen.getByLabelText('Câu hỏi'), {
        target: { value: 'Ngân sách vận hành là bao nhiêu?' },
      });
      expect(screen.getByRole('button', { name: 'Hỏi' })).toBeDisabled();

      fireEvent.change(asOfInput, { target: { value: '2026-01-01T01:00' } });
      expect(screen.getByRole('button', { name: 'Hỏi' })).toBeEnabled();
    });

    it('sends normalized ISO asOf without documentId on a valid as_of submit', async () => {
      const client = await loggedInClient();
      const captured: Record<string, unknown>[] = [];
      const installedFetch = globalThis.fetch;
      globalThis.fetch = async (input, init) => {
        const url =
          typeof input === 'string' ? input : input instanceof URL ? input.href : input.url;
        if (url.includes('/ask/stream') && typeof init?.body === 'string') {
          captured.push(JSON.parse(init.body) as Record<string, unknown>);
        }
        return installedFetch(input, init);
      };

      try {
        renderQa(client);
        await selectAsOfMode();
        fireEvent.change(screen.getByLabelText('Thời điểm (as-of)'), {
          target: { value: '2026-01-01T01:00' },
        });
        await askQuestion('Ngân sách vận hành là bao nhiêu?');

        await waitFor(() => {
          expect(captured.length).toBeGreaterThan(0);
        });
        const body = captured[0];
        expect(body.mode).toBe('as_of');
        expect(body.asOf).toBe(new Date('2026-01-01T01:00').toISOString());
        expect(body).not.toHaveProperty('documentId');
      } finally {
        globalThis.fetch = installedFetch;
      }
    });
  });
});
