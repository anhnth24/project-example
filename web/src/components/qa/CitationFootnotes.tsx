// Part C of the owner's Q&A redesign spec: "Citations dạng footnote cuối câu
// trả lời" — a numbered "Nguồn trích dẫn" block at the end of a turn, one
// item per citation (`[n] <tên/nguồn> — trang X`), replacing the old flat
// list of `CitationCard`s. Reuses `CitationCard.tsx`'s deep-link logic
// (`hasDeepLink`/`locationLabel`) rather than re-deriving it — this is a
// restructuring of that same card into a footnote-shaped item, not a parallel
// implementation.
//
// **P2-19 gap closed:** `CitationPin` (`api/generated/contract.ts`) now
// carries a nullable `documentTitle` — `services::citation::pin_from_hit`/
// `resolve_citation` populate it from `documents.title`, which PostgreSQL
// hydration already joins for ACL/state (no extra per-pin lookup; see the
// P2-19 backlog entry's trade-off note). This component prefers
// `citation.documentTitle` when present; a pin that still omits it (older
// stored chat turn, or a future producer that cannot resolve one) falls back
// to `collectionId` resolved against `collectionNameById` — a map the caller
// already has in hand from a single `GET /collections` call made once per
// page, not once per citation — same graceful-degradation fallback as before.
//
// Quotes are collapsed by default so the source list stays scannable; the
// answer body already carries the relevant passages for extractive turns.
import { useState } from 'react';
import { buildLibraryDocPath } from '../../lib/router';
import { RouteLink } from '../RouteLink';
import { footnoteAnchorId } from './AnswerText';
import { hasDeepLink, locationLabel, type CitationPin } from './CitationCard';
import { buildCitationFootnotes, distinctDocumentCount } from './citationFootnoteModel';

const UNKNOWN_COLLECTION_LABEL = 'Bộ sưu tập không xác định';

function FootnoteItem({
  n,
  citation,
  sourceLabel,
  location,
  scopeId,
}: {
  n: number;
  citation: CitationPin;
  sourceLabel: string;
  location: string | null;
  scopeId: string;
}) {
  const [quoteOpen, setQuoteOpen] = useState(false);
  const deepLinkable = hasDeepLink(citation);
  return (
    <li
      id={footnoteAnchorId(scopeId, n)}
      style={{
        padding: 'var(--space-2) 0',
        borderBottom: '1px solid var(--border-subtle, rgba(0,0,0,0.08))',
      }}
    >
      <div
        style={{
          display: 'flex',
          gap: 'var(--space-2)',
          alignItems: 'baseline',
          flexWrap: 'wrap',
        }}
      >
        <strong>[{n}]</strong>
        <span>{sourceLabel}</span>
        {location && <span className="text-muted">— {location}</span>}
        {citation.isCurrent === false && (
          <span className="tag tag-neutral" title="Trích dẫn từ phiên bản không phải bản mới nhất">
            Không phải bản hiện hành
          </span>
        )}
        {deepLinkable && (
          <RouteLink
            className="btn btn-secondary btn-sm"
            to={buildLibraryDocPath(citation.collectionId, citation.logicalDocumentId)}
          >
            Xem trước
          </RouteLink>
        )}
        {citation.quote && (
          <button
            type="button"
            className="btn btn-secondary btn-sm"
            aria-expanded={quoteOpen}
            onClick={() => setQuoteOpen((open) => !open)}
          >
            {quoteOpen ? 'Ẩn đoạn trích' : 'Hiện đoạn trích'}
          </button>
        )}
      </div>
      {quoteOpen && citation.quote && (
        <blockquote
          style={{ margin: 'var(--space-2) 0 0', fontStyle: 'italic' }}
          data-testid="qa-footnote-quote"
        >
          “{citation.quote}”
        </blockquote>
      )}
    </li>
  );
}

export function CitationFootnotes({
  citations,
  collectionNameById,
  scopeId,
}: {
  citations: readonly CitationPin[];
  /** `collectionId -> tên bộ sưu tập`, from whatever `GET /collections` call the page already made — never fetched again per citation (see this file's module doc). */
  collectionNameById: ReadonlyMap<string, string>;
  scopeId: string;
}) {
  if (citations.length === 0) return null;
  const footnotes = buildCitationFootnotes(citations);
  const documentCount = distinctDocumentCount(citations);

  return (
    <div aria-labelledby={`${scopeId}-sources-heading`}>
      <p className="eyebrow" id={`${scopeId}-sources-heading`} style={{ margin: 0 }}>
        Nguồn trích dẫn
      </p>
      {documentCount > 1 && (
        <p className="text-muted" style={{ margin: 'var(--space-1) 0' }}>
          Tổng hợp từ {documentCount} tài liệu.
        </p>
      )}
      <ol
        style={{
          listStyle: 'none',
          margin: 'var(--space-2) 0 0',
          padding: 0,
          display: 'grid',
          gap: 0,
        }}
      >
        {footnotes.map(({ n, citation }) => {
          const collectionName = citation.collectionId
            ? (collectionNameById.get(citation.collectionId) ?? UNKNOWN_COLLECTION_LABEL)
            : UNKNOWN_COLLECTION_LABEL;
          const sourceLabel = citation.documentTitle ?? collectionName;
          const location = locationLabel(citation);
          return (
            <FootnoteItem
              key={citation.citeId + n}
              n={n}
              citation={citation}
              sourceLabel={sourceLabel}
              location={location}
              scopeId={scopeId}
            />
          );
        })}
      </ol>
      {citations.some((citation) => !hasDeepLink(citation)) && (
        <p className="text-muted" style={{ marginTop: 'var(--space-2)' }}>
          Một số trích dẫn ở đây chưa kèm định danh tài liệu/phiên bản — dùng ô Tìm kiếm để mở bản
          xem trước theo tài liệu.
        </p>
      )}
    </div>
  );
}
