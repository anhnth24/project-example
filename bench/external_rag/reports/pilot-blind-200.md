# External Vietnamese document blind-query RAG pilot

- Documents: `200` official public files
- Converted: `200/200`
- Non-empty: `200/200`
- Reused converted Markdown: `59`
- Production chunks: `8651`
- Queries: `100` (`independent-agent-overview-only`)
- Embedding: `AITeamVN/Vietnamese_Embedding@dea33aa1ab33`
- Runtime path: `local-neural`
- Ranking path: `fileconv-knowledge/desktop::service::hybrid_search`

> **Non-gating pilot.** Conversion, chunking, SQLite indexing, neural
> embedding calls, and hybrid ranking use the production Rust path.
> Questions were written by an independent agent given only one topic-level
> overview per document; it saw no titles, identifiers, source text, chunks, or retrieval results.
> The first 50 documents are query targets; the remaining 150 are chronological distractors.

## Conversion

| Metric | Value |
|---|---:|
| Success rate | 1.0000 |
| Non-empty rate | 1.0000 |
| Median conversion ms | 120289.91 |
| P95 conversion ms | 545277.67 |
| Median Markdown chars | 50219 |

## Production hybrid retrieval

| Scope | N | Recall@5 | Recall@10 | MRR | nDCG@10 |
|---|---:|---:|---:|---:|---:|
| Overall | 100 | 0.9500 | 0.9800 | 0.8612 | 0.8900 |
| `authority` | 8 | 1.0000 | 1.0000 | 0.8750 | 0.9077 |
| `condition` | 17 | 0.8824 | 0.9412 | 0.7765 | 0.8161 |
| `deadline` | 2 | 1.0000 | 1.0000 | 1.0000 | 1.0000 |
| `exception` | 2 | 1.0000 | 1.0000 | 1.0000 | 1.0000 |
| `procedure` | 25 | 1.0000 | 1.0000 | 0.8813 | 0.9112 |
| `responsibility` | 21 | 1.0000 | 1.0000 | 0.8643 | 0.8969 |
| `rights` | 9 | 0.8889 | 1.0000 | 0.9028 | 0.9239 |
| `sanction` | 8 | 0.8750 | 1.0000 | 0.8889 | 0.9126 |
| `scope` | 8 | 0.8750 | 0.8750 | 0.8125 | 0.8289 |

## Controlled 50 → 200 comparison

| Corpus | Chunks | Recall@5 | Recall@10 | MRR | nDCG@10 |
|---|---:|---:|---:|---:|---:|
| 50 targets | 2,170 | 0.9600 | 0.9900 | 0.8910 | 0.9150 |
| 50 targets + 150 distractors | 8,651 | 0.9500 | 0.9800 | 0.8612 | 0.8900 |
| Delta | +6,481 | -0.0100 | -0.0100 | -0.0298 | -0.0250 |

## Observed misses

- `5` queries missed the relevant document in top 5.
- `2` remained absent from top 10.
- Misses are grouped below by independently assigned query intent.

| Query | Relevant rank |
|---|---:|
| `blind-035` | 9 |
| `blind-041` | 6 |
| `blind-075` | >10 |
| `blind-088` | >10 |
| `blind-094` | 8 |

- `blind-088` was crowded out by a new driver-training and licensing document
  with stronger lexical matches for “điều kiện” and “giấy phép lái xe”.
- `blind-094` was crowded by the broader recruitment and management document
  for public employees, whose contract-benefit chunks matched the query closely.

## Interpretation limits

- The pool contains 200 documents, but all targets remain in the first 50.
- Relevance is document-level over the production chunk ranking.
- Questions are overview-derived rather than written after reading source text.
- Topic-to-document qrels are positional and do not prove that every requested detail is present.
- Topic overviews remain discriminative in a 200-document corpus, so this score is still optimistic.
- Recall measures intended-document retrieval, not correct-chunk evidence or answer quality.
- The corpus is government/legal-document heavy.
- No-answer and answer-grounding quality are not scored.
