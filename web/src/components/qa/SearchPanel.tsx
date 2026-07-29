// Search box + result list + citations + a per-hit preview deep-link.
// `SearchResponse.hits` is `{ additionalProperties: true }[]` in the contract
// (openapi.yaml) — no fixed wire shape — so this file's `toSearchHit` is a
// mock-side *convention* (`mocks/handlers/qa.ts` is the one place that shape
// is actually decided), read defensively here rather than trusted blindly:
// a field this mock happens to add (`documentId`/`versionId`/`collectionId`)
// is what makes the preview deep-link possible at all, but a real server's
// `hits` could omit it, so every consumer below tolerates it being absent.
import { useId, useState, type FormEvent } from 'react';
import { apiClient, type ApiClient } from '../../api/client';
import type { components } from '../../api/generated/contract';
import { useScopeSafeRequest } from '../../hooks/useScopeSafeRequest';
import { Notice } from '../ui';
import { CitationCard, type CitationPin } from './CitationCard';
import { DocumentPreviewPanel } from './DocumentPreviewPanel';

type SearchResponse = components['schemas']['SearchResponse'];

export interface SearchHit {
  citeId?: string;
  documentId?: string;
  collectionId?: string;
  versionId?: string;
  title?: string;
  score?: number;
  snippet?: string;
}

function toSearchHit(raw: Record<string, unknown>): SearchHit {
  return {
    citeId: typeof raw.citeId === 'string' ? raw.citeId : undefined,
    documentId: typeof raw.documentId === 'string' ? raw.documentId : undefined,
    collectionId: typeof raw.collectionId === 'string' ? raw.collectionId : undefined,
    versionId: typeof raw.versionId === 'string' ? raw.versionId : undefined,
    title: typeof raw.title === 'string' ? raw.title : undefined,
    score: typeof raw.score === 'number' ? raw.score : undefined,
    snippet: typeof raw.snippet === 'string' ? raw.snippet : undefined,
  };
}

export function SearchPanel({
  collectionIds,
  client = apiClient,
  onHitsChanged,
}: {
  collectionIds?: string[];
  client?: ApiClient;
  /** Lets `ChatPanel`'s compare/history document picker reuse whichever documents a search already turned up, instead of asking the user to type a UUID. */
  onHitsChanged?: (hits: SearchHit[]) => void;
}) {
  const inputId = useId();
  const [query, setQuery] = useState('');
  const [submitted, setSubmitted] = useState<string | undefined>(undefined);
  const [selected, setSelected] = useState<SearchHit | undefined>(undefined);

  const result = useScopeSafeRequest<SearchResponse | null>(
    async (signal) => {
      if (submitted === undefined) return null;
      return client.request('post', '/search', {
        body: { query: submitted, collectionIds, limit: 10 },
        signal,
      });
    },
    [client, submitted, collectionIds?.join(',')],
  );

  const hits = (result.data?.hits ?? []).map((h) => toSearchHit(h as Record<string, unknown>));
  const citations = (result.data?.citations ?? []) as CitationPin[];

  function submit(event: FormEvent) {
    event.preventDefault();
    setSelected(undefined);
    setSubmitted(query);
  }

  // Adjust-state-while-rendering (same idiom as `LibraryPage.tsx`'s retained
  // list): report the newly-arrived hits to the parent once, the render this
  // response first becomes visible, rather than from an extra effect.
  const [reportedFor, setReportedFor] = useState<unknown>(undefined);
  if (result.data && reportedFor !== result.data) {
    setReportedFor(result.data);
    onHitsChanged?.(hits);
  }

  return (
    <div className="card" aria-labelledby="qa-search-heading">
      <p className="eyebrow">Tìm kiếm</p>
      <h2 id="qa-search-heading">Tìm trong tài liệu đã lập chỉ mục</h2>
      <form
        onSubmit={submit}
        style={{ display: 'flex', gap: 'var(--space-2)', alignItems: 'flex-end', flexWrap: 'wrap' }}
      >
        <div className="field" style={{ flex: '1 1 260px' }}>
          <label htmlFor={inputId}>Từ khóa</label>
          <input
            id={inputId}
            className="input"
            type="search"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            placeholder="Ví dụ: nghỉ phép, ngân sách, lộ trình…"
          />
        </div>
        <button type="submit" className="btn btn-primary" disabled={query.trim() === ''}>
          Tìm kiếm
        </button>
      </form>

      <div aria-live="polite">
        {result.status === 'loading' && submitted !== undefined && (
          <p className="text-muted">Đang tìm kiếm…</p>
        )}
        {result.status === 'error' && <Notice tone="error">Không thể thực hiện tìm kiếm.</Notice>}
        {result.status === 'success' && submitted !== undefined && hits.length === 0 && (
          <Notice tone="info">Không tìm thấy kết quả phù hợp với "{submitted}".</Notice>
        )}
      </div>

      {hits.length > 0 && (
        <div
          style={{
            display: 'grid',
            gridTemplateColumns: 'minmax(0, 1.3fr) minmax(0, 1fr)',
            gap: 'var(--space-4)',
            alignItems: 'start',
            marginTop: 'var(--space-3)',
          }}
        >
          <ul
            style={{
              listStyle: 'none',
              margin: 0,
              padding: 0,
              display: 'grid',
              gap: 'var(--space-2)',
            }}
          >
            {hits.map((hit, i) => (
              <li key={hit.documentId ?? i} className="card" style={{ padding: 'var(--space-3)' }}>
                <div
                  style={{
                    display: 'flex',
                    justifyContent: 'space-between',
                    gap: 'var(--space-2)',
                    flexWrap: 'wrap',
                  }}
                >
                  <strong>{hit.title ?? 'Tài liệu'}</strong>
                  {hit.score !== undefined && (
                    <span className="text-muted">Điểm khớp {hit.score.toFixed(2)}</span>
                  )}
                </div>
                {hit.snippet && <p style={{ margin: 'var(--space-1) 0' }}>{hit.snippet}</p>}
                {hit.documentId && (
                  <button
                    type="button"
                    className="btn btn-secondary btn-sm"
                    onClick={() => setSelected(hit)}
                  >
                    Xem trước
                  </button>
                )}
              </li>
            ))}
          </ul>

          {selected?.documentId ? (
            <DocumentPreviewPanel
              documentId={selected.documentId}
              versionId={selected.versionId}
              client={client}
            />
          ) : (
            <aside className="card">
              <p className="text-muted">Chọn "Xem trước" ở một kết quả để xem nội dung tài liệu.</p>
            </aside>
          )}
        </div>
      )}

      {citations.length > 0 && (
        <div style={{ marginTop: 'var(--space-3)' }}>
          <p className="eyebrow">Trích dẫn</p>
          <ul
            style={{
              listStyle: 'none',
              margin: 0,
              padding: 0,
              display: 'grid',
              gap: 'var(--space-2)',
            }}
          >
            {citations.map((citation) => (
              <CitationCard key={citation.citeId} citation={citation} />
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}
