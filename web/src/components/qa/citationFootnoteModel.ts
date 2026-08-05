// Footnote-style citation rendering (owner spec, part C — "Citations dạng
// footnote cuối câu trả lời"). Pure, framework-free logic so the
// answer-token-to-footnote-number mapping is unit-testable without mounting
// any component.
//
// Wire format verified in the mock/server, not assumed: the answer text a
// grounded turn carries embeds the citation's own `citeId` inline as
// `[CITE-0001]` (`crates/knowledge/src/citation.rs`'s `validate_answer_citations`
// tests + `mocks/handlers/qa.ts`'s `buildAnswer`) — the same token this whole
// app already renders verbatim as a `<span class="tag">CITE-0001</span>` per
// citation card. This module maps each such token, in the order it actually
// appears in the answer text, to a small `[n]` footnote number derived from
// `citations`' own array order (`citations[i]` -> footnote `i + 1`) — the
// same order `ask.citations`/`ChatTurn.citations` already deliver them in,
// never re-sorted here.
//
// A token whose `CITE-xxxx` id has no matching entry in `citations` (should
// not happen for a well-formed turn, but a stored/historical turn's citations
// are never re-validated on read — see `AppendChatTurnRequest`'s own doc) is
// left as its original literal text rather than silently dropped or crashing
// — "never invent, never lose content" applies here as much as anywhere else
// in this codebase.
import type { CitationPin } from './CitationCard';

const CITE_TOKEN_PATTERN = /\[(CITE-[A-Za-z0-9]+)\]/g;

export interface AnswerTextSegment {
  readonly kind: 'text' | 'footnote';
  /** Present when `kind === 'text'`. */
  readonly text?: string;
  /** Present when `kind === 'footnote'` — the `[n]` number to render. */
  readonly footnoteNumber?: number;
}

/**
 * Splits `answer` into plain-text segments and footnote-marker segments,
 * mapping each `[CITE-xxxx]` token to the 1-based position of the matching
 * entry in `citations`. Concatenating every segment's `text` (substituting
 * `[n]` for each footnote segment) reproduces `answer` exactly — no content
 * is added or removed, only re-labelled.
 */
export function splitAnswerIntoFootnoteSegments(
  answer: string,
  citations: readonly CitationPin[],
): AnswerTextSegment[] {
  const footnoteNumberByCiteId = new Map(citations.map((citation, i) => [citation.citeId, i + 1]));
  const segments: AnswerTextSegment[] = [];
  let lastIndex = 0;
  for (const match of answer.matchAll(CITE_TOKEN_PATTERN)) {
    const index = match.index ?? 0;
    if (index > lastIndex) {
      segments.push({ kind: 'text', text: answer.slice(lastIndex, index) });
    }
    const footnoteNumber = footnoteNumberByCiteId.get(match[1]);
    if (footnoteNumber !== undefined) {
      segments.push({ kind: 'footnote', footnoteNumber });
    } else {
      segments.push({ kind: 'text', text: match[0] });
    }
    lastIndex = index + match[0].length;
  }
  if (lastIndex < answer.length) {
    segments.push({ kind: 'text', text: answer.slice(lastIndex) });
  }
  return segments;
}

export interface CitationFootnote {
  readonly n: number;
  readonly citation: CitationPin;
}

/** `citations[i]` -> footnote `{ n: i + 1, citation }`, the same numbering `splitAnswerIntoFootnoteSegments` uses inline. */
export function buildCitationFootnotes(citations: readonly CitationPin[]): CitationFootnote[] {
  return citations.map((citation, i) => ({ n: i + 1, citation }));
}

/** A stable per-citation grouping key for "how many distinct documents": prefers `logicalDocumentId` (the real identity), falls back to `collectionId`, then `citeId` for a pin carrying neither (never `undefined` grouped with `undefined` across unrelated pins). */
function documentGroupKey(citation: CitationPin): string {
  return citation.logicalDocumentId ?? citation.collectionId ?? citation.citeId;
}

/** Count of distinct documents across `citations` — feeds the "Tổng hợp từ N tài liệu" note, shown only when N > 1. */
export function distinctDocumentCount(citations: readonly CitationPin[]): number {
  return new Set(citations.map(documentGroupKey)).size;
}
