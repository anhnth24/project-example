// Renders a turn's answer text with inline `[n]` footnote markers instead of
// the raw `[CITE-xxxx]` token the server embeds — see `citationFootnotes.ts`
// for the pure mapping this is a thin view over. Each marker links (`href`,
// real in-page anchor, not just a visual number) to its matching item in the
// `CitationFootnotes` block this turn also renders, so a screen reader user
// gets the same "jump to source" affordance a sighted user gets by clicking
// the superscript number.
import type { CitationPin } from './CitationCard';
import { splitAnswerIntoFootnoteSegments } from './citationFootnoteUtils';

export function footnoteAnchorId(scopeId: string, n: number): string {
  return `${scopeId}-cite-${n}`;
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
  return (
    <p data-testid="qa-answer" style={{ whiteSpace: 'pre-wrap', margin: 0 }}>
      {segments.map((segment, i) =>
        segment.kind === 'footnote' ? (
          <sup key={i}>
            <a href={`#${footnoteAnchorId(scopeId, segment.footnoteNumber!)}`}>
              [{segment.footnoteNumber}]
            </a>
          </sup>
        ) : (
          <span key={i}>{segment.text}</span>
        ),
      )}
    </p>
  );
}
