import { describe, expect, it } from 'vitest';
import type { CitationPin } from './CitationCard';
import {
  buildCitationFootnotes,
  distinctDocumentCount,
  splitAnswerIntoFootnoteSegments,
} from './citationFootnoteModel';

function pin(overrides: Partial<CitationPin> & { citeId: string }): CitationPin {
  return {
    sourceContentSha256: 'src',
    canonicalMarkdownSha256: 'md',
    quoteSha256: 'quote',
    chunkIdentitySha256: 'chunk',
    quote: 'quote text',
    sourceSpanStart: 0,
    sourceSpanEnd: 10,
    quoteLocalStart: 0,
    quoteLocalEnd: 10,
    ...overrides,
  };
}

describe('splitAnswerIntoFootnoteSegments', () => {
  it('maps each [CITE-xxxx] token to its 1-based position in citations, in appearance order', () => {
    const citations = [pin({ citeId: 'CITE-0001' }), pin({ citeId: 'CITE-0002' })];
    const segments = splitAnswerIntoFootnoteSegments('A [CITE-0001] B [CITE-0002] C', citations);
    expect(segments).toEqual([
      { kind: 'text', text: 'A ' },
      { kind: 'footnote', footnoteNumber: 1 },
      { kind: 'text', text: ' B ' },
      { kind: 'footnote', footnoteNumber: 2 },
      { kind: 'text', text: ' C' },
    ]);
  });

  it('numbers by citations array order, not by first-appearance order in the text', () => {
    // citeId "CITE-0002" is listed FIRST in `citations` (so it is footnote
    // [1]) even though it appears SECOND in the answer text.
    const citations = [pin({ citeId: 'CITE-0002' }), pin({ citeId: 'CITE-0001' })];
    const segments = splitAnswerIntoFootnoteSegments('[CITE-0001] then [CITE-0002]', citations);
    expect(segments).toEqual([
      { kind: 'footnote', footnoteNumber: 2 },
      { kind: 'text', text: ' then ' },
      { kind: 'footnote', footnoteNumber: 1 },
    ]);
  });

  it('leaves a token with no matching citation as literal text (never drops content)', () => {
    const citations = [pin({ citeId: 'CITE-0001' })];
    const segments = splitAnswerIntoFootnoteSegments(
      'Kinh phí là 10 triệu [CITE-9999].',
      citations,
    );
    expect(segments).toEqual([
      { kind: 'text', text: 'Kinh phí là 10 triệu ' },
      { kind: 'text', text: '[CITE-9999]' },
      { kind: 'text', text: '.' },
    ]);
  });

  it('returns the whole answer as one text segment when there are no citations at all', () => {
    const segments = splitAnswerIntoFootnoteSegments('Không có trích dẫn nào.', []);
    expect(segments).toEqual([{ kind: 'text', text: 'Không có trích dẫn nào.' }]);
  });

  it('concatenating every segment reproduces the original answer exactly', () => {
    const citations = [pin({ citeId: 'CITE-0001' }), pin({ citeId: 'CITE-0002' })];
    const answer = 'Đầu [CITE-0001] giữa [CITE-0002] cuối.';
    const segments = splitAnswerIntoFootnoteSegments(answer, citations);
    const reconstructed = segments
      .map((s) => (s.kind === 'footnote' ? `[${s.footnoteNumber}]` : s.text))
      .join('');
    // Re-labelled (CITE-xxxx -> [n]), same length/shape otherwise.
    expect(reconstructed).toBe('Đầu [1] giữa [2] cuối.');
  });
});

describe('buildCitationFootnotes', () => {
  it('numbers citations 1-based, in their given array order', () => {
    const citations = [pin({ citeId: 'CITE-0001' }), pin({ citeId: 'CITE-0002' })];
    expect(buildCitationFootnotes(citations)).toEqual([
      { n: 1, citation: citations[0] },
      { n: 2, citation: citations[1] },
    ]);
  });

  it('returns an empty list for no citations', () => {
    expect(buildCitationFootnotes([])).toEqual([]);
  });
});

describe('distinctDocumentCount', () => {
  it('counts one document when every citation shares the same logicalDocumentId', () => {
    const citations = [
      pin({ citeId: 'CITE-0001', logicalDocumentId: 'doc-1' }),
      pin({ citeId: 'CITE-0002', logicalDocumentId: 'doc-1' }),
    ];
    expect(distinctDocumentCount(citations)).toBe(1);
  });

  it('counts each distinct logicalDocumentId separately', () => {
    const citations = [
      pin({ citeId: 'CITE-0001', logicalDocumentId: 'doc-1' }),
      pin({ citeId: 'CITE-0002', logicalDocumentId: 'doc-2' }),
    ];
    expect(distinctDocumentCount(citations)).toBe(2);
  });

  it('falls back to collectionId, then citeId, for a pin missing logicalDocumentId', () => {
    const citations = [
      pin({ citeId: 'CITE-0001', collectionId: 'col-1' }),
      pin({ citeId: 'CITE-0002' }),
    ];
    expect(distinctDocumentCount(citations)).toBe(2);
  });
});
