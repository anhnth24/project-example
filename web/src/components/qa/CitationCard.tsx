// Renders one `CitationPin` (contract.ts). P2-10 gap close: the server
// (`services::citation::CitationPin`) has always carried
// `logicalDocumentId`/`versionId`/`collectionId` — the OpenAPI schema simply
// didn't declare them (see `openapi.yaml`'s `CitationPin` doc comment), so
// the generated contract type dropped them and this card had no way to link
// back to "which document/version". Now that the schema declares them
// (nullable — a resolved-but-un-hydrated pin, or a future producer that still
// omits them, must keep degrading gracefully rather than throwing), a
// citation that carries a `collectionId` gets a real
// `/library/:collectionId?doc=:documentId` deep-link (`buildLibraryDocPath`,
// same path `LibraryPage` itself reads); one that doesn't (the previous,
// verified contract gap) still falls back to the old explanatory note rather
// than a silently-dead link.
import type { components } from '../../api/generated/contract';
import { buildLibraryDocPath } from '../../lib/router';
import { RouteLink } from '../RouteLink';

export type CitationPin = components['schemas']['CitationPin'];

/** Exported for `CitationFootnotes.tsx` — same "page/slide/sheet, whichever is present" label a footnote item shows, kept in one place rather than duplicated. */
export function locationLabel(citation: CitationPin): string | null {
  // `!= null` (not `!== undefined`): the real API serializes an absent
  // location as an explicit JSON `null`, which rendered as "Trang null".
  if (citation.page != null) return `Trang ${citation.page}`;
  if (citation.slide != null) return `Slide ${citation.slide}`;
  if (citation.sheet != null) return `Sheet ${citation.sheet}`;
  return null;
}

/** Whether this pin carries enough identity to build a working preview deep-link. */
export function hasDeepLink(
  citation: CitationPin,
): citation is CitationPin & { collectionId: string; logicalDocumentId: string } {
  return Boolean(citation.collectionId && citation.logicalDocumentId);
}

export function CitationCard({ citation }: { citation: CitationPin }) {
  const location = locationLabel(citation);
  const deepLinkable = hasDeepLink(citation);
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
        {deepLinkable && (
          <RouteLink
            className="btn btn-secondary btn-sm"
            to={buildLibraryDocPath(citation.collectionId, citation.logicalDocumentId)}
          >
            Xem trước tài liệu
          </RouteLink>
        )}
      </div>
      <blockquote style={{ margin: 'var(--space-2) 0 0', fontStyle: 'italic' }}>
        “{citation.quote}”
      </blockquote>
    </li>
  );
}
