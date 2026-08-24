import { describe, expect, it } from 'vitest';
import type { CitationPin } from './CitationCard';
import {
  buildCitationFootnotes,
  citationsUsedInAnswer,
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

  it('maps every pin of the same document to one footnote number', () => {
    const citations = [
      pin({ citeId: 'CITE-0001', logicalDocumentId: 'doc-a' }),
      pin({ citeId: 'CITE-0002', logicalDocumentId: 'doc-a' }),
      pin({ citeId: 'CITE-0003', logicalDocumentId: 'doc-b' }),
    ];
    const segments = splitAnswerIntoFootnoteSegments(
      'A [CITE-0001] B [CITE-0002] C [CITE-0003]',
      citations,
    );
    expect(segments).toEqual([
      { kind: 'text', text: 'A ' },
      { kind: 'footnote', footnoteNumber: 1 },
      { kind: 'text', text: ' B ' },
      { kind: 'footnote', footnoteNumber: 1 },
      { kind: 'text', text: ' C ' },
      { kind: 'footnote', footnoteNumber: 2 },
    ]);
  });

  it('collapses adjacent same-document [CITE] tokens into one marker', () => {
    const citations = [
      pin({ citeId: 'CITE-0001', logicalDocumentId: 'doc-a' }),
      pin({ citeId: 'CITE-0002', logicalDocumentId: 'doc-a' }),
    ];
    const segments = splitAnswerIntoFootnoteSegments(
      'Luật Điện lực [CITE-0001] [CITE-0002] còn lại.',
      citations,
    );
    expect(segments).toEqual([
      { kind: 'text', text: 'Luật Điện lực ' },
      { kind: 'footnote', footnoteNumber: 1 },
      { kind: 'text', text: ' còn lại.' },
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
  it('emits one footnote per distinct document, in first-seen order', () => {
    const citations = [
      pin({ citeId: 'CITE-0001', logicalDocumentId: 'doc-a', page: 2 }),
      pin({ citeId: 'CITE-0002', logicalDocumentId: 'doc-a', page: 7 }),
      pin({ citeId: 'CITE-0003', logicalDocumentId: 'doc-b', page: 1 }),
    ];
    const footnotes = buildCitationFootnotes(citations);
    expect(footnotes).toHaveLength(2);
    expect(footnotes[0]).toMatchObject({ n: 1, citation: citations[0] });
    expect(footnotes[0].citations).toEqual([citations[0], citations[1]]);
    expect(footnotes[1]).toMatchObject({ n: 2, citation: citations[2] });
    expect(footnotes[1].citations).toEqual([citations[2]]);
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

describe('citationsUsedInAnswer', () => {
  it('drops pins whose citeId never appears in the answer', () => {
    const citations = [
      pin({ citeId: 'CITE-0001', logicalDocumentId: 'thong-tu' }),
      pin({ citeId: 'CITE-0002', logicalDocumentId: 'thong-tu' }),
      pin({ citeId: 'CITE-0008', logicalDocumentId: 'e2e-canary' }),
    ];
    const kept = citationsUsedInAnswer('Thông tư 36 sửa TT 16 [CITE-0001] [CITE-0002]', citations);
    expect(kept.map((c) => c.citeId)).toEqual(['CITE-0001', 'CITE-0002']);
  });

  it('keeps the original list when the answer has no CITE tokens', () => {
    const citations = [pin({ citeId: 'CITE-0001' }), pin({ citeId: 'CITE-0002' })];
    expect(citationsUsedInAnswer('Xin chào', citations)).toEqual(citations);
  });
});
