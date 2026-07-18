# P0-06 retrieval evaluation

- Generated: `2026-07-18T18:54:00.334597+00:00`
- Git commit: `31e24f1d62ed8e48b19b1bed8ec84b582fb89148`
- Dirty: `False`
- Chunking: `heading-chunks-2000-v1`
- Embedding runtime: `sentence-transformers:AITeamVN/Vietnamese_Embedding@dea33aa1ab33`
- Runtime path: `local-neural`
- Index signature: `d54db7b6de20b51a416670927eeab346256c9b891732965e51586fac333c1835`
- RRF vectorWeight: `0.55` (tuned=False)

## Legs (document-level)

| Leg | Recall@5 | Recall@10 | Hit@5 | MRR | nDCG@10 |
|---|---:|---:|---:|---:|---:|
| `lexical` | 0.9902 | 0.9990 | 1.0000 | 0.8718 | 0.8829 |
| `vector_neural` | 0.9261 | 0.9608 | 0.9538 | 0.8067 | 0.7992 |
| `hybrid` | 0.9849 | 1.0000 | 0.9916 | 0.9198 | 0.9079 |

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

- `G0-RET-RECALL-AT-5`: metric=0.984944 threshold=0.85 pass=True evaluated=True
- `G0-RET-TEMPORAL-ACCURACY`: metric=1.0 threshold=0.95 pass=True evaluated=True
- `G0-RET-CHANGE-ACCURACY`: metric=1.0 threshold=0.95 pass=True evaluated=True
- `G0-RET-VERSION-CITATION-PRECISION`: metric=1.0 threshold=1.0 pass=True evaluated=True
- `G0-RET-VERSION-CITATION-RECALL`: metric=1.0 threshold=1.0 pass=True evaluated=True

## Verdict

- P0-06 closed: **YES**

- All P0-06 retrieval/version/conflict gates passed with neural hybrid.
