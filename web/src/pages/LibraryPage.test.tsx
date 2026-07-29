import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
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
    // P2-07: `?doc=` now lives on `window.location.search`
    // (`RouterProvider`'s `searchParams`), which — unlike the `collectionId`
    // prop `renderLibrary` injects directly — is real browser state that
    // persists across tests unless reset. Without this, a test that
    // navigates to `?doc=...` would leak that query string into whichever
    // test runs next.
    window.history.pushState(null, '', '/');
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
    expect(screen.getByRole('heading', { name: 'Employee Handbook' })).toBeVisible();
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
    // its own `<h1 id="library-heading">` (the collection-name heading),
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

  describe('P2-07 URL param (?doc=)', () => {
    it('selecting a document pushes ?doc= onto the URL', async () => {
      const collection = seedCollection();
      const doc = seedDocumentWithVersion(collection.id, { title: 'Selected.pdf' });
      const client = await loggedInClient();
      renderLibrary(client, collection.id);

      fireEvent.click(await screen.findByRole('button', { name: /Selected\.pdf/ }));

      await waitFor(() =>
        expect(window.location.search).toBe(`?doc=${encodeURIComponent(doc.id)}`),
      );
    });

    it('opening the page with ?doc= already on the URL preselects and previews that document (deep-link)', async () => {
      const collection = seedCollection();
      const doc = seedDocumentWithVersion(collection.id, { title: 'DeepLinked.pdf' });
      seedDocumentWithVersion(collection.id, { title: 'Other.pdf' });
      window.history.pushState(null, '', `/library/${collection.id}?doc=${doc.id}`);
      const client = await loggedInClient();
      renderLibrary(client, collection.id);

      // Preselected without any click: the row shows selected and its own
      // preview loads straight away. `getByText` (not `getByRole('heading',
      // …)`) because the mock's preview markdown re-embeds the title as its
      // own `# DeepLinked.pdf` heading — same title text appears twice
      // (this panel's own `<h2 id="library-preview-heading">` and that `<h1>`
      // inside the rendered Markdown), so only the id-scoped one is unambiguous.
      expect(await screen.findByTestId('document-preview-markdown')).toHaveTextContent(
        'Mock preview content for version 1',
      );
      expect(document.getElementById('library-preview-heading')).toHaveTextContent(
        'DeepLinked.pdf',
      );
    });

    it('reload (fresh mount at the same URL) keeps the same document open', async () => {
      const collection = seedCollection();
      const doc = seedDocumentWithVersion(collection.id, { title: 'Reloaded.pdf' });
      const client = await loggedInClient();
      const first = renderLibrary(client, collection.id);

      fireEvent.click(await screen.findByRole('button', { name: /Reloaded\.pdf/ }));
      await waitFor(() => expect(window.location.search).toContain(doc.id));

      // Simulate a reload: unmount without touching `window.location` (a
      // real reload keeps the URL, tears down all component state, and
      // re-parses it from scratch), then mount a brand-new instance.
      first.unmount();
      renderLibrary(client, collection.id);

      expect(await screen.findByTestId('document-preview-markdown')).toHaveTextContent(
        'Mock preview content for version 1',
      );
      expect(document.getElementById('library-preview-heading')).toHaveTextContent('Reloaded.pdf');
    });

    it('back/forward (popstate) moves the selection without a click', async () => {
      const collection = seedCollection();
      const docA = seedDocumentWithVersion(collection.id, { title: 'A.pdf' });
      const docB = seedDocumentWithVersion(collection.id, { title: 'B.pdf' });
      const client = await loggedInClient();
      renderLibrary(client, collection.id);

      fireEvent.click(await screen.findByRole('button', { name: /A\.pdf/ }));
      await waitFor(() => expect(window.location.search).toContain(docA.id));
      fireEvent.click(await screen.findByRole('button', { name: /B\.pdf/ }));
      await waitFor(() => expect(window.location.search).toContain(docB.id));

      // Back: the browser itself moves history; `popstate` is what
      // `RouterProvider` listens for (it never intercepts real back/forward).
      // jsdom dispatches `popstate` asynchronously, so this is a `waitFor`,
      // not an immediate assertion.
      window.history.back();

      await waitFor(() => expect(window.location.search).toContain(docA.id));
      await waitFor(() =>
        expect(document.getElementById('library-preview-heading')).toHaveTextContent('A.pdf'),
      );
    });
  });

  describe('P2-18 projects', () => {
    it('groups collection nav by project, with an unassigned group for collections with no project', async () => {
      const client = await loggedInClient();
      renderLibrary(client, undefined);

      // Seeded: "Nhân sự" -> Employee Handbook; Product Specs unassigned.
      expect(await screen.findByText('Nhân sự')).toBeVisible();
      expect(screen.getByText('Chưa thuộc dự án')).toBeVisible();
      expect(screen.getByRole('link', { name: 'Employee Handbook' })).toBeVisible();
      expect(screen.getByRole('link', { name: 'Product Specs' })).toBeVisible();
    });

    // The create/rename/assign flow itself moved out of this page entirely
    // (P2-18 "Khu Quản trị" move, owner critique 2026-07-29) into its own
    // route — `AdminProjectsPage.test.tsx` covers it, wrapped in a real
    // `AuthProvider` (same reason `AdminMembersPage.test.tsx` does): a bare
    // injected `client` prop, this file's own convention, never populates
    // `AuthContext`, so `useAuth().hasPermission` here always reads `false`.
    it('no longer renders the old ProjectsPanel create/assign form — only collection-nav grouping and a permission-gated shortcut link remain', async () => {
      const client = await loggedInClient();
      renderLibrary(client, undefined);

      await screen.findByText('Nhân sự');
      expect(screen.queryByRole('heading', { name: 'Dự án' })).not.toBeInTheDocument();
      expect(screen.queryByLabelText('Tên dự án mới')).not.toBeInTheDocument();
      expect(screen.queryByRole('button', { name: 'Tạo dự án' })).not.toBeInTheDocument();
      // The shortcut link itself is gated on `doc.upload`, which a bare
      // injected `client` (no `AuthProvider`) always reads as `false` for —
      // see this describe block's own note above.
      expect(screen.queryByRole('link', { name: 'Quản lý dự án' })).not.toBeInTheDocument();
    });
  });

  describe('P2-08 gap close: live document-status polling', () => {
    const RESTORE_VISIBILITY = Object.getOwnPropertyDescriptor(document, 'visibilityState');

    afterEach(() => {
      vi.useRealTimers();
      if (RESTORE_VISIBILITY) {
        Object.defineProperty(document, 'visibilityState', RESTORE_VISIBILITY);
      }
    });

    function setVisibility(state: 'visible' | 'hidden') {
      Object.defineProperty(document, 'visibilityState', { value: state, configurable: true });
      act(() => {
        document.dispatchEvent(new Event('visibilitychange'));
      });
    }

    function documentListCallCount(spy: { mock: { calls: unknown[][] } }): number {
      return spy.mock.calls.filter((call) => call[1] === '/collections/{collectionId}/documents')
        .length;
    }

    it('polls the document list every 5s while a non-terminal document is on the page, and stops once every document is terminal', async () => {
      const collection = seedCollection();
      const doc = seedDocument(collection.id, { title: 'Processing.pdf', state: 'converting' });
      const client = await loggedInClient();
      const requestSpy = vi.spyOn(client, 'request');

      vi.useFakeTimers({ shouldAdvanceTime: true });
      renderLibrary(client, collection.id);

      await screen.findByRole('button', { name: /Processing\.pdf/ });
      const baseline = documentListCallCount(requestSpy);

      // The server (not a user action) finishes the pipeline between polls.
      const stored = getStore()
        .documents.get(collection.id)!
        .find((d) => d.id === doc.id)!;
      stored.state = 'indexed';

      await act(async () => {
        await vi.advanceTimersByTimeAsync(5200);
      });

      expect(await screen.findByText('Đã lập chỉ mục')).toBeVisible();
      expect(documentListCallCount(requestSpy)).toBeGreaterThan(baseline);
      const callsWhileTerminal = documentListCallCount(requestSpy);

      // Every document on the page is now terminal — no more requests, even
      // across several more base intervals.
      await act(async () => {
        await vi.advanceTimersByTimeAsync(20000);
      });
      expect(documentListCallCount(requestSpy)).toBe(callsWhileTerminal);
    });

    it('does not poll while there is no non-terminal document on the page', async () => {
      const collection = seedCollection();
      seedDocument(collection.id, { title: 'AlreadyDone.pdf', state: 'indexed' });
      const client = await loggedInClient();
      const requestSpy = vi.spyOn(client, 'request');

      vi.useFakeTimers({ shouldAdvanceTime: true });
      renderLibrary(client, collection.id);

      await screen.findByRole('button', { name: /AlreadyDone\.pdf/ });
      const baseline = documentListCallCount(requestSpy);

      await act(async () => {
        await vi.advanceTimersByTimeAsync(20000);
      });
      expect(documentListCallCount(requestSpy)).toBe(baseline);
    });

    it('stops polling while the tab is hidden, and resumes once it is visible again', async () => {
      const collection = seedCollection();
      seedDocument(collection.id, { title: 'Hidden.pdf', state: 'converting' });
      const client = await loggedInClient();
      const requestSpy = vi.spyOn(client, 'request');

      vi.useFakeTimers({ shouldAdvanceTime: true });
      renderLibrary(client, collection.id);

      await screen.findByRole('button', { name: /Hidden\.pdf/ });
      const baseline = documentListCallCount(requestSpy);

      setVisibility('hidden');
      await act(async () => {
        await vi.advanceTimersByTimeAsync(20000);
      });
      expect(documentListCallCount(requestSpy)).toBe(baseline);

      setVisibility('visible');
      await act(async () => {
        await vi.advanceTimersByTimeAsync(5200);
      });
      expect(documentListCallCount(requestSpy)).toBeGreaterThan(baseline);
    });

    it('backs off after a poll error instead of retrying at the base 5s cadence, and the next attempt still lands', async () => {
      const collection = seedCollection();
      seedDocument(collection.id, { title: 'Retry.pdf', state: 'converting' });
      const client = await loggedInClient();
      const requestSpy = vi.spyOn(client, 'request');

      vi.useFakeTimers({ shouldAdvanceTime: true });
      renderLibrary(client, collection.id);

      await screen.findByRole('button', { name: /Retry\.pdf/ });
      const baseline = documentListCallCount(requestSpy);

      // The next `listDocuments` call — the first poll tick — comes back 429.
      mockControl.forceStatus('listDocuments', 429, { times: 1 });
      await act(async () => {
        await vi.advanceTimersByTimeAsync(5200);
      });
      await waitFor(() => expect(documentListCallCount(requestSpy)).toBe(baseline + 1));

      // Backed off to stage 1 (15s): a plain 6s more must NOT yet produce a
      // second poll attempt — a still-5s cadence would have.
      await act(async () => {
        await vi.advanceTimersByTimeAsync(6000);
      });
      expect(documentListCallCount(requestSpy)).toBe(baseline + 1);

      // By well past the 15s backoff mark, the next attempt fires (and
      // succeeds, since the forced 429 above only applied once).
      await act(async () => {
        await vi.advanceTimersByTimeAsync(10000);
      });
      await waitFor(() => expect(documentListCallCount(requestSpy)).toBe(baseline + 2));
    });
  });
});
