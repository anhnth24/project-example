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
// This page is where P2-07's list, P2-08's upload panel and P2-09's
// per-document actions meet. Both of the latter mutate the collection, so
// both feed the same `refreshDocuments()` — a bump of the retry counter the
// documents request already depends on. Nothing here re-derives a document
// from a mutation's response: the list is refetched and the server stays the
// only source of truth for state (`converting` -> `indexed`, tombstoning),
// which also means a delete's own row disappears on the next page load
// rather than being optimistically hidden while the server may still reject.
//
// P2-07 gap close: which document is open lives in the URL's `?doc=`
// query param (`useRouter().searchParams`), not local component state — a
// reload re-parses the same URL and reopens the same document, and
// back/forward through browser history moves the selection because it moves
// through `RouterProvider`'s own `popstate` listener, both for free. This is
// also what makes `CitationCard`'s deep-link (`/library/:collectionId?doc=`)
// work: navigating there is indistinguishable from a user clicking the same
// row themselves. Selecting a document (`selectDocument` below) calls
// `navigate()` so each selection is its own history entry, same as any other
// in-app navigation this router already treats that way (`RouteLink`).
import { useState } from 'react';
import { apiClient, type ApiClient } from '../api/client';
import {
  CollectionNav,
  DocumentFilters,
  DocumentList,
  DocumentPreview,
  Pagination,
  ProjectsPanel,
  describeApiError,
  matchesQuery,
  type Collection,
  type LibraryDocument,
  type PreviewLoadState,
  type StatusFilterValue,
} from '../components/library';
import { DocumentRowActions } from '../components/actions';
import { RouteLink } from '../components/RouteLink';
import { Notice } from '../components/ui';
import { UploadPanel } from '../components/upload';
import { useScopeSafeRequest } from '../hooks/useScopeSafeRequest';
import { buildLibraryDocPath, buildScopedPath } from '../lib/router';
import { useRouter } from '../state/RouterProvider';
import { useScope } from '../state/ScopeProvider';

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
}

function initialView(collectionId: string | undefined): ViewState {
  return {
    collectionId,
    pageIndex: 0,
    cursors: [undefined],
    searchText: '',
    statusFilter: 'all',
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
  const { epoch } = useScope();
  const { searchParams, navigate } = useRouter();
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
  // The heading must never flash a raw collectionId (owner-reported UI gap):
  // while the nav's own `GET /collections` fetch is still in flight there is
  // no name to show yet, so a neutral "Bộ sưu tập" placeholder is used
  // instead of the id — same "loading" -> "loaded" rule `DocumentPreview`
  // follows for a still-loading document. A collectionId that doesn't match
  // any item (not yet loaded, or a stale/deleted deep link) falls back to
  // the same neutral placeholder rather than ever rendering the id itself.
  const activeCollection = collections.find((c) => c.id === collectionId);
  const libraryHeading = !collectionId
    ? 'Tất cả bộ sưu tập'
    : (activeCollection?.name ?? 'Bộ sưu tập');

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

  // A refresh must not blank the list. `useScopeSafeRequest` reports
  // `data: undefined` for the whole of a re-run, so without this the
  // `refreshDocuments()` that follows an upload or a reindex would drop
  // `items` to `[]` for a frame, `selectedDocument` to `null`, and unmount
  // the preview — taking the actions component (and the success notice it
  // had just rendered) with it, then remounting it fresh.
  //
  // Retaining the previous payload is only safe if it can never outlive the
  // request identity it belongs to, so the retained copy is keyed by the
  // scope epoch, the collection and the cursor — the three things that make
  // it a *different* list rather than the same list re-read. An org switch
  // or a collection change therefore discards it rather than showing one
  // tenant's documents while another's load, which is the exact failure
  // `useScopeSafeRequest` exists to prevent; only a `documentsRetry` bump
  // reuses it.
  const documentsKey = `${epoch}|${collectionId ?? ''}|${cursor ?? ''}`;
  const [retainedDocuments, setRetainedDocuments] = useState<{
    key: string;
    data: NonNullable<typeof documentsResult.data>;
  } | null>(null);
  if (documentsResult.data && retainedDocuments?.data !== documentsResult.data) {
    // Adjust-state-while-rendering, the same idiom `useRequestGeneration`
    // and `SelectControl` use. Converges: the next render compares equal.
    setRetainedDocuments({ key: documentsKey, data: documentsResult.data });
  }
  const documentsData =
    documentsResult.data ??
    (retainedDocuments?.key === documentsKey ? retainedDocuments.data : undefined);

  const items: LibraryDocument[] = documentsData?.items ?? [];
  const visibleItems = items.filter(
    (doc) =>
      matchesQuery(doc.title, effectiveView.searchText) &&
      (effectiveView.statusFilter === 'all' || doc.state === effectiveView.statusFilter),
  );

  // Source of truth for "which document is open" is the URL, not local
  // state — see this file's module doc. `selectedDocumentId` may name a
  // document that is not on the currently-fetched page (a citation deep-link
  // or a reload can point at any document in the collection, not just page
  // 1), so a miss against `items` falls back to fetching that one document
  // directly by id rather than silently showing nothing.
  const selectedDocumentId = searchParams.get('doc');
  const documentFromList = items.find((doc) => doc.id === selectedDocumentId) ?? null;
  const directDocumentResult = useScopeSafeRequest(
    async (signal) => {
      if (!selectedDocumentId || documentFromList) return null;
      return client.request('get', '/documents/{documentId}', {
        params: { path: { documentId: selectedDocumentId } },
        signal,
      });
    },
    [client, selectedDocumentId, documentFromList],
  );
  // A directly-fetched fallback is only trusted when it actually belongs to
  // the collection currently open — `GET /documents/{documentId}` is
  // permission-checked (so a cross-collection id in the query string is
  // never a data leak), but a mismatch here would still be the wrong
  // document for this collection's heading/breadcrumb.
  const directDocument =
    directDocumentResult.data && directDocumentResult.data.collectionId === collectionId
      ? directDocumentResult.data
      : null;
  const selectedDocument = documentFromList ?? directDocument;

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
    if (!collectionId) return;
    navigate(buildLibraryDocPath(collectionId, documentId));
  }

  // Re-runs the documents request (and, through the selected document's
  // `currentVersionId`, the preview) without disturbing the current page,
  // filters or selection. Both the upload panel and the row actions call it.
  function refreshDocuments() {
    setDocumentsRetry((n) => n + 1);
  }

  // Turning the page drops whatever `?doc=` is open — the previously
  // selected document belonged to a different page's results, same as
  // before this became a URL param (`selectedDocumentId: null` on the old
  // local `ViewState`). Only navigates when there was a selection to clear,
  // so plain pagination without a preview open doesn't grow history for no
  // reason.
  function clearSelection() {
    if (collectionId && searchParams.get('doc')) {
      navigate(buildScopedPath('library', collectionId));
    }
  }

  function goToPrevPage() {
    if (effectiveView.pageIndex === 0) return;
    clearSelection();
    setView((v) => ({ ...v, pageIndex: v.pageIndex - 1 }));
  }

  function goToNextPage() {
    const page = documentsResult.data?.page;
    const nextCursor = page?.nextCursor;
    if (!page?.hasMore || !nextCursor) return;
    clearSelection();
    setView((v) => {
      const cursors = v.cursors.slice(0, v.pageIndex + 1);
      cursors.push(nextCursor);
      return { ...v, pageIndex: v.pageIndex + 1, cursors };
    });
  }

  const pageInfo = documentsData?.page;

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
      <h1 id="library-heading">{libraryHeading}</h1>
      <p className="lede">
        Duyệt bộ sưu tập, theo dõi trạng thái xử lý và xem trước nội dung Markdown đã chuyển đổi.
      </p>

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

      <ProjectsPanel
        collections={collections}
        client={client}
        onChanged={() => setCollectionsRetry((n) => n + 1)}
      />

      {!collectionId ? (
        <Notice
          tone="info"
          // UX gap (owner backlog): "Tất cả bộ sưu tập" has no upload panel of
          // its own (`POST /uploads` needs a `collectionId` — see the module
          // doc above), so the fastest path to actually uploading something is
          // one click into whichever collection already exists. Navigation
          // only — this is deliberately not a second, cross-collection upload
          // panel bolted onto this screen; see the P2-07 report for why.
          action={
            collections.length > 0 ? (
              <RouteLink
                className="btn btn-secondary btn-sm"
                to={buildScopedPath('library', collections[0].id)}
                // Explicit `aria-label` (generic, no collection name):
                // `CollectionNav` already renders a same-named link for this
                // exact collection, so this button's *visible* text
                // includes the name for sighted users, but its accessible
                // name deliberately does not — otherwise "Employee Handbook"
                // (say) would resolve to two links by accessible name, which
                // is exactly the ambiguity `e2e/support.ts`'s
                // `openEmployeeHandbook` (and several other suites) would
                // trip on.
                aria-label="Mở bộ sưu tập đầu tiên để tải lên"
              >
                Mở {collections[0].name} để tải lên
              </RouteLink>
            ) : undefined
          }
        >
          Chọn một bộ sưu tập ở trên để xem danh sách tài liệu.
        </Notice>
      ) : (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 'var(--space-4)' }}>
          {/* Scoped to the open collection — `POST /uploads` needs a
              collectionId, so there is nothing to render on the "all
              collections" view above. */}
          <div className="card" data-slot="library-upload">
            <UploadPanel
              collectionId={collectionId}
              onUploaded={refreshDocuments}
              client={client}
            />
          </div>

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
                selectedDocumentId={selectedDocumentId}
                onSelect={selectDocument}
                loading={documentsResult.status === 'loading' && documentsData === undefined}
              />
              {documentsData !== undefined && (
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
              actions={
                selectedDocument ? (
                  // Keyed by document id so switching selection remounts the
                  // actions: `useSingleFlightAction` holds per-ticket phase
                  // state, and a success notice ("đã đưa vào hàng đợi") left
                  // over from the previous document would otherwise read as
                  // belonging to the newly selected one.
                  <DocumentRowActions
                    key={selectedDocument.id}
                    document={selectedDocument}
                    onChanged={refreshDocuments}
                    client={client}
                  />
                ) : undefined
              }
            />
          </div>
        </div>
      )}
    </section>
  );
}
