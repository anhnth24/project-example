# P0-06 retrieval track

## Artifacts

- `expected-chunks.tsv` — canonical chunk catalog for `heading-chunks-2000-v1`
- `expected-chunks.meta.json` — counts + content hash
- `summary.json` — latest retrieval/version/conflict metrics

## Regenerate / evaluate

```bash
python3 bench/markhand_web/scripts/generate_expected_chunks.py
python3 bench/markhand_web/scripts/generate_expected_chunks.py --check
python3 bench/markhand_web/scripts/fill_citation_chunk_ids.py
python3 bench/markhand_web/scripts/fill_citation_chunk_ids.py --check
python3 scripts/validate_corpus.py
python3 bench/markhand_web/scripts/run_retrieval_eval.py --self-test
python3 bench/markhand_web/scripts/run_retrieval_eval.py
```

Identity digests match `crates/knowledge` schema v2 (see ADR 0006).
Neural hybrid uses pinned `AITeamVN/Vietnamese_Embedding` on CPU (same quality track as P0-05).

## Status

- Chunk catalog + citation span resolve: done
- Golden citation `chunkId` fill: done (mechanical annotation; semantic adjudication pin preserved)
- Neural lexical/vector/hybrid eval + light RRF vectorWeight tune: done
- Temporal / change / version-citation / conflict gates: done (deterministic offline rules)
