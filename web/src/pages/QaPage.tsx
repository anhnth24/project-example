// P2-10: search + streamed ask, built directly on the OpenAPI contract +
// mock server (owner gate-down, 2026-07-29 — see
// `plans/markhand-web/backlog/phase-2/issues/README.md`'s P2-10 entry). Was a
// placeholder before this; every field/endpoint used below is one the
// contract (`web/src/api/generated/contract.ts`) actually declares — see
// `components/qa/**`'s own module docs for the one verified gap (citations
// from `ask`/`ask/stream` carry no document/version id to deep-link with).
import { useState } from 'react';
import { apiClient, type ApiClient } from '../api/client';
import { AskPanel, SearchPanel, type SearchHit } from '../components/qa';

export function QaPage({
  collectionId,
  client = apiClient,
}: {
  collectionId?: string;
  /** Injectable for tests; defaults to the app-wide singleton, same convention as `LibraryPage`. */
  client?: ApiClient;
}) {
  const collectionIds = collectionId ? [collectionId] : undefined;
  // Fed by `SearchPanel` so `AskPanel`'s compare/history document picker has
  // real documents to choose from instead of a raw UUID field — see
  // `AskPanel.tsx`'s module doc.
  const [candidateDocuments, setCandidateDocuments] = useState<SearchHit[]>([]);

  return (
    <section className="page" style={{ maxWidth: 'none' }} aria-labelledby="qa-heading">
      <p className="eyebrow">Hỏi đáp</p>
      <h1 id="qa-heading">
        {collectionId ? `Hỏi đáp trên bộ sưu tập ${collectionId}` : 'Hỏi đáp trên toàn bộ thư viện'}
      </h1>
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
        <AskPanel
          collectionIds={collectionIds}
          client={client}
          candidateDocuments={candidateDocuments}
        />
      </div>
    </section>
  );
}
