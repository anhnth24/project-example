# P0-06 retrieval evaluation (scaffold)

- Generated: `2026-07-18T17:44:43.440692+00:00`
- Git commit: `78a75a49eebf191f6a3b9005cc3be07f5613d186`
- Chunking: `heading-chunks-2000-v1`
- Runtime path: `local-hash`
- Index signature: `dedf790eb237c6f562ba2e338fb50038345b4af26c0ef56f17a9b9ca73ca060f`
- RRF tuned: `False`

## Legs (document-level, local-hash scaffold)

| Leg | Recall@5 | Recall@10 | Hit@5 | MRR | nDCG@10 |
|---|---:|---:|---:|---:|---:|
| `lexical` | 0.9804 | 0.9990 | 0.9958 | 0.8921 | 0.8930 |
| `vector_local_hash` | 0.8929 | 0.9636 | 0.9034 | 0.7245 | 0.7713 |
| `hybrid` | 0.9867 | 0.9951 | 0.9916 | 0.8379 | 0.8606 |

## Version citation / temporal

- Citations total: `305`
- Citations with chunkId: `0`
- Version-citation precision/recall: `0.0` / `0.0` (not yet measurable)
- Note: chunkId still null in golden citations; fill after expected-chunks wiring

## Verdict

- P0-06 closed: **NO**

- Expected chunks pinned for heading-chunks-2000-v1 with span resolve.
- Hybrid scaffold uses frozen RRF weights + local-hash vectors.
- Version-citation gates remain red until chunkId is filled into gold.
- Neural embedding hybrid + claim/conflict metrics deferred.
