// Renders a turn's answer text with inline `[n]` footnote markers instead of
// the raw `[CITE-xxxx]` token the server embeds — see `citationFootnoteModel.ts`
// for the pure mapping this is a thin view over. Each marker links (`href`,
// real in-page anchor, not just a visual number) to its matching item in the
// `CitationFootnotes` block this turn also renders, so a screen reader user
// gets the same "jump to source" affordance a sighted user gets by clicking
// the superscript number.
//
// Text segments are split on blank lines (`\n\n`) into paragraphs so extractive
// passages read as a list. Single newlines inside a passage stay in the text
// and render via `white-space: pre-wrap`.
import type { ReactNode } from 'react';
import type { CitationPin } from './CitationCard';
import { splitAnswerIntoFootnoteSegments, type AnswerTextSegment } from './citationFootnoteModel';

export function footnoteAnchorId(scopeId: string, n: number): string {
  return `${scopeId}-cite-${n}`;
}

function renderSegments(
  segments: readonly AnswerTextSegment[],
  scopeId: string,
  keyPrefix: string,
): ReactNode[] {
  return segments.map((segment, i) =>
    segment.kind === 'footnote' ? (
      <sup key={`${keyPrefix}-f-${i}`}>
        <a href={`#${footnoteAnchorId(scopeId, segment.footnoteNumber!)}`}>
          [{segment.footnoteNumber}]
        </a>
      </sup>
    ) : (
      <span key={`${keyPrefix}-t-${i}`}>{segment.text}</span>
    ),
  );
}

/** Split a flat segment list into paragraph groups at blank-line boundaries in text segments. */
function groupIntoParagraphs(segments: readonly AnswerTextSegment[]): AnswerTextSegment[][] {
  const paragraphs: AnswerTextSegment[][] = [];
  let current: AnswerTextSegment[] = [];

  const flush = () => {
    if (current.length > 0) {
      paragraphs.push(current);
      current = [];
    }
  };

  for (const segment of segments) {
    if (segment.kind === 'footnote') {
      current.push(segment);
      continue;
    }
    const text = segment.text ?? '';
    const parts = text.split(/\n{2,}/);
    for (let i = 0; i < parts.length; i++) {
      if (i > 0) flush();
      const part = parts[i];
      if (part.length > 0) {
        // Preserve single newlines inside a paragraph as line breaks via CSS white-space.
        current.push({ kind: 'text', text: part });
      }
    }
  }
  flush();
  return paragraphs.length > 0 ? paragraphs : [segments.slice()];
}

export function AnswerText({
  text,
  citations,
  scopeId,
}: {
  text: string;
  citations: readonly CitationPin[];
  /** Disambiguates footnote anchors when more than one turn/bubble renders on the same page (e.g. a whole chat log) — must be unique per turn. */
  scopeId: string;
}) {
  const segments = splitAnswerIntoFootnoteSegments(text, citations);
  const paragraphs = groupIntoParagraphs(segments);
  return (
    <div data-testid="qa-answer" className="chat-answer">
      {paragraphs.map((paragraph, i) => (
        <p key={i}>{renderSegments(paragraph, scopeId, `p${i}`)}</p>
      ))}
    </div>
  );
}
