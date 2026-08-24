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
  return groupedLocationLabel([citation]);
}

function isFinitePage(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value);
}

/** Compact "1, 2, 5–7" from a sorted unique list. */
export function formatPageRanges(pages: readonly number[]): string {
  const sorted = [...new Set(pages.filter(isFinitePage))].sort((a, b) => a - b);
  if (sorted.length === 0) return '';
  const parts: string[] = [];
  let i = 0;
  while (i < sorted.length) {
    let j = i;
    while (j + 1 < sorted.length && sorted[j + 1] === sorted[j] + 1) {
      j += 1;
    }
    parts.push(i === j ? `${sorted[i]}` : `${sorted[i]}–${sorted[j]}`);
    i = j + 1;
  }
  return parts.join(', ');
}

/** Page/slide/sheet label for one pin or a whole same-document group. `null` page is omitted (never "Trang null"). */
export function groupedLocationLabel(citations: readonly CitationPin[]): string | null {
  const pages = citations.map((citation) => citation.page).filter(isFinitePage);
  if (pages.length > 0) {
    return `Trang ${formatPageRanges(pages)}`;
  }
  const slides = citations.map((citation) => citation.slide).filter(isFinitePage);
  if (slides.length > 0) {
    const ranges = formatPageRanges(slides);
    return `Slide ${ranges}`;
  }
  const sheets = citations
    .map((citation) => citation.sheet)
    .filter((value): value is string => typeof value === 'string' && value.trim().length > 0);
  if (sheets.length > 0) {
    return `Sheet ${[...new Set(sheets)].join(', ')}`;
  }
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
    <li className="chat-citation-card">
      <div className="chat-footnote-row">
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
      <blockquote className="chat-footnote-quote">“{citation.quote}”</blockquote>
    </li>
  );
}
