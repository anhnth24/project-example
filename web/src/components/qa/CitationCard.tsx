// Renders one `CitationPin` (contract.ts) exactly as the wire shape carries
// it. Deliberately does NOT attempt a "go to this document" link: the
// `CitationPin` schema (`crates/server/openapi/openapi.yaml`) has no
// `logicalDocumentId`/`versionId` field — only `resolveCitation`'s *request*
// shape (`ResolveCitationRequest`) requires those, which the caller would
// already need to know before calling it, so there is no contract-given path
// from a bare ask/search citation back to "which document/version". This is
// a verified gap (see the P2-10 report), not a client oversight — search
// *hits* carry a `documentId` (that shape is mock-convention/generic per the
// spec, not the citation itself) and get a working deep-link via
// `DocumentPreviewPanel`; ask's `citations` do not.
import type { components } from '../../api/generated/contract';

export type CitationPin = components['schemas']['CitationPin'];

function locationLabel(citation: CitationPin): string | null {
  if (citation.page !== undefined) return `Trang ${citation.page}`;
  if (citation.slide !== undefined) return `Slide ${citation.slide}`;
  if (citation.sheet !== undefined) return `Sheet ${citation.sheet}`;
  return null;
}

export function CitationCard({ citation }: { citation: CitationPin }) {
  const location = locationLabel(citation);
  return (
    <li className="card" style={{ padding: 'var(--space-3)' }}>
      <div
        style={{ display: 'flex', gap: 'var(--space-2)', alignItems: 'center', flexWrap: 'wrap' }}
      >
        <span className="tag tag-outline">{citation.citeId}</span>
        {citation.isCurrent === false && (
          <span className="tag tag-neutral" title="Trích dẫn từ phiên bản không phải bản mới nhất">
            Không phải bản hiện hành
          </span>
        )}
        {citation.isCurrent === true && <span className="tag tag-accent-2">Bản hiện hành</span>}
        {location && <span className="text-muted">{location}</span>}
      </div>
      <blockquote style={{ margin: 'var(--space-2) 0 0', fontStyle: 'italic' }}>
        “{citation.quote}”
      </blockquote>
    </li>
  );
}
