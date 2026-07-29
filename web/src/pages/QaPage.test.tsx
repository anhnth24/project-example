import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { createApiClient, type ApiClient } from '../api/client';
import { installMockFetch, resetMockState, uninstallMockFetch } from '../mocks';
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

async function askQuestion(question: string) {
  fireEvent.change(screen.getByLabelText('Câu hỏi'), { target: { value: question } });
  fireEvent.click(screen.getByRole('button', { name: 'Hỏi' }));
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

  it('searches indexed documents and previews a hit with its version badge', async () => {
    const client = await loggedInClient();
    renderQa(client, HANDBOOK_COLLECTION_ID);

    fireEvent.change(screen.getByLabelText('Từ khóa'), { target: { value: 'hội nhập' } });
    fireEvent.click(screen.getByRole('button', { name: 'Tìm kiếm' }));

    expect(await screen.findByText('Onboarding Guide.pdf')).toBeVisible();
    expect(screen.getAllByText(/khóa đào tạo hội nhập/).length).toBeGreaterThan(0);
    expect(screen.getAllByText('CITE-0001').length).toBeGreaterThan(0);

    fireEvent.click(screen.getByRole('button', { name: 'Xem trước' }));
    expect(await screen.findByTestId('qa-preview-markdown')).toHaveTextContent(
      'Mock preview content for version 1',
    );
    expect(screen.getByText('Phiên bản 1 (bản hiện hành)')).toBeVisible();
  });

  it('shows "no results" copy for a query that matches nothing indexed', async () => {
    const client = await loggedInClient();
    renderQa(client, HANDBOOK_COLLECTION_ID);

    fireEvent.change(screen.getByLabelText('Từ khóa'), {
      target: { value: 'khong-co-gi-khop-cau-nay' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Tìm kiếm' }));

    expect(
      await screen.findByText(/Không tìm thấy kết quả phù hợp với "khong-co-gi-khop-cau-nay"/),
    ).toBeVisible();
  });

  it('streams a grounded answer token-by-token through to citations (default scenario)', async () => {
    const client = await loggedInClient();
    renderQa(client);

    await askQuestion('Lộ trình quý 3 tập trung vào việc gì?');

    await waitFor(() => {
      expect(screen.getByTestId('qa-answer')).toHaveTextContent(
        'Lộ trình quý 3 tập trung vào tối ưu hiệu năng lập chỉ mục.',
      );
    });
    expect(await screen.findByText('CITE-0001')).toBeVisible();
    expect(
      screen.getByText(
        /Trích dẫn ở đây chưa kèm định danh tài liệu\/phiên bản theo hợp đồng hiện tại/,
      ),
    ).toBeVisible();
    // Default scenario is not the fallback path.
    expect(screen.queryByText('Trả lời trích xuất (không qua LLM)')).not.toBeInTheDocument();
  });

  it('reports "no answer" (zero citations) honestly for a question with no matching passage', async () => {
    const client = await loggedInClient();
    renderQa(client);

    await askQuestion('cau-hoi-khong-khop-bat-ky-tai-lieu-nao');

    await waitFor(() => {
      expect(screen.getByTestId('qa-answer')).toHaveTextContent(
        'Không tìm thấy nội dung liên quan trong tài liệu đã lập chỉ mục để trả lời câu hỏi này.',
      );
    });
  });

  it('citation_revoked mid-answer: partial answer stays visible with an accessible revoked notice, no crash', async () => {
    const client = await loggedInClient();
    renderQa(client);

    await askQuestion(`Lộ trình quý 3 là gì? ${QA_STREAM_MARKERS.citationRevoked}`);

    await waitFor(() => {
      expect(screen.getByText(/Trích dẫn đã bị thu hồi giữa chừng/)).toBeVisible();
    });
    // A partial answer (the tokens that streamed before the revoke) is kept, not blanked.
    expect(screen.getByTestId('qa-answer')).toHaveTextContent(/\S/);
    // The revoke happens before ask.citations ever arrives.
    expect(screen.queryByText('CITE-0001')).not.toBeInTheDocument();
  });

  it('provider-fallback: still answers via extractive tokens, labelled as extractive, with a warning', async () => {
    const client = await loggedInClient();
    renderQa(client);

    await askQuestion(`Lộ trình quý 3 là gì? ${QA_STREAM_MARKERS.providerFallback}`);

    await waitFor(() => {
      expect(screen.getByText('Trả lời trích xuất (không qua LLM)')).toBeVisible();
    });
    expect(screen.getByText(/Nhà cung cấp LLM tạm thời không khả dụng/)).toBeVisible();
    expect(screen.getByTestId('qa-answer')).toHaveTextContent(
      'Lộ trình quý 3 tập trung vào tối ưu hiệu năng lập chỉ mục.',
    );
  });

  it('"Hủy" aborts an in-flight turn, freezing it with a cancelled notice instead of blanking it', async () => {
    const client = await loggedInClient();
    renderQa(client);

    // Deliberately synchronous (no `await` between these) so the assertion
    // does not race the mock's own near-instant stream resolution (see
    // `mocks/handlers/qa.ts`'s module doc: the whole response is a
    // pre-serialized string with nothing to actually await) — clicking
    // "Hủy" sets this turn's status to `'cancelled'` synchronously
    // (`ChatTurnBubble`'s own `abort()`, layered on `useAskStream`'s
    // `reset()`), which hides the button and re-enables the composer
    // regardless of whether the stream had already finished on its own by
    // this point.
    fireEvent.change(screen.getByLabelText('Câu hỏi'), {
      target: { value: 'Lộ trình quý 3 là gì?' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Hỏi' }));
    fireEvent.click(screen.getByRole('button', { name: 'Hủy' }));

    expect(screen.queryByRole('button', { name: 'Hủy' })).not.toBeInTheDocument();
    expect(screen.getByText(/Đã hủy câu trả lời này/)).toBeVisible();
    // The composer re-enables immediately so the next question can be typed.
    expect(screen.getByLabelText('Câu hỏi')).not.toBeDisabled();
  });

  it("keeps two turns' history: a second question does not disturb the first turn's answer", async () => {
    const client = await loggedInClient();
    renderQa(client);

    await askQuestion('Lộ trình quý 3 tập trung vào việc gì?');
    await waitFor(() => {
      expect(screen.getByTestId('qa-answer')).toHaveTextContent(
        'Lộ trình quý 3 tập trung vào tối ưu hiệu năng lập chỉ mục.',
      );
    });
    // The composer re-enables once the first turn settles, allowing a second question.
    await waitFor(() => expect(screen.getByLabelText('Câu hỏi')).not.toBeDisabled());

    await askQuestion('cau-hoi-khong-khop-bat-ky-tai-lieu-nao');
    await waitFor(() => {
      const answers = screen.getAllByTestId('qa-answer');
      expect(answers).toHaveLength(2);
      expect(answers[1]).toHaveTextContent(
        'Không tìm thấy nội dung liên quan trong tài liệu đã lập chỉ mục để trả lời câu hỏi này.',
      );
    });
    // The first turn's answer is still there, unchanged, in the log.
    const answers = screen.getAllByTestId('qa-answer');
    expect(answers[0]).toHaveTextContent(
      'Lộ trình quý 3 tập trung vào tối ưu hiệu năng lập chỉ mục.',
    );
    expect(screen.getByRole('log', { name: 'Lịch sử hỏi đáp' })).toBeVisible();
  });

  it('citation_revoked on a second turn does not revoke/disturb the first, already-completed turn', async () => {
    const client = await loggedInClient();
    renderQa(client);

    await askQuestion('Lộ trình quý 3 tập trung vào việc gì?');
    await waitFor(() => {
      expect(screen.getByTestId('qa-answer')).toHaveTextContent(
        'Lộ trình quý 3 tập trung vào tối ưu hiệu năng lập chỉ mục.',
      );
    });
    expect(await screen.findByText('CITE-0001')).toBeVisible();
    await waitFor(() => expect(screen.getByLabelText('Câu hỏi')).not.toBeDisabled());

    await askQuestion(`Lộ trình quý 3 là gì? ${QA_STREAM_MARKERS.citationRevoked}`);
    await waitFor(() => {
      expect(screen.getByText(/Trích dẫn đã bị thu hồi giữa chừng/)).toBeVisible();
    });

    // The first turn's answer and citation are still present, unaffected by
    // the second turn's revoke.
    const answers = screen.getAllByTestId('qa-answer');
    expect(answers).toHaveLength(2);
    expect(answers[0]).toHaveTextContent(
      'Lộ trình quý 3 tập trung vào tối ưu hiệu năng lập chỉ mục.',
    );
    expect(screen.getByText('CITE-0001')).toBeVisible();
  });

  it('switching org clears the whole chat history in-memory (no stale-org bubble ever painted)', async () => {
    const client = await loggedInClient();
    const manager = createScopeManager();
    manager.setScope({ orgId: 'org-a', permissions: [], allowedCollectionIds: [] });
    renderQa(client, undefined, manager);

    await askQuestion('Lộ trình quý 3 tập trung vào việc gì?');
    await waitFor(() => {
      expect(screen.getByTestId('qa-answer')).toHaveTextContent(
        'Lộ trình quý 3 tập trung vào tối ưu hiệu năng lập chỉ mục.',
      );
    });

    act(() => {
      manager.setScope({ orgId: 'org-b', permissions: [], allowedCollectionIds: [] });
    });

    expect(screen.queryByTestId('qa-answer')).not.toBeInTheDocument();
    expect(
      screen.getByText('Chưa có câu hỏi nào trong phiên này — đặt câu hỏi bên dưới.'),
    ).toBeVisible();
    // The composer is enabled again — nothing is left "busy" from the discarded turn.
    expect(screen.getByLabelText('Câu hỏi')).not.toBeDisabled();
  });
});
