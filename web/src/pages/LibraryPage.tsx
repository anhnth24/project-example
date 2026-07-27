// P2-07 (plans/markhand-web/phase-2-web-spa.md §P2.4): collection
// navigation, document list with filter + cursor pagination, and a
// sanitized Markdown preview. Every fetch goes through
// `useScopeSafeRequest` (never a raw `useEffect` + `apiClient.request`) so
// an org switch mid-flight discards stale responses instead of leaking
// cross-tenant data — see that hook's module doc for why each guard exists.
//
// Explicitly out of scope here (plan P2.4): filesystem tree, native
// dialogs, local paths, client-side conversion. There is also no
// cross-collection document list endpoint (`GET
// /collections/{collectionId}/documents` always requires a collectionId),
// so "Tất cả bộ sưu tập" (no collectionId) can only offer collection
// navigation, not a merged document list — documented further in the P2-07
// report.
//
// Two mount points intentionally left empty for other in-flight agents:
//   - `<div data-slot="library-upload" />` below the header — the upload
//     entry point (components/upload/**), scoped to `collectionId` when set.
//   - Each document row's last `<td data-slot="document-row-actions:id">`
//     (components/library/DocumentList.tsx) and the preview panel's
//     `<div data-slot="document-actions:id">` (components/library/
//     DocumentPreview.tsx) — per-document actions (components/actions/**).
import { useState } from 'react';
import { apiClient, type ApiClient } from '../api/client';
import {
  CollectionNav,
  DocumentFilters,
  DocumentList,
  DocumentPreview,
  Pagination,
  describeApiError,
  matchesQuery,
  type Collection,
  type LibraryDocument,
  type PreviewLoadState,
  type StatusFilterValue,
} from '../components/library';
import { Notice } from '../components/ui';
import { useScopeSafeRequest } from '../hooks/useScopeSafeRequest';

/** Server default is 50 (clamped [1,100]); 20 keeps a page comfortably scannable. */
const DOCUMENTS_PAGE_SIZE = 20;

interface ViewState {
  /** Which `collectionId` this state was built for — the reset trigger. */
  collectionId: string | undefined;
  pageIndex: number;
  /** cursors[i] is the cursor used to fetch page i; cursors[0] is always undefined (first page). */
  cursors: Array<string | undefined>;
  searchText: string;
  statusFilter: StatusFilterValue;
  selectedDocumentId: string | null;
}

function initialView(collectionId: string | undefined): ViewState {
  return {
    collectionId,
    pageIndex: 0,
    cursors: [undefined],
    searchText: '',
    statusFilter: 'all',
    selectedDocumentId: null,
  };
}

export function LibraryPage({
  collectionId,
  client = apiClient,
}: {
  collectionId?: string;
  /** Injectable for tests; defaults to the app-wide singleton, same convention as `AuthProvider`. */
  client?: ApiClient;
}) {
  const [view, setView] = useState<ViewState>(() => initialView(collectionId));
  // A `collectionId` prop change (collection switch/deep link) re-scopes the
  // whole page — pagination, filters, and selection all belonged to the
  // previous collection. Adjusting state during render (same idiom as
  // `ui.tsx`'s `SelectControl` and `useScopeSafeRequest.ts`'s own
  // `useRequestGeneration`) instead of an effect means the rest of *this*
  // render already uses the reset values — no stale-collection fetch fires
  // first and gets thrown away a tick later.
  let effectiveView = view;
  if (view.collectionId !== collectionId) {
    effectiveView = initialView(collectionId);
    setView(effectiveView);
  }

  const [collectionsRetry, setCollectionsRetry] = useState(0);
  const collectionsResult = useScopeSafeRequest(
    (signal) => client.request('get', '/collections', { signal }),
    [client, collectionsRetry],
  );
  const collections: Collection[] = collectionsResult.data?.items ?? [];

  const cursor = effectiveView.cursors[effectiveView.pageIndex];
  const [documentsRetry, setDocumentsRetry] = useState(0);
  const documentsResult = useScopeSafeRequest(
    async (signal) => {
      if (!collectionId) return null;
      return client.request('get', '/collections/{collectionId}/documents', {
        params: { path: { collectionId }, query: { limit: DOCUMENTS_PAGE_SIZE, cursor } },
        signal,
      });
    },
    [client, collectionId, cursor, documentsRetry],
  );

  const items: LibraryDocument[] = documentsResult.data?.items ?? [];
  const visibleItems = items.filter(
    (doc) =>
      matchesQuery(doc.title, effectiveView.searchText) &&
      (effectiveView.statusFilter === 'all' || doc.state === effectiveView.statusFilter),
  );
  const selectedDocument = items.find((doc) => doc.id === effectiveView.selectedDocumentId) ?? null;

  const previewResult = useScopeSafeRequest(
    async (signal) => {
      if (!selectedDocument?.currentVersionId) return null;
      return client.request('get', '/documents/{documentId}/preview', {
        params: { path: { documentId: selectedDocument.id } },
        signal,
      });
    },
    [client, selectedDocument?.id, selectedDocument?.currentVersionId],
  );

  function selectDocument(documentId: string) {
    setView((v) => ({ ...v, selectedDocumentId: documentId }));
  }

  function goToPrevPage() {
    setView((v) =>
      v.pageIndex === 0 ? v : { ...v, pageIndex: v.pageIndex - 1, selectedDocumentId: null },
    );
  }

  function goToNextPage() {
    setView((v) => {
      const page = documentsResult.data?.page;
      if (!page?.hasMore || !page.nextCursor) return v;
      const cursors = v.cursors.slice(0, v.pageIndex + 1);
      cursors.push(page.nextCursor);
      return { ...v, pageIndex: v.pageIndex + 1, cursors, selectedDocumentId: null };
    });
  }

  const pageInfo = documentsResult.data?.page;

  let previewLoadState: PreviewLoadState | undefined;
  if (selectedDocument) {
    if (!selectedDocument.currentVersionId) previewLoadState = 'no-version';
    else if (previewResult.status === 'loading') previewLoadState = 'loading';
    else if (previewResult.status === 'error') previewLoadState = 'error';
    else previewLoadState = 'success';
  }

  return (
    <section className="page" style={{ maxWidth: 'none' }} aria-labelledby="library-heading">
      <p className="eyebrow">Thư viện</p>
      <h1 id="library-heading">
        {collectionId ? `Bộ sưu tập ${collectionId}` : 'Tất cả bộ sưu tập'}
      </h1>
      <p className="lede">
        Duyệt bộ sưu tập, theo dõi trạng thái xử lý và xem trước nội dung Markdown đã chuyển đổi.
      </p>

      <div data-slot="library-upload" />

      {collectionsResult.status === 'error' && (
        <Notice
          tone="error"
          action={
            <button
              type="button"
              className="btn btn-secondary btn-sm"
              onClick={() => setCollectionsRetry((n) => n + 1)}
            >
              Thử lại
            </button>
          }
        >
          {describeApiError(collectionsResult.error)}
        </Notice>
      )}

      <CollectionNav
        collections={collections}
        activeCollectionId={collectionId}
        loading={collectionsResult.status === 'loading'}
      />

      {!collectionId ? (
        <Notice tone="info">Chọn một bộ sưu tập ở trên để xem danh sách tài liệu.</Notice>
      ) : (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-4)' }}>
          <DocumentFilters
            searchText={effectiveView.searchText}
            onSearchTextChange={(value) => setView((v) => ({ ...v, searchText: value }))}
            statusFilter={effectiveView.statusFilter}
            onStatusFilterChange={(value) => setView((v) => ({ ...v, statusFilter: value }))}
          />

          {documentsResult.status === 'error' && (
            <Notice
              tone="error"
              action={
                <button
                  type="button"
                  className="btn btn-secondary btn-sm"
                  onClick={() => setDocumentsRetry((n) => n + 1)}
                >
                  Thử lại
                </button>
              }
            >
              {describeApiError(documentsResult.error)}
            </Notice>
          )}

          <div
            style={{
              display: 'grid',
              gridTemplateColumns: 'minmax(0, 2fr) minmax(0, 1fr)',
              gap: 'var(--space-4)',
              alignItems: 'start',
            }}
          >
            <div className="card">
              <DocumentList
                items={visibleItems}
                totalOnPage={items.length}
                selectedDocumentId={effectiveView.selectedDocumentId}
                onSelect={selectDocument}
                loading={documentsResult.status === 'loading'}
              />
              {documentsResult.status === 'success' && (
                <Pagination
                  pageNumber={effectiveView.pageIndex + 1}
                  hasMore={pageInfo?.hasMore ?? false}
                  onPrev={goToPrevPage}
                  onNext={goToNextPage}
                />
              )}
            </div>

            <DocumentPreview
              document={selectedDocument}
              loadState={previewLoadState}
              markdown={previewResult.data?.markdown}
              versionNumber={previewResult.data?.versionNumber}
              isCurrent={previewResult.data?.isCurrent}
              serverTruncated={previewResult.data?.truncated}
              errorMessage={
                previewResult.status === 'error' ? describeApiError(previewResult.error) : undefined
              }
            />
          </div>
        </div>
      )}
    </section>
  );
}
