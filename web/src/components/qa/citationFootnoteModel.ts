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
// appears in the answer text, to a small `[n]` footnote number grouped by
// document (first distinct `logicalDocumentId` is [1], later pins of the same
// document reuse that number). Adjacent `[CITE-…]` tokens of the same document
// collapse to one marker so extractive dumps do not read as `[1][2][3]…`.
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
 * Maps each pin to a 1-based footnote number grouped by document: the first
 * distinct document in `citations` is [1], the next distinct document is [2],
 * and every later pin of the same document reuses that number. Pins without
 * `logicalDocumentId` still group by `collectionId`, then by `citeId`.
 */
export function footnoteNumberByCiteId(citations: readonly CitationPin[]): Map<string, number> {
  const numberByGroup = new Map<string, number>();
  const numberByCiteId = new Map<string, number>();
  for (const citation of citations) {
    const group = documentGroupKey(citation);
    let n = numberByGroup.get(group);
    if (n === undefined) {
      n = numberByGroup.size + 1;
      numberByGroup.set(group, n);
    }
    numberByCiteId.set(citation.citeId, n);
  }
  return numberByCiteId;
}

function isWhitespaceOnly(text: string | undefined): boolean {
  return text !== undefined && text.trim().length === 0;
}

/** Drop consecutive `[n]` markers (and whitespace between them) that point at the same document. */
export function collapseAdjacentSameFootnotes(
  segments: readonly AnswerTextSegment[],
): AnswerTextSegment[] {
  const out: AnswerTextSegment[] = [];
  for (const segment of segments) {
    if (segment.kind !== 'footnote') {
      out.push(segment);
      continue;
    }
    while (
      out.length >= 2 &&
      out[out.length - 1]?.kind === 'text' &&
      isWhitespaceOnly(out[out.length - 1]?.text) &&
      out[out.length - 2]?.kind === 'footnote' &&
      out[out.length - 2]?.footnoteNumber === segment.footnoteNumber
    ) {
      out.pop();
    }
    const last = out[out.length - 1];
    if (last?.kind === 'footnote' && last.footnoteNumber === segment.footnoteNumber) {
      continue;
    }
    out.push(segment);
  }
  return out;
}

/** Splits `answer` into text + `[n]` footnote segments, grouped by document. */
export function splitAnswerIntoFootnoteSegments(
  answer: string,
  citations: readonly CitationPin[],
): AnswerTextSegment[] {
  const footnoteNumberById = footnoteNumberByCiteId(citations);
  const segments: AnswerTextSegment[] = [];
  let lastIndex = 0;
  for (const match of answer.matchAll(CITE_TOKEN_PATTERN)) {
    const index = match.index ?? 0;
    if (index > lastIndex) {
      segments.push({ kind: 'text', text: answer.slice(lastIndex, index) });
    }
    const footnoteNumber = footnoteNumberById.get(match[1]);
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
  return collapseAdjacentSameFootnotes(segments);
}

export interface CitationFootnote {
  readonly n: number;
  /** First pin in the group — title, deep-link, current-version badge. */
  readonly citation: CitationPin;
  /** Every pin that collapsed into this footnote (same document). */
  readonly citations: readonly CitationPin[];
}

/**
 * One footnote per distinct document, numbered in first-seen order — the same
 * numbering `splitAnswerIntoFootnoteSegments` uses inline.
 */
export function buildCitationFootnotes(citations: readonly CitationPin[]): CitationFootnote[] {
  const groups = new Map<string, CitationPin[]>();
  for (const citation of citations) {
    const key = documentGroupKey(citation);
    const group = groups.get(key);
    if (group) {
      group.push(citation);
    } else {
      groups.set(key, [citation]);
    }
  }
  return [...groups.values()].map((group, i) => ({
    n: i + 1,
    citation: group[0],
    citations: group,
  }));
}

/** A stable per-citation grouping key for "how many distinct documents": prefers `logicalDocumentId` (the real identity), falls back to `collectionId`, then `citeId` for a pin carrying neither (never `undefined` grouped with `undefined` across unrelated pins). */
function documentGroupKey(citation: CitationPin): string {
  return citation.logicalDocumentId ?? citation.collectionId ?? citation.citeId;
}

/** Count of distinct documents across `citations` — feeds the "Tổng hợp từ N tài liệu" note, shown only when N > 1. */
export function distinctDocumentCount(citations: readonly CitationPin[]): number {
  return new Set(citations.map(documentGroupKey)).size;
}

/** Pins whose `citeId` appears as `[CITE-…]` in `answer`. Retrieval extras that the body never cited (E2E canary, filler chunks) stay out of footnotes. Answers with no CITE tokens keep the original list. */
export function citationsUsedInAnswer(
  answer: string,
  citations: readonly CitationPin[],
): CitationPin[] {
  const used = new Set(
    [...answer.matchAll(CITE_TOKEN_PATTERN)].map((match) => match[1]).filter(Boolean),
  );
  if (used.size === 0) {
    return [...citations];
  }
  return citations.filter((citation) => used.has(citation.citeId));
}
