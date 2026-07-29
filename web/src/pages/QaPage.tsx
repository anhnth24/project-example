// P2-10: search + streamed ask, built directly on the OpenAPI contract +
// mock server (owner gate-down, 2026-07-29 — see
// `plans/markhand-web/backlog/phase-2/issues/README.md`'s P2-10 entry). Was a
// placeholder before this; every field/endpoint used below is one the
// contract (`web/src/api/generated/contract.ts`) actually declares — see
// `components/qa/**`'s own module docs for the one verified gap (citations
// from `ask`/`ask/stream` carry no document/version id to deep-link with).
import { useState } from 'react';
import { apiClient, type ApiClient } from '../api/client';
import { ChatPanel, SearchPanel, type SearchHit } from '../components/qa';
import type { Collection } from '../components/library';
import { useScopeSafeRequest } from '../hooks/useScopeSafeRequest';

export function QaPage({
  collectionId,
  client = apiClient,
}: {
  collectionId?: string;
  /** Injectable for tests; defaults to the app-wide singleton, same convention as `LibraryPage`. */
  client?: ApiClient;
}) {
  const collectionIds = collectionId ? [collectionId] : undefined;
  // Fed by `SearchPanel` so `ChatPanel`'s compare/history document picker has
  // real documents to choose from instead of a raw UUID field — see
  // `ChatPanel.tsx`'s module doc.
  const [candidateDocuments, setCandidateDocuments] = useState<SearchHit[]>([]);

  // Only fetched for the heading's collection name — same "never show the
  // raw id" rule `LibraryPage.tsx` follows for its own heading (owner-
  // reported UI gap). While this is still loading, or for a stale/unknown
  // collectionId, falls back to a neutral placeholder rather than the id.
  const collectionsResult = useScopeSafeRequest(
    (signal) => client.request('get', '/collections', { signal }),
    [client],
  );
  const collections: Collection[] = collectionsResult.data?.items ?? [];
  const activeCollection = collections.find((c) => c.id === collectionId);
  const qaHeading = !collectionId
    ? 'Hỏi đáp trên toàn bộ thư viện'
    : `Hỏi đáp trên bộ sưu tập ${activeCollection?.name ?? ''}`.trim();

  return (
    <section className="page" style={{ maxWidth: 'none' }} aria-labelledby="qa-heading">
      <p className="eyebrow">Hỏi đáp</p>
      <h1 id="qa-heading">{qaHeading}</h1>
      <p className="lede">
        Tìm kiếm tài liệu đã lập chỉ mục hoặc đặt câu hỏi để nhận câu trả lời có trích dẫn, phát dần
        theo thời gian thực.
      </p>

      <div style={{ display: 'grid', gap: 'var(--space-4)' }}>
        <SearchPanel
          collectionIds={collectionIds}
          client={client}
          onHitsChanged={setCandidateDocuments}
        />
        <ChatPanel
          collectionIds={collectionIds}
          client={client}
          candidateDocuments={candidateDocuments}
        />
      </div>
    </section>
  );
}
