// Q&A page — owner redesign ("hiện tại phần hỏi đáp đang nhìn lộn xộn quá",
// 2026-07-29). Four parts, all documented at their own point of use:
//
//   A. Chat-first layout: the chat log + composer (`ChatPanel`) is the main
//      column; "Tìm kiếm" (`SearchPanel`) moves into a second tab instead of
//      stacking as its own always-visible block. A collapsible history
//      sidebar (`ChatHistorySidebar`) lists the caller's own saved sessions
//      (`useChatHistory.ts` owns all of that state/plumbing) down the left.
//   B. The multi-project picker (`ProjectPicker`) lives inside `ChatPanel`'s
//      composer; its selection is lifted here (`projectIds`) so `SearchPanel`
//      (a sibling tab, not a sibling block anymore) can scope by the same
//      selection.
//   C. Citation footnotes: `ChatTurnBubble`/`HistoricalTurnBubble` now render
//      `CitationFootnotes` instead of a flat citation-card list — nothing
//      this page does directly, but `collectionNameById` (built once here
//      from the same `GET /collections` call this page already makes for its
//      own heading) is threaded down through `ChatPanel` to every bubble, so
//      a footnote can show a collection name as a fallback — without an
//      extra request per citation — when a pin's `documentTitle` (P2-19 gap
//      closed) is absent, e.g. an older stored turn (see `CitationFootnotes.tsx`'s
//      own doc).
//   D. General cleanup: consistent `.card`/spacing, the old "lịch sử chỉ lưu
//      tạm trong phiên này" disclaimer removed (there is now a real,
//      server-backed history).
import { useState, type KeyboardEvent } from 'react';
import { apiClient, type ApiClient } from '../api/client';
import {
  ChatHistorySidebar,
  ChatPanel,
  SearchPanel,
  useChatHistory,
  type SearchHit,
} from '../components/qa';
import type { Collection } from '../components/library';
import { useScopeSafeRequest } from '../hooks/useScopeSafeRequest';

type Tab = 'chat' | 'search';

const TABS: { id: Tab; label: string }[] = [
  { id: 'chat', label: 'Hỏi đáp' },
  { id: 'search', label: 'Tìm kiếm' },
];

export function QaPage({
  collectionId,
  client = apiClient,
}: {
  collectionId?: string;
  /** Injectable for tests; defaults to the app-wide singleton, same convention as `LibraryPage`. */
  client?: ApiClient;
}) {
  const collectionIds = collectionId ? [collectionId] : undefined;
  const [activeTab, setActiveTab] = useState<Tab>('chat');
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);

  // Fed by `SearchPanel` so `ChatPanel`'s compare/history document picker has
  // real documents to choose from instead of a raw UUID field — see
  // `ChatPanel.tsx`'s module doc.
  const [candidateDocuments, setCandidateDocuments] = useState<SearchHit[]>([]);
  // Part B — the multi-project picker itself lives inside `ChatPanel`'s
  // composer (see that component's `onProjectIdsChange` doc), but its value
  // must also scope `SearchPanel`'s request — this page is the one place both
  // tabs meet, so it's the natural owner of the lifted value.
  const [projectIds, setProjectIds] = useState<string[]>([]);

  const history = useChatHistory(client);

  // Only fetched for the heading's collection name + the footnote block's
  // collection-name fallback (part C) — same "never show the raw id" rule
  // `LibraryPage.tsx` follows for its own heading (owner-reported UI gap).
  // While this is still loading, or for a stale/unknown collectionId, falls
  // back to a neutral placeholder rather than the id.
  const collectionsResult = useScopeSafeRequest(
    (signal) => client.request('get', '/collections', { signal }),
    [client],
  );
  const collections: Collection[] = collectionsResult.data?.items ?? [];
  const collectionNameById = new Map(collections.map((c) => [c.id, c.name]));
  const activeCollection = collections.find((c) => c.id === collectionId);
  const qaHeading = !collectionId
    ? 'Hỏi đáp trên toàn bộ thư viện'
    : `Hỏi đáp trên bộ sưu tập ${activeCollection?.name ?? ''}`.trim();

  function handleTabKeyDown(event: KeyboardEvent) {
    if (event.key !== 'ArrowLeft' && event.key !== 'ArrowRight') return;
    event.preventDefault();
    const index = TABS.findIndex((t) => t.id === activeTab);
    const direction = event.key === 'ArrowRight' ? 1 : -1;
    const next = TABS[(index + direction + TABS.length) % TABS.length];
    setActiveTab(next.id);
  }

  return (
    <section className="page" style={{ maxWidth: 'none' }} aria-labelledby="qa-heading">
      <p className="eyebrow">Hỏi đáp</p>
      <h1 id="qa-heading">{qaHeading}</h1>

      <div
        role="tablist"
        aria-label="Chế độ hỏi đáp"
        style={{ display: 'flex', gap: 'var(--space-2)' }}
        onKeyDown={handleTabKeyDown}
      >
        {TABS.map((tab) => (
          <button
            key={tab.id}
            type="button"
            role="tab"
            id={`qa-tab-${tab.id}`}
            aria-selected={activeTab === tab.id}
            aria-controls={`qa-tabpanel-${tab.id}`}
            tabIndex={activeTab === tab.id ? 0 : -1}
            className={`btn btn-sm ${activeTab === tab.id ? 'btn-primary' : 'btn-secondary'}`}
            onClick={() => setActiveTab(tab.id)}
          >
            {tab.label}
          </button>
        ))}
      </div>

      <div style={{ display: 'flex', gap: 'var(--space-4)', alignItems: 'flex-start' }}>
        {activeTab === 'chat' && (
          <ChatHistorySidebar
            collapsed={sidebarCollapsed}
            onToggleCollapsed={() => setSidebarCollapsed((c) => !c)}
            sessions={history.sessions}
            sessionsStatus={history.sessionsStatus}
            hasMoreSessions={history.hasMoreSessions}
            onLoadMore={history.loadMoreSessions}
            activeSessionId={history.activeSessionId}
            onSelectSession={history.selectSession}
            onNewConversation={history.startNewConversation}
            onRename={history.renameSession}
            renamingSessionId={history.renamingSessionId}
            renameError={history.renameError}
            onDelete={history.deleteSession}
            deletingSessionId={history.deletingSessionId}
            deleteError={history.deleteError}
          />
        )}

        <div style={{ flex: 1, minWidth: 0, display: 'grid', gap: 'var(--space-3)' }}>
          {history.appendError && (
            <p className="notice notice-info" role="status">
              {history.appendError}{' '}
              <button
                type="button"
                className="btn btn-ghost btn-sm"
                onClick={history.dismissAppendError}
              >
                Đóng
              </button>
            </p>
          )}

          {/* Both tabpanels stay mounted across a tab switch (only `hidden`
              toggles) — deliberately, so neither an in-progress question in
              `ChatPanel`'s composer nor `SearchPanel`'s last result set is
              lost just from glancing at the other tab. */}
          <div
            id="qa-tabpanel-chat"
            role="tabpanel"
            aria-labelledby="qa-tab-chat"
            hidden={activeTab !== 'chat'}
          >
            <ChatPanel
              collectionIds={collectionIds}
              client={client}
              candidateDocuments={candidateDocuments}
              onProjectIdsChange={setProjectIds}
              activeSessionId={history.activeSessionId}
              historicalTurns={history.historicalTurns}
              historicalStatus={history.historicalStatus}
              sessionSwitchToken={history.sessionSwitchToken}
              collectionNameById={collectionNameById}
              onTurnSettled={history.recordTurn}
            />
          </div>

          <div
            id="qa-tabpanel-search"
            role="tabpanel"
            aria-labelledby="qa-tab-search"
            hidden={activeTab !== 'search'}
          >
            <SearchPanel
              collectionIds={collectionIds}
              projectIds={projectIds}
              client={client}
              onHitsChanged={setCandidateDocuments}
            />
          </div>
        </div>
      </div>
    </section>
  );
}
