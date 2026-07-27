import { registerOperation } from '../registry';
import { nextRequestId } from '../ids';
import { getStore } from '../fixtures';
import type { components } from '../../api/generated/contract';

type SearchRequest = components['schemas']['SearchRequest'];
type AskRequest = components['schemas']['AskRequest'];
type CitationPin = components['schemas']['CitationPin'];

function sampleCitation(seed: number): CitationPin {
  return {
    citeId: `cite-${seed}`,
    sourceContentSha256: 'a'.repeat(64),
    canonicalMarkdownSha256: 'b'.repeat(64),
    quoteSha256: 'c'.repeat(64),
    chunkIdentitySha256: 'd'.repeat(64),
    quote: 'Employees accrue fifteen days of paid leave per year.',
    sourceSpanStart: 120,
    sourceSpanEnd: 178,
    quoteLocalStart: 0,
    quoteLocalEnd: 58,
    isCurrent: true,
    anchor: '#leave-policy',
  };
}

registerOperation('search', async (ctx) => {
  const body = await ctx.json<SearchRequest>();
  const documents = [...getStore().documents.values()].flat();
  const hits = documents.slice(0, body.limit ?? 10).map((doc, i) => ({
    documentId: doc.id,
    title: doc.title,
    score: Number((1 - i * 0.1).toFixed(2)),
    snippet: `Matched "${body.query}" in ${doc.title}.`,
  }));
  return {
    status: 200,
    body: { hits, citations: hits.map((_, i) => sampleCitation(i)), requestId: nextRequestId() },
  };
});

registerOperation('ask', async (ctx) => {
  const body = await ctx.json<AskRequest>();
  return {
    status: 200,
    body: {
      answer: `Mock grounded answer for: "${body.question}"`,
      mode: body.mode ?? 'current',
      citations: [sampleCitation(0)],
      warnings: [],
      embeddingMode: 'mock',
      requestId: nextRequestId(),
    },
  };
});
