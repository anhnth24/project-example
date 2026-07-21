# External Vietnamese document RAG pilot

- Documents: `50` official public files
- Converted: `50/50`
- Non-empty: `50/50`
- Production chunks: `2170`
- Queries: `100` metadata-derived
- Embedding: `AITeamVN/Vietnamese_Embedding@dea33aa1ab33`
- Runtime path: `local-neural`
- Ranking path: `fileconv-knowledge/desktop::service::hybrid_search`

> **Non-gating pilot.** Conversion, chunking, SQLite indexing, neural
> embedding calls, and hybrid ranking use the production Rust path. Queries
> remain metadata-derived and do not establish production semantic quality.

## Conversion

| Metric | Value |
|---|---:|
| Success rate | 1.0000 |
| Non-empty rate | 1.0000 |
| Median conversion ms | 43562.90 |
| P95 conversion ms | 158850.87 |
| Median Markdown chars | 56256 |

## Production hybrid retrieval

| Scope | N | Recall@5 | Recall@10 | MRR | nDCG@10 |
|---|---:|---:|---:|---:|---:|
| Overall | 100 | 0.9500 | 0.9600 | 0.8624 | 0.8867 |
| `identifier` | 50 | 0.9000 | 0.9200 | 0.7349 | 0.7808 |
| `official_subject` | 50 | 1.0000 | 1.0000 | 0.9900 | 0.9926 |

## Observed misses

- `5` queries missed the relevant document in top 5.
- `4` remained absent from top 10.
- Every top-5 miss was an identifier query. Numeric document codes are
  split into common tokens, OCR can alter the code, and repeated chunks
  from competing decrees can crowd the fixed chunk-level top-k.

| Query | Relevant rank |
|---|---:|
| `cp-218871-identifier` | >10 |
| `cp-218861-identifier` | 7 |
| `cp-218665-identifier` | >10 |
| `cp-218662-identifier` | >10 |
| `cp-218747-identifier` | >10 |

## Interpretation limits

- Fifty documents remain a small candidate pool.
- Relevance is document-level over the production chunk ranking.
- Metadata-derived queries retain lexical overlap.
- The corpus is government/legal-document heavy.
- No-answer and answer-grounding quality are not scored.
