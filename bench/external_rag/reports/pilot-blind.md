# External Vietnamese document blind-query RAG pilot

- Documents: `50` official public files
- Converted: `50/50`
- Non-empty: `50/50`
- Production chunks: `2170`
- Queries: `100` (`independent-agent-overview-only`)
- Embedding: `AITeamVN/Vietnamese_Embedding@dea33aa1ab33`
- Runtime path: `local-neural`
- Ranking path: `fileconv-knowledge/desktop::service::hybrid_search`

> **Non-gating pilot.** Conversion, chunking, SQLite indexing, neural
> embedding calls, and hybrid ranking use the production Rust path.
> Questions were written by an independent agent given only one topic-level
> overview per document; it saw no titles, identifiers, source text, chunks, or retrieval results.

## Conversion

| Metric | Value |
|---|---:|
| Success rate | 1.0000 |
| Non-empty rate | 1.0000 |
| Median conversion ms | 44488.33 |
| P95 conversion ms | 162500.53 |
| Median Markdown chars | 56256 |

## Production hybrid retrieval

| Scope | N | Recall@5 | Recall@10 | MRR | nDCG@10 |
|---|---:|---:|---:|---:|---:|
| Overall | 100 | 0.9600 | 0.9900 | 0.8910 | 0.9150 |
| `authority` | 8 | 1.0000 | 1.0000 | 0.8438 | 0.8827 |
| `condition` | 17 | 0.9412 | 1.0000 | 0.7976 | 0.8468 |
| `deadline` | 2 | 1.0000 | 1.0000 | 1.0000 | 1.0000 |
| `exception` | 2 | 1.0000 | 1.0000 | 1.0000 | 1.0000 |
| `procedure` | 25 | 1.0000 | 1.0000 | 0.9333 | 0.9505 |
| `responsibility` | 21 | 0.9524 | 1.0000 | 0.9101 | 0.9316 |
| `rights` | 9 | 1.0000 | 1.0000 | 0.9167 | 0.9367 |
| `sanction` | 8 | 0.8750 | 1.0000 | 0.8875 | 0.9111 |
| `scope` | 8 | 0.8750 | 0.8750 | 0.8750 | 0.8750 |

## Observed misses

- `4` queries missed the relevant document in top 5.
- `1` remained absent from top 10.
- Misses are grouped below by independently assigned query intent.

| Query | Relevant rank |
|---|---:|
| `blind-016` | 9 |
| `blind-035` | 10 |
| `blind-041` | 7 |
| `blind-075` | >10 |

## Interpretation limits

- Fifty documents remain a small candidate pool.
- Relevance is document-level over the production chunk ranking.
- Questions are overview-derived rather than written after reading source text.
- Topic-to-document qrels are positional and do not prove that every requested detail is present.
- Topic overviews remain discriminative in a 50-document corpus, so this score is still optimistic.
- Recall measures intended-document retrieval, not correct-chunk evidence or answer quality.
- The corpus is government/legal-document heavy.
- No-answer and answer-grounding quality are not scored.
