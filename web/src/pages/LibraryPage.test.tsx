import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { createApiClient, type ApiClient } from '../api/client';
import type { components } from '../api/generated/contract';
import { installMockFetch, mockControl, resetMockState, uninstallMockFetch } from '../mocks';
import { getStore, nextId } from '../mocks/fixtures';
import { mockTimestamp, mockUuid } from '../mocks/ids';
import { RouterProvider } from '../state/RouterProvider';
import { ScopeProvider } from '../state/ScopeProvider';
import { createScopeManager } from '../state/scope';
import { LibraryPage } from './LibraryPage';

type Collection = components['schemas']['Collection'];
type LibraryDocument = components['schemas']['Document'];
type DocumentVersion = components['schemas']['DocumentVersion'];

const DEMO_EMAIL = 'demo@markhand.test';
const DEMO_PASSWORD = 'demo-password';

// Seeded in mocks/fixtures.ts: "Employee Handbook" (2 documents) / "Product Specs" (1 document).
const HANDBOOK_COLLECTION_ID = mockUuid(10);

function seedCollection(overrides: Partial<Collection> = {}): Collection {
  const collection: Collection = {
    id: nextId(),
    name: 'Bộ sưu tập kiểm thử',
    slug: 'test-collection',
    description: null,
    visibility: 'org',
    createdAt: mockTimestamp(0),
    ...overrides,
  };
  getStore().collections.push(collection);
  return collection;
}

function seedDocument(
  collectionId: string,
  overrides: Partial<LibraryDocument> = {},
): LibraryDocument {
  const doc: LibraryDocument = {
    id: nextId(),
    collectionId,
    title: 'Tài liệu.pdf',
    state: 'indexed',
    currentVersionId: null,
    createdAt: mockTimestamp(0),
    updatedAt: mockTimestamp(0),
    ...overrides,
  };
  const docs = getStore().documents.get(collectionId) ?? [];
  docs.push(doc);
  getStore().documents.set(collectionId, docs);
  return doc;
}

/** A document that already has a current version, so `GET .../preview` has something to return. */
function seedDocumentWithVersion(
  collectionId: string,
  overrides: Partial<LibraryDocument> = {},
  versionOverrides: Partial<DocumentVersion> = {},
): LibraryDocument {
  const versionId = nextId();
  const doc = seedDocument(collectionId, { currentVersionId: versionId, ...overrides });
  const version: DocumentVersion = {
    id: versionId,
    documentId: doc.id,
    versionNumber: 1,
    isCurrent: true,
    sourceContentSha256: 'a'.repeat(64),
    effectiveFrom: mockTimestamp(0),
    effectiveTo: null,
    changeSummary: null,
    createdAt: mockTimestamp(0),
    ...versionOverrides,
  };
  getStore().versions.set(doc.id, [version]);
  return doc;
}

async function loggedInClient(): Promise<ApiClient> {
  const client = createApiClient({ baseUrl: '' });
  await client.login({ email: DEMO_EMAIL, password: DEMO_PASSWORD });
  return client;
}

function renderLibrary(client: ApiClient, collectionId?: string) {
  return render(
    <RouterProvider>
      <ScopeProvider>
        <LibraryPage collectionId={collectionId} client={client} />
      </ScopeProvider>
    </RouterProvider>,
  );
}

describe('LibraryPage', () => {
  beforeEach(() => {
    installMockFetch();
    resetMockState();
  });

  afterEach(() => {
    cleanup();
    uninstallMockFetch();
  });

  it('renders the document list for a collection from the mock', async () => {
    const client = await loggedInClient();
    renderLibrary(client, HANDBOOK_COLLECTION_ID);

    expect(await screen.findByRole('button', { name: /Onboarding Guide\.pdf/ })).toBeVisible();
    expect(screen.getByRole('button', { name: /Leave Policy\.docx/ })).toBeVisible();
    expect(
      screen.getByRole('heading', { name: `Bộ sưu tập ${HANDBOOK_COLLECTION_ID}` }),
    ).toBeVisible();
  });

  it('shows a prompt instead of a document list when no collection is selected', async () => {
    const client = await loggedInClient();
    renderLibrary(client, undefined);

    expect(
      await screen.findByText('Chọn một bộ sưu tập ở trên để xem danh sách tài liệu.'),
    ).toBeVisible();
    // Collection navigation is still populated from the real GET /collections mock.
    expect(await screen.findByRole('link', { name: 'Employee Handbook' })).toBeVisible();
    expect(screen.getByRole('link', { name: 'Product Specs' })).toBeVisible();
  });

  it('cursor pagination advances to the next page and back, round-tripping the real PageInfo cursor', async () => {
    const collection = seedCollection();
    for (let i = 1; i <= 25; i += 1) {
      seedDocument(collection.id, { title: `Doc-${String(i).padStart(2, '0')}.pdf` });
    }
    const client = await loggedInClient();
    renderLibrary(client, collection.id);

    expect(await screen.findByRole('button', { name: /Doc-01\.pdf/ })).toBeVisible();
    expect(screen.getByRole('button', { name: /Doc-20\.pdf/ })).toBeVisible();
    expect(screen.queryByRole('button', { name: /Doc-21\.pdf/ })).not.toBeInTheDocument();
    expect(screen.getByText('Trang 1')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Trang trước' })).toBeDisabled();

    fireEvent.click(screen.getByRole('button', { name: 'Trang sau' }));

    await waitFor(() => expect(screen.getByRole('button', { name: /Doc-21\.pdf/ })).toBeVisible());
    expect(screen.queryByRole('button', { name: /Doc-01\.pdf/ })).not.toBeInTheDocument();
    expect(screen.getByText('Trang 2')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Trang sau' })).toBeDisabled();

    fireEvent.click(screen.getByRole('button', { name: 'Trang trước' }));

    await waitFor(() => expect(screen.getByRole('button', { name: /Doc-01\.pdf/ })).toBeVisible());
    expect(screen.queryByRole('button', { name: /Doc-21\.pdf/ })).not.toBeInTheDocument();
    expect(screen.getByText('Trang 1')).toBeVisible();
  });

  it('renders each of the six document states distinctly', async () => {
    const collection = seedCollection();
    const states: Array<LibraryDocument['state']> = [
      'uploaded',
      'converting',
      'converted',
      'indexing',
      'indexed',
      'failed',
    ];
    for (const state of states) {
      seedDocument(collection.id, { title: `State-${state}.pdf`, state });
    }
    const client = await loggedInClient();
    renderLibrary(client, collection.id);

    await screen.findByRole('button', { name: /State-uploaded\.pdf/ });

    expect(screen.getByText('Đã tải lên')).toBeVisible();
    expect(screen.getByText('Đang chuyển đổi')).toBeVisible();
    expect(screen.getByText('Đã chuyển đổi')).toBeVisible();
    expect(screen.getByText('Đang lập chỉ mục')).toBeVisible();
    expect(screen.getByText('Đã lập chỉ mục')).toBeVisible();
    expect(screen.getByText('Lỗi chuyển đổi')).toBeVisible();
  });

  it('shows an honest empty state for a collection with no documents at all', async () => {
    const collection = seedCollection();
    const client = await loggedInClient();
    renderLibrary(client, collection.id);

    expect(await screen.findByText('Chưa có tài liệu nào trong bộ sưu tập này.')).toBeVisible();
  });

  it('shows a distinct empty state when the search filter matches nothing', async () => {
    const collection = seedCollection();
    seedDocument(collection.id, { title: 'Alpha.pdf' });
    const client = await loggedInClient();
    renderLibrary(client, collection.id);

    await screen.findByRole('button', { name: /Alpha\.pdf/ });
    fireEvent.change(screen.getByLabelText('Tìm tài liệu'), {
      target: { value: 'không tồn tại đâu' },
    });

    expect(
      await screen.findByText('Không tìm thấy tài liệu phù hợp với bộ lọc hiện tại.'),
    ).toBeVisible();
  });

  it('shows an error notice with a working retry when the document list request fails', async () => {
    const collection = seedCollection();
    seedDocument(collection.id, { title: 'Alpha.pdf' });
    const client = await loggedInClient();
    // 503 is not a status `listDocuments` declares in the contract (only
    // 200/404/429) — the mock's own spec-drift guard would reject it, which
    // would surface as a `NetworkError`, not the `HttpApiError` this test
    // wants to exercise. 429 (rate-limited) is a status the operation does
    // declare.
    mockControl.forceStatus('listDocuments', 429, { times: 1 });
    renderLibrary(client, collection.id);

    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('Quá nhiều yêu cầu. Vui lòng thử lại sau ít phút.');
    expect(screen.queryByRole('button', { name: /Alpha\.pdf/ })).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button', { name: 'Thử lại' }));

    await waitFor(() => expect(screen.getByRole('button', { name: /Alpha\.pdf/ })).toBeVisible());
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('shows a state-aware message instead of fetching a preview for a document with no version yet', async () => {
    const collection = seedCollection();
    seedDocument(collection.id, {
      title: 'Processing.pdf',
      state: 'converting',
      currentVersionId: null,
    });
    const client = await loggedInClient();
    renderLibrary(client, collection.id);

    fireEvent.click(await screen.findByRole('button', { name: /Processing\.pdf/ }));

    expect(
      await screen.findByText('Đang xử lý — chưa có phiên bản nào sẵn sàng để xem trước.'),
    ).toBeVisible();
  });

  it('renders the selected document preview through SafeMarkdown, parsing Markdown and sanitizing raw HTML rather than dumping raw text/HTML', async () => {
    const collection = seedCollection();
    // The mock's previewDocument handler embeds the title verbatim in a
    // Markdown heading (`# ${title}\n\n...`) — using a title that itself
    // carries a script-vector payload lets this one test prove two things at
    // once: react-markdown really parses the `#` into a heading (not shown
    // as literal "# " text, which a raw dump would produce), and
    // rehype-sanitize really strips the `onerror` handler (which a raw
    // `dangerouslySetInnerHTML` of the unparsed title would not do).
    const dangerousTitle = 'Special<img src="x" onerror="window.__pwned = true">.pdf';
    seedDocumentWithVersion(collection.id, { title: dangerousTitle });
    const client = await loggedInClient();
    const { container } = renderLibrary(client, collection.id);

    fireEvent.click(await screen.findByRole('button', { name: /Special/ }));

    // Scoped to the rendered-Markdown wrapper specifically: the page also has
    // its own `<h1 id="library-heading">` (the "Bộ sưu tập <id>" heading),
    // which would otherwise be `querySelector('h1')`'s first match.
    await waitFor(() =>
      expect(
        container.querySelector('[data-testid="document-preview-markdown"] h1'),
      ).not.toBeNull(),
    );
    const markdownRoot = container.querySelector('[data-testid="document-preview-markdown"]');
    const heading = markdownRoot?.querySelector('h1');
    expect(heading?.textContent).toContain('Special');
    expect(markdownRoot?.textContent).not.toContain('# Special');

    const img = markdownRoot?.querySelector('img');
    expect(img).not.toBeNull();
    expect(img?.getAttribute('onerror')).toBeNull();
    expect(markdownRoot?.innerHTML).not.toContain('onerror');
    expect((window as unknown as { __pwned?: boolean }).__pwned).toBeUndefined();
  });

  it("renders a plain preview for a normal document, distinct from the panel's own title heading", async () => {
    const client = await loggedInClient();
    const { container } = renderLibrary(client, HANDBOOK_COLLECTION_ID);

    fireEvent.click(await screen.findByRole('button', { name: /Onboarding Guide\.pdf/ }));

    await waitFor(() =>
      expect(
        container.querySelector('[data-testid="document-preview-markdown"] h1'),
      ).not.toBeNull(),
    );
    expect(
      container.querySelector('[data-testid="document-preview-markdown"] h1')?.textContent,
    ).toBe('Onboarding Guide.pdf');
    // The panel's own document-title heading is a separate <h2>, not the
    // rendered Markdown body.
    expect(screen.getByRole('heading', { level: 2, name: 'Onboarding Guide.pdf' })).toBeVisible();
  });

  // The three cases below cover this page's own wiring of the upload panel
  // (P2-08) and the per-document actions (P2-09) — the seams between the
  // three components, which each component's own suite cannot see.
  describe('upload and document-action wiring', () => {
    it('mounts the upload panel scoped to the open collection, and not on the all-collections view', async () => {
      const client = await loggedInClient();
      const { container } = renderLibrary(client, HANDBOOK_COLLECTION_ID);

      await screen.findByRole('button', { name: /Onboarding Guide\.pdf/ });
      expect(container.querySelector('[data-slot="library-upload"]')).not.toBeNull();
      expect(screen.getByLabelText('Chọn tệp để tải lên')).toBeVisible();

      cleanup();
      renderLibrary(client, undefined);

      // `POST /uploads` requires a collectionId, so there is nothing to
      // upload *into* here — the panel must be absent, not disabled.
      await screen.findByText('Chọn một bộ sưu tập ở trên để xem danh sách tài liệu.');
      expect(screen.queryByLabelText('Chọn tệp để tải lên')).not.toBeInTheDocument();
    });

    it('mounts the document actions in the preview panel once a document is selected, keyed so a previous selection’s notice cannot carry over', async () => {
      const collection = seedCollection();
      const first = seedDocumentWithVersion(collection.id, { title: 'Đầu tiên.pdf' });
      seedDocumentWithVersion(collection.id, { title: 'Thứ hai.pdf' });
      const client = await loggedInClient();
      const { container } = renderLibrary(client, collection.id);

      // Nothing selected yet: no actions anywhere.
      await screen.findByRole('button', { name: /Đầu tiên\.pdf/ });
      expect(screen.queryByRole('button', { name: 'Lập chỉ mục lại' })).not.toBeInTheDocument();

      fireEvent.click(screen.getByRole('button', { name: /Đầu tiên\.pdf/ }));
      await waitFor(() =>
        expect(
          container.querySelector(`[data-slot="document-actions:${first.id}"]`),
        ).not.toBeNull(),
      );
      // Exactly one instance — the list no longer carries a per-row copy.
      expect(screen.getAllByRole('button', { name: 'Lập chỉ mục lại' })).toHaveLength(1);

      fireEvent.click(screen.getByRole('button', { name: 'Lập chỉ mục lại' }));
      expect(await screen.findByText('Đã đưa tài liệu vào hàng đợi lập chỉ mục.')).toBeVisible();

      // Switching selection must remount the actions: the success notice
      // belongs to the previous document, not to this one.
      fireEvent.click(screen.getByRole('button', { name: /Thứ hai\.pdf/ }));
      await waitFor(() =>
        expect(
          screen.queryByText('Đã đưa tài liệu vào hàng đợi lập chỉ mục.'),
        ).not.toBeInTheDocument(),
      );
      expect(screen.getByRole('button', { name: 'Lập chỉ mục lại' })).toBeVisible();
    });

    it('discards the retained document list on an org switch instead of showing the previous org’s rows while the new list loads', async () => {
      const collection = seedCollection();
      seedDocument(collection.id, { title: 'Của org cũ.pdf' });
      const client = await loggedInClient();
      const manager = createScopeManager();
      manager.setScope({ orgId: 'org-a', permissions: [], allowedCollectionIds: [collection.id] });
      render(
        <RouterProvider>
          <ScopeProvider manager={manager}>
            <LibraryPage collectionId={collection.id} client={client} />
          </ScopeProvider>
        </RouterProvider>,
      );

      expect(await screen.findByRole('button', { name: /Của org cũ\.pdf/ })).toBeVisible();

      // The epoch moves, so the retained payload's key no longer matches and
      // it must not be served while the new org's list is in flight. This is
      // the guard that keeps the retention above from becoming the
      // cross-tenant leak `useScopeSafeRequest` exists to prevent.
      act(() => {
        manager.setScope({ orgId: 'org-b', permissions: [], allowedCollectionIds: [] });
      });

      expect(screen.queryByRole('button', { name: /Của org cũ\.pdf/ })).not.toBeInTheDocument();
      expect(screen.getByText('Đang tải danh sách tài liệu…')).toBeVisible();
    });

    it('refetches the document list after a delete, rather than hiding the row optimistically', async () => {
      const collection = seedCollection();
      seedDocumentWithVersion(collection.id, { title: 'Sẽ bị xóa.pdf' });
      const client = await loggedInClient();
      renderLibrary(client, collection.id);

      fireEvent.click(await screen.findByRole('button', { name: /Sẽ bị xóa\.pdf/ }));
      fireEvent.click(await screen.findByRole('button', { name: 'Xóa' }));
      fireEvent.click(await screen.findByRole('button', { name: 'Xóa tài liệu' }));

      // The mock deletes server-side; the row goes away because the list was
      // refetched (`onChanged` -> `refreshDocuments`), not because the UI
      // guessed. If the refetch were dropped, the row would still be here.
      await waitFor(() =>
        expect(screen.queryByRole('button', { name: /Sẽ bị xóa\.pdf/ })).not.toBeInTheDocument(),
      );
      expect(screen.getByText('Chưa có tài liệu nào trong bộ sưu tập này.')).toBeVisible();
    });
  });
});
