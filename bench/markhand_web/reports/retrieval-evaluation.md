# P0-06 retrieval evaluation

- Generated: `2026-07-18T18:19:02.349189+00:00`
- Git commit: `b6db271ca69f80854442da9330173b0967c05186`
- Dirty: `False`
- Chunking: `heading-chunks-2000-v1`
- Embedding runtime: `sentence-transformers:AITeamVN/Vietnamese_Embedding@dea33aa1ab33`
- Runtime path: `provider-cloud`
- Index signature: `b5e58653f6666837e3da8f2fb25f7ae36bee6bc96cab9f282ad791c9e40061cb`
- RRF vectorWeight: `0.65` (tuned=True)

## Legs (document-level)

| Leg | Recall@5 | Recall@10 | Hit@5 | MRR | nDCG@10 |
|---|---:|---:|---:|---:|---:|
| `lexical` | 0.9804 | 0.9990 | 0.9958 | 0.8921 | 0.8930 |
| `vector_neural` | 0.9261 | 0.9608 | 0.9538 | 0.8067 | 0.7992 |
| `hybrid` | 0.9877 | 1.0000 | 0.9916 | 0.9209 | 0.9052 |

## Version citation / temporal / conflict

- Citations with chunkId: `305/305`
- Version-citation P/R: `1.0` / `1.0`
- Temporal accuracy: `1.0` (n=6)
- Change accuracy: `1.0` (n=4)
- Conflict status accuracy: `1.0` (n=8)
- Unresolved warning accuracy: `1.0`
- Resolved history accuracy: `1.0`
- Claim conflict P/R: `1.0` / `1.0`

## Gates

- `G0-RET-RECALL-AT-5`: metric=0.987745 threshold=0.85 pass=True evaluated=True
- `G0-RET-TEMPORAL-ACCURACY`: metric=1.0 threshold=0.95 pass=True evaluated=True
- `G0-RET-CHANGE-ACCURACY`: metric=1.0 threshold=0.95 pass=True evaluated=True
- `G0-RET-VERSION-CITATION-PRECISION`: metric=1.0 threshold=1.0 pass=True evaluated=True
- `G0-RET-VERSION-CITATION-RECALL`: metric=1.0 threshold=1.0 pass=True evaluated=True

## Verdict

- P0-06 closed: **YES**

- All P0-06 retrieval/version/conflict gates passed with neural hybrid.
